//! Real time (specification §12, L05-US08): `GET /api/v1/events`, a
//! Server-Sent Events stream of the signals of the signed-in member's
//! tenant (`infra::events`).
//!
//! Each API instance reads the Valkey stream of a tenant once, however many
//! browsers follow it ([`Realtime`]), and hands the signals to each
//! connection, which keeps those its member may see: tickets of projects
//! they see (tenant admins: all), their own new notifications. A browser
//! that reconnects sends `Last-Event-ID` and gets exactly what it missed;
//! if the log no longer holds it, a `resync` event tells it to read
//! everything again. Signals carry no data: the browser reads it through
//! the API. A connection ends after [`MAX_LIFETIME`]: the browser reconnects
//! at once, its session checked again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use domain::permissions::Permission;
use infra::events::{Entry, EventLog, Topic, id_order, is_stream_id};
use infra::uuid::Uuid;
use infra::valkey::Valkey;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use utoipa::ToSchema;

use crate::nodes::is_tenant_admin;
use crate::problem::{PROBLEM_JSON, Problem, ProblemBody};
use crate::tenant::{AppState, TenantTx};

const TAG: &str = "events";
/// How long a connection lasts before the browser opens a new one.
pub const MAX_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// How long a permission found stays trusted by a connection.
const PERMISSION_TTL: Duration = Duration::from_secs(60);
/// How long a read of the Valkey stream waits for signals.
const WAIT_MS: u64 = 1_000;

/// The readers of the event logs of the tenants this instance's browsers
/// follow: one per tenant, shared by its connections, stopped when none is
/// left.
pub struct Realtime {
    log: EventLog,
    channels: Mutex<HashMap<Uuid, broadcast::Sender<Entry>>>,
}

impl std::fmt::Debug for Realtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Realtime")
            .field("log", &self.log)
            .finish_non_exhaustive()
    }
}

impl Realtime {
    pub fn new(valkey: Valkey, prefix: &str) -> Self {
        Self {
            log: EventLog::new(valkey, prefix),
            channels: Mutex::new(HashMap::new()),
        }
    }

    /// The signals of `tenant` from now on.
    fn subscribe(self: &Arc<Self>, tenant: Uuid) -> broadcast::Receiver<Entry> {
        let mut channels = self
            .channels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = channels.get(&tenant) {
            return sender.subscribe();
        }
        let (sender, receiver) = broadcast::channel(256);
        channels.insert(tenant, sender.clone());
        tokio::spawn(read(Arc::clone(self), tenant, sender));
        receiver
    }
}

/// Reads the log of `tenant` and hands its signals to `sender`'s
/// receivers, until there is none.
async fn read(realtime: Arc<Realtime>, tenant: Uuid, sender: broadcast::Sender<Entry>) {
    // Blocking reads need a connection of their own.
    let reader = realtime.log.reader();
    let key = realtime.log.key(tenant);
    let mut last = realtime
        .log
        .last_id(tenant)
        .await
        .unwrap_or_else(|_| "$".to_owned());
    loop {
        if sender.receiver_count() == 0 {
            let mut channels = realtime
                .channels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A connection may have subscribed meanwhile.
            if sender.receiver_count() == 0 {
                if channels.get(&tenant).is_some_and(|s| s.same_channel(&sender)) {
                    channels.remove(&tenant);
                }
                break;
            }
        }
        match EventLog::wait(&reader, &key, &last, WAIT_MS).await {
            Ok(entries) => {
                for entry in entries {
                    last.clone_from(&entry.id);
                    // No receiver right now: dropped, the loop ends next turn.
                    drop(sender.send(entry));
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, %tenant, "real-time events not read");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    EventLog::close(&reader).await;
}

/// What a browser receives: which ticket changed, or that the member has
/// new notifications.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = RealtimeSignal)]
pub struct SignalView {
    /// `issue` or `notifications`.
    topic: String,
    /// `issue.updated`, `comment.created`…
    #[schema(required = true)]
    kind: Option<String>,
    #[schema(required = true)]
    issue_id: Option<Uuid>,
    #[schema(required = true)]
    issue_key: Option<String>,
    /// Who changed it; the member's own changes come back too.
    #[schema(required = true)]
    actor_id: Option<Uuid>,
}

/// Who a connection is for.
struct Viewer {
    state: AppState,
    tenant: Uuid,
    account: Uuid,
    admin: bool,
    /// Projects already checked, with when.
    seen: HashMap<Uuid, (bool, Instant)>,
}

impl Viewer {
    /// Whether the member may see a signal about `project`.
    async fn sees(&mut self, project: Uuid) -> bool {
        if self.admin {
            return true;
        }
        if let Some((sees, at)) = self.seen.get(&project)
            && at.elapsed() < PERMISSION_TTL
        {
            return *sees;
        }
        let sees = self.check(project).await.unwrap_or(false);
        self.seen.insert(project, (sees, Instant::now()));
        sees
    }

    async fn check(&self, project: Uuid) -> Result<bool, infra::sqlx::Error> {
        let mut tx = infra::db::begin_tenant_tx(&self.state.pool, self.tenant).await?;
        infra::db::set_account(&mut tx, self.account).await?;
        let roles = self
            .state
            .grants
            .roles_of(&mut tx, self.account, Some(project))
            .await?;
        let permissions = infra::roles::permissions_of(&mut tx, &roles).await?;
        tx.commit().await?;
        Ok(permissions.contains(Permission::View))
    }

    /// The event to send for `entry`, if the member may see it.
    async fn event_of(&mut self, entry: &Entry) -> Option<Event> {
        let signal = &entry.signal;
        let topic = match signal.topic {
            Topic::Issue => {
                if !self.sees(signal.project_id?).await {
                    return None;
                }
                "issue"
            }
            Topic::Notifications => {
                if !signal.recipients.contains(&self.account) {
                    return None;
                }
                "notifications"
            }
        };
        let view = SignalView {
            topic: topic.to_owned(),
            kind: signal.kind.clone(),
            issue_id: signal.issue_id,
            issue_key: signal.issue_key.clone(),
            actor_id: signal.actor_id,
        };
        Event::default()
            .id(entry.id.clone())
            .event(topic)
            .json_data(view)
            .ok()
    }
}

/// `GET /api/v1/events`.
#[utoipa::path(
    get,
    path = "/events",
    tag = TAG,
    operation_id = "streamEvents",
    summary = "Real-time signals (Server-Sent Events)",
    description = "A `text/event-stream` of the changes of the member's tenant they may see: \
                   `issue` (a ticket of a project they see changed: `issueId`, `issueKey`, \
                   `kind`, `actorId`), `notifications` (they have new ones), `ready` (the stream \
                   is open; its id is where it starts), `resync` (signals were missed: read \
                   everything again). Signals carry no data: read it through the API. Send \
                   `Last-Event-ID` to resume. A comment line every 15 seconds keeps it open; it \
                   ends after 10 minutes, and the browser reconnects.",
    params(("Last-Event-ID" = Option<String>, Header, description = "The id of the last event received")),
    responses(
        (status = 200, description = "The stream", content_type = "text/event-stream", body = SignalView),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn events(
    State(state): State<AppState>,
    tx: TenantTx,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    let (tenant, account, admin) = (tx.tenant_id(), tx.account_id(), is_tenant_admin(&tx));
    tx.commit().await?;
    let realtime = Arc::clone(&state.realtime);
    // Subscribed first: what comes meanwhile is in the replay or the channel.
    let mut live = realtime.subscribe(tenant);
    let resumed = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .filter(|id| is_stream_id(id))
        .map(str::to_owned);
    let start = match &resumed {
        Some(id) => id.clone(),
        None => realtime.log.last_id(tenant).await?,
    };
    let missed = realtime.log.since(tenant, &start).await?;
    let (out, stream) = mpsc::channel::<Result<Event, std::convert::Infallible>>(64);
    let mut viewer = Viewer {
        state: state.clone(),
        tenant,
        account,
        admin,
        seen: HashMap::new(),
    };
    tokio::spawn(async move {
        let opened = Instant::now();
        let mut last = start.clone();
        let first = if missed.gap {
            Event::default().id(start.clone()).event("resync").data("{}")
        } else {
            Event::default().id(start.clone()).event("ready").data("{}")
        };
        if out.send(Ok(first)).await.is_err() {
            return;
        }
        for entry in missed.entries {
            last.clone_from(&entry.id);
            if let Some(event) = viewer.event_of(&entry).await
                && out.send(Ok(event)).await.is_err()
            {
                return;
            }
        }
        loop {
            let remaining = MAX_LIFETIME.saturating_sub(opened.elapsed());
            let entry = tokio::select! {
                () = out.closed() => return,
                () = tokio::time::sleep(remaining) => return,
                entry = live.recv() => entry,
            };
            match entry {
                Ok(entry) => {
                    if id_order(&entry.id, &last) != std::cmp::Ordering::Greater {
                        continue;
                    }
                    last.clone_from(&entry.id);
                    if let Some(event) = viewer.event_of(&entry).await
                        && out.send(Ok(event)).await.is_err()
                    {
                        return;
                    }
                }
                // Too slow to follow: the browser reads everything again.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let resync = Event::default().id(last.clone()).event("resync").data("{}");
                    if out.send(Ok(resync)).await.is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    Ok(Sse::new(ReceiverStream::new(stream))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response())
}
