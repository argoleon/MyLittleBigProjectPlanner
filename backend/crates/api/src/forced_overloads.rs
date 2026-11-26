//! The forced overloads of the tenant (specification §8, L08-US03): every
//! assignment written beyond its resource's capacity plus tolerance in
//! blocking mode by a member allowed to force it. Tenant admins read them;
//! each one is also in its ticket's history.

use infra::overload::ForcedOverload;
use infra::uuid::Uuid;
use serde::Serialize;
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::extract::{ApiJson, ApiQuery};
use crate::issues::{ListParams, PersonView, person};
use crate::members::require_member_admin;
use crate::pagination::{PageParams, encode_cursor};
use crate::problem::{OverloadDay, PROBLEM_JSON, Problem, ProblemBody};
use crate::resources::TAG;
use crate::tenant::TenantTx;

/// A forced overload.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ForcedOverload)]
pub struct ForcedOverloadView {
    id: Uuid,
    assignment_id: Uuid,
    issue_id: Uuid,
    /// `null`: the ticket is gone.
    #[schema(required = true, example = "PLAT-148")]
    issue_key: Option<String>,
    #[schema(required = true)]
    issue_summary: Option<String>,
    resource_id: Uuid,
    /// `null`: the resource is gone.
    #[schema(required = true, example = "Nathan Roux")]
    resource_name: Option<String>,
    /// Who forced it; `null`: no longer a member.
    #[schema(required = true)]
    actor: Option<PersonView>,
    /// `interface`, `api`, `mcp`…
    #[schema(example = "interface")]
    origin: String,
    /// The days overloaded at the time, by date.
    days: Vec<OverloadDay>,
    #[serde(with = "time::serde::rfc3339")]
    #[schema(value_type = String, format = DateTime)]
    created_at: OffsetDateTime,
}

impl From<ForcedOverload> for ForcedOverloadView {
    fn from(forced: ForcedOverload) -> Self {
        Self {
            id: forced.id,
            assignment_id: forced.assignment_id,
            issue_id: forced.issue_id,
            issue_key: forced.issue_key,
            issue_summary: forced.issue_summary,
            resource_id: forced.resource_id,
            resource_name: forced.resource_name,
            actor: forced
                .actor_name
                .as_ref()
                .and_then(|name| person(Some(forced.actor_id), Some(name))),
            origin: forced.origin,
            days: forced
                .days
                .iter()
                .filter_map(|d| {
                    Some(OverloadDay {
                        date: infra::capacity::sql_date(d.date)?,
                        planned_minutes: d.planned,
                        available_minutes: d.available,
                        limit_minutes: d.limit,
                    })
                })
                .collect(),
            created_at: forced.created_at,
        }
    }
}

/// A page of forced overloads.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[schema(as = ForcedOverloadPage)]
pub struct ForcedOverloadPage {
    items: Vec<ForcedOverloadView>,
    /// Cursor of the next (older) page, `null` on the last page.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

/// `GET /api/v1/tenant/forced-overloads`.
#[utoipa::path(
    get,
    path = "/tenant/forced-overloads",
    tag = TAG,
    operation_id = "listForcedOverloads",
    summary = "List the forced overloads",
    description = "Tenant admins only, newest first: who forced an assignment beyond its \
                   resource's capacity plus tolerance, when, on which ticket and resource, and \
                   the days overloaded at the time.",
    params(ListParams),
    responses(
        (status = 200, description = "A page of forced overloads", body = ForcedOverloadPage),
        (status = 400, description = "Invalid `limit` or `cursor`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 401, description = "`unauthenticated`", body = ProblemBody, content_type = PROBLEM_JSON),
        (status = 403, description = "`forbidden`: not a tenant admin", body = ProblemBody, content_type = PROBLEM_JSON),
    )
)]
pub async fn list(
    mut tx: TenantTx,
    ApiQuery(params): ApiQuery<ListParams>,
) -> Result<ApiJson<ForcedOverloadPage>, Problem> {
    require_member_admin(&tx)?;
    let (limit, before) = PageParams {
        limit: params.limit,
        cursor: params.cursor,
    }
    .resolve()?;
    let page = infra::overload::list_forced(&mut tx, limit, before).await?;
    tx.commit().await?;
    Ok(ApiJson(ForcedOverloadPage {
        items: page.items.into_iter().map(ForcedOverloadView::from).collect(),
        next_cursor: page.next.map(encode_cursor),
    }))
}
