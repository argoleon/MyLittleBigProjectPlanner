//! Integration tests of moving tickets through their workflow (L05-US09)
//! on real PostgreSQL and Valkey: the transitions offered from a status and
//! to whom, a condition on a role (tenant admins meet it), validators, a
//! transition screen, post-functions in order, the fields required at the
//! target status, the version read, the outbox event; edit needed, tenant
//! isolation, archived projects.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

type Session = (String, String);

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

fn field_codes(body: &Value) -> Vec<(String, String)> {
    let mut codes: Vec<(String, String)> = body["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("no errors in {body}"))
        .iter()
        .map(|e| {
            (
                e["field"].as_str().unwrap().to_owned(),
                e["code"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    codes.sort();
    codes
}

fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path).header("accept-language", "en")
}

fn as_member(
    api: &TestApi,
    tenant: &SeedTenant,
    session: &Session,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    api.anonymous(method, tenant, path)
        .header("accept-language", "en")
        .header("cookie", format!("mlbpp_session={}", session.0))
        .header("x-csrf-token", &session.1)
}

async fn get(api: &TestApi, tenant: &SeedTenant, path: &str) -> Value {
    let (status, body) = send(admin(api, tenant, Method::GET, path)).await;
    assert_eq!(status, 200, "{body}");
    body
}

fn by<'a>(items: &'a Value, field: &str, value: &str) -> &'a Value {
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item[field] == value)
        .unwrap_or_else(|| panic!("no {field} {value} in {items}"))
}

async fn member(db: &TestDb, api: &TestApi, tenant: &SeedTenant) -> (Uuid, Session) {
    let account_id = db.add_member(tenant, TenantRole::Member).await.account_id;
    let settings = common::session_settings();
    let opened = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(account_id, tenant.id)
    .await
    .unwrap();
    (account_id, (opened.token, opened.session.csrf_token))
}

async fn role(db: &TestDb, tenant: &SeedTenant, label: &str) -> Uuid {
    infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = $2")
        .bind(tenant.id)
        .bind(label)
        .fetch_one(&db.owner_pool)
        .await
        .unwrap()
}

async fn grant(db: &TestDb, api: &TestApi, tenant: &SeedTenant, node: &str, label: &str, account: Uuid) {
    let role = role(db, tenant, label).await;
    let path = format!("/api/v1/nodes/{node}/grants");
    let (status, granted) =
        send(admin(api, tenant, Method::POST, &path).json(&json!({"roleId": role, "accountId": account})))
            .await;
    assert_eq!(status, 201, "{granted}");
}

async fn project(api: &TestApi, tenant: &SeedTenant, name: &str, code: &str) -> String {
    let body = json!({"type": "project", "name": name, "key": code});
    let (status, node) = send(admin(api, tenant, Method::POST, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().to_owned()
}

/// What the configuration of the test is made of.
struct Setup {
    remaining: String,
    points: String,
    application: String,
    directory: Value,
}

/// A configuration "Software" for `project`: Start assigns to whoever takes
/// it; Send to review needs the remaining work and shows the points on its
/// screen; Validate is for project managers, clears the remaining work and
/// adds a comment; Application is required on stories once done.
async fn software(db: &TestDb, api: &TestApi, tenant: &SeedTenant, project: &str) -> Setup {
    let (status, config) = send(
        admin(api, tenant, Method::POST, "/api/v1/project-configs")
            .json(&json!({"nameTranslations": {"en": "Software"}})),
    )
    .await;
    assert_eq!(status, 201, "{config}");
    let base = format!("/api/v1/project-configs/{}", config["id"].as_str().unwrap());
    let types = get(api, tenant, &format!("{base}/issue-types")).await["items"].clone();
    let story = by(&types, "key", "story")["id"].clone();
    let workflow = get(api, tenant, &format!("{base}/workflows")).await["items"][0].clone();
    let done = by(&workflow["statuses"], "key", "done")["id"].clone();
    let fields = get(api, tenant, &format!("{base}/fields")).await["items"].clone();
    let remaining = by(&fields, "key", "remaining")["id"].as_str().unwrap().to_owned();
    let points = by(&fields, "key", "story_points")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let body = json!({
        "key": "application",
        "labelTranslations": {"en": "Application"},
        "kind": "single_select",
        "options": {"values": [{"labelTranslations": {"en": "Directory"}}]},
        "issueTypeIds": [story],
        "rules": [{"issueTypeId": story, "statusId": done, "mode": "required"}],
    });
    let (status, application) =
        send(admin(api, tenant, Method::POST, &format!("{base}/fields")).json(&body)).await;
    assert_eq!(status, 201, "{application}");
    let body = json!({"kind": "transition", "nameTranslations": {"en": "Estimate"}, "fieldIds": [points]});
    let (status, screen) =
        send(admin(api, tenant, Method::POST, &format!("{base}/screens")).json(&body)).await;
    assert_eq!(status, 201, "{screen}");
    let manager = role(db, tenant, "Project manager").await;
    let open = json!({"roleIds": [], "groupIds": []});
    for (name, rules) in [
        (
            "Start",
            json!({"conditions": open, "validators": [], "postFunctions": [{"kind": "assign", "assignTo": "current_user"}], "screenId": null}),
        ),
        (
            "Send to review",
            json!({"conditions": open, "validators": [remaining], "postFunctions": [], "screenId": screen["id"]}),
        ),
        (
            "Validate",
            json!({"conditions": {"roleIds": [manager], "groupIds": []}, "validators": [],
                   "postFunctions": [{"kind": "clear_field", "fieldId": remaining}, {"kind": "add_comment", "text": "Validated"}],
                   "screenId": null}),
        ),
    ] {
        let workflow = get(api, tenant, &format!("{base}/workflows")).await["items"][0].clone();
        let transition = by(&workflow["transitions"], "name", name).clone();
        let body = json!({
            "nameTranslations": transition["nameTranslations"],
            "fromId": transition["fromId"],
            "toId": transition["toId"],
            "rules": rules,
        });
        let path = format!(
            "{base}/workflow-transitions/{}",
            transition["id"].as_str().unwrap()
        );
        let (status, saved) = send(
            admin(api, tenant, Method::PUT, &path)
                .header("if-match", "*")
                .json(&body),
        )
        .await;
        assert_eq!(status, 200, "{name}: {saved}");
    }
    let (status, published) =
        send(admin(api, tenant, Method::POST, &format!("{base}/publish")).header("if-match", "*")).await;
    assert_eq!(status, 200, "{published}");
    let path = format!("/api/v1/nodes/{project}/configuration");
    let (status, moved) =
        send(admin(api, tenant, Method::PUT, &path).json(&json!({"configId": config["id"]}))).await;
    assert_eq!(status, 200, "{moved}");
    Setup {
        remaining,
        points,
        directory: application["options"]["values"][0]["id"].clone(),
        application: application["id"].as_str().unwrap().to_owned(),
    }
}

fn names(transitions: &Value) -> Vec<&str> {
    transitions
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect()
}

async fn events(db: &TestDb, tenant: &SeedTenant, kind: &str) -> Vec<Value> {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    let events: Vec<Value> =
        infra::sqlx::query_scalar("SELECT payload FROM outbox_event WHERE kind = $1 ORDER BY created_at, id")
            .bind(kind)
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    tx.commit().await.unwrap();
    events
}

#[tokio::test]
async fn a_ticket_goes_through_its_workflow_with_the_rules_of_each_transition() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let plat = project(&api, t, "Identity platform", "PLAT").await;
    let setup = software(&db, &api, t, &plat).await;
    let form = get(&api, t, "/api/v1/projects/PLAT/issue-form").await;
    let story = by(&form["types"], "key", "story")["id"].clone();
    let (status, created) = send(
        admin(&api, t, Method::POST, "/api/v1/projects/PLAT/issues")
            .json(&json!({"typeId": story, "summary": "OIDC"})),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let (hugo, contributor) = member(&db, &api, t).await;
    grant(&db, &api, t, &plat, "Contributor", hugo).await;
    let run = |session: &Session, version: Option<&str>, body: Value| {
        let request = as_member(
            &api,
            t,
            session,
            Method::POST,
            "/api/v1/issues/PLAT-1/transitions",
        )
        .json(&body);
        match version {
            Some(version) => request.header("if-match", version.to_owned()),
            None => request,
        }
    };
    let offered =
        |session: &Session| as_member(&api, t, session, Method::GET, "/api/v1/issues/PLAT-1/transitions");

    // To do: Start, and Cancel from any status.
    let (status, transitions) = send(offered(&contributor)).await;
    assert_eq!(status, 200, "{transitions}");
    assert_eq!(names(&transitions), ["Start", "Cancel"]);
    assert_eq!(transitions[0]["toStatus"]["label"], "In progress");
    assert_eq!(transitions[0]["fields"], json!([]));
    let start = transitions[0]["id"].clone();

    // Start: the version read; assigned to whoever takes it.
    let (status, refused) = send(run(&contributor, None, json!({"transitionId": start}))).await;
    assert_problem(status, &refused, 428, "precondition_required");
    let (status, started) = send(run(&contributor, Some("\"1\""), json!({"transitionId": start}))).await;
    assert_eq!(status, 200, "{started}");
    assert_eq!(
        (
            &started["status"]["key"],
            &started["assignee"]["id"],
            &started["version"]
        ),
        (&json!("in_progress"), &json!(hugo), &json!(2))
    );
    let (status, stale) = send(run(&contributor, Some("\"1\""), json!({"transitionId": start}))).await;
    assert_problem(status, &stale, 412, "precondition_failed");
    // Not offered from In progress.
    let (status, refused) = send(run(&contributor, Some("*"), json!({"transitionId": start}))).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [("transitionId".to_owned(), "not_found".to_owned())]
    );

    // Send to review: the points of its screen, and its validator, the
    // remaining work, required.
    let (_, transitions) = send(offered(&contributor)).await;
    let review = by(&transitions, "name", "Send to review").clone();
    let taken: Vec<(&Value, &Value)> = review["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (&f["fieldId"], &f["mode"]))
        .collect();
    assert_eq!(
        taken,
        [
            (&json!(setup.points), &json!("visible")),
            (&json!(setup.remaining), &json!("required"))
        ]
    );
    let (status, refused) = send(run(
        &contributor,
        Some("*"),
        json!({"transitionId": review["id"], "fields": {&setup.points: 3, &setup.application: setup.directory}}),
    ))
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [(
            format!("fields.{}", setup.application),
            "not_applicable".to_owned()
        )]
    );
    let (status, refused) = send(run(
        &contributor,
        Some("*"),
        json!({"transitionId": review["id"], "fields": {&setup.points: 3}}),
    ))
    .await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [(format!("fields.{}", setup.remaining), "required".to_owned())]
    );
    let (status, _) = send(
        admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({"fields": {&setup.remaining: 4, &setup.application: setup.directory}})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, reviewed) = send(run(
        &contributor,
        Some("*"),
        json!({"transitionId": review["id"], "fields": {&setup.points: 3}}),
    ))
    .await;
    assert_eq!(status, 200, "{reviewed}");
    assert_eq!(reviewed["status"]["key"], "in_review");
    assert_eq!(by(&reviewed["fields"], "key", "story_points")["value"], 3);

    // Validate: for project managers only; tenant admins meet it.
    let (_, transitions) = send(offered(&contributor)).await;
    assert_eq!(names(&transitions), ["Reject", "Cancel"]);
    let validate = by(
        &get(&api, t, "/api/v1/issues/PLAT-1/transitions").await,
        "name",
        "Validate",
    )
    .clone();
    // Done takes the application, required there.
    assert_eq!(
        (&validate["fields"][0]["fieldId"], &validate["fields"][0]["mode"]),
        (&json!(setup.application), &json!("required"))
    );
    let (status, refused) = send(run(
        &contributor,
        Some("*"),
        json!({"transitionId": validate["id"]}),
    ))
    .await;
    assert_problem(status, &refused, 403, "permission.denied");
    assert_eq!(
        get(&api, t, "/api/v1/issues/PLAT-1").await["status"]["key"],
        "in_review"
    );
    let (pia, manager) = member(&db, &api, t).await;
    grant(&db, &api, t, &plat, "Project manager", pia).await;
    let (_, transitions) = send(offered(&manager)).await;
    assert!(names(&transitions).contains(&"Validate"));

    // Done needs the application: cleared first, refused; then validated,
    // the remaining work cleared, the comment carried by the event.
    let (status, _) = send(
        admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({"fields": {&setup.application: null}})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, refused) = send(run(&manager, Some("*"), json!({"transitionId": validate["id"]}))).await;
    assert_problem(status, &refused, 400, "validation_failed");
    assert_eq!(
        field_codes(&refused),
        [(format!("fields.{}", setup.application), "required".to_owned())]
    );
    let (status, _) = send(
        admin(&api, t, Method::PATCH, "/api/v1/issues/PLAT-1")
            .header("if-match", "*")
            .json(&json!({"fields": {&setup.application: setup.directory}})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, done) = send(
        admin(&api, t, Method::POST, "/api/v1/issues/PLAT-1/transitions")
            .header("if-match", "*")
            .json(&json!({"transitionId": validate["id"]})),
    )
    .await;
    assert_eq!(status, 200, "{done}");
    assert_eq!(done["status"]["category"], "done");
    assert!(
        done["fields"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["key"] != "remaining"),
        "{done}"
    );

    let moves = events(&db, t, "issue.transitioned").await;
    assert_eq!(moves.len(), 3);
    assert_eq!(
        moves[0]["changes"],
        json!([{"field": "assigneeId", "old": null, "new": hugo}])
    );
    assert_eq!(moves[2]["comments"], json!(["Validated"]));
    // Its "add a comment" is an internal comment by whoever ran it (L05-US04).
    let comments = get(&api, t, "/api/v1/issues/PLAT-1/comments").await;
    assert_eq!(comments.as_array().unwrap().len(), 1);
    assert_eq!(
        (
            &comments[0]["contentHtml"],
            &comments[0]["visibility"],
            &comments[0]["byTransition"]
        ),
        (&json!("<p>Validated</p>"), &json!("internal"), &json!(true))
    );
    assert_eq!(
        comments[0]["author"]["id"],
        json!(api.session(t).member.account_id)
    );
    assert_eq!(
        moves[2]["changes"],
        json!([{"field": format!("fields.{}", setup.remaining), "old": 4, "new": null}])
    );
}

#[tokio::test]
async fn transitions_need_edit_and_stay_in_their_tenant() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let plat = project(&api, a, "Identity platform", "PLAT").await;
    let form = get(&api, a, "/api/v1/projects/PLAT/issue-form").await;
    let story = by(&form["types"], "key", "story")["id"].clone();
    let (status, _) = send(
        admin(&api, a, Method::POST, "/api/v1/projects/PLAT/issues")
            .json(&json!({"typeId": story, "summary": "Mine"})),
    )
    .await;
    assert_eq!(status, 201);
    let start = by(
        &get(&api, a, "/api/v1/issues/PLAT-1/transitions").await,
        "name",
        "Start",
    )["id"]
        .clone();
    let (lea, reader) = member(&db, &api, a).await;
    grant(&db, &api, a, &plat, "Reader", lea).await;
    let body = json!({"transitionId": start});

    for method in [Method::GET, Method::POST] {
        let (status, refused) = send(
            as_member(
                &api,
                a,
                &reader,
                method.clone(),
                "/api/v1/issues/PLAT-1/transitions",
            )
            .header("if-match", "*")
            .json(&body),
        )
        .await;
        assert_problem(status, &refused, 403, "permission.denied");
        let (status, hidden) = send(
            admin(&api, b, method, "/api/v1/issues/PLAT-1/transitions")
                .header("if-match", "*")
                .json(&body),
        )
        .await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    let (status, _) = send(
        admin(&api, a, Method::PUT, &format!("/api/v1/nodes/{plat}/archiving"))
            .header("if-match", "*")
            .json(&json!({"archived": true})),
    )
    .await;
    assert_eq!(status, 200);
    let (status, archived) = send(
        admin(&api, a, Method::POST, "/api/v1/issues/PLAT-1/transitions")
            .header("if-match", "*")
            .json(&body),
    )
    .await;
    assert_problem(status, &archived, 409, "node.archived");
}
