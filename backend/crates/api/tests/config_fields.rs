//! Integration tests of the fields of a configuration (L04-US03) on real
//! PostgreSQL and Valkey: the system fields of every configuration, custom
//! fields of every type with their options, the rules grid per type and
//! workflow step, list values deactivated once published, system fields
//! protected, fields following the types they name, who may edit them,
//! and tenant isolation.

#![allow(clippy::unwrap_used)]

mod common;

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn assert_problem(status: u16, body: &Value, expected_status: u16, code: &str) {
    assert_eq!(status, expected_status, "{body}");
    assert_eq!(body["code"], code, "{body}");
}

/// A request of the seeded tenant admin of `tenant`, in English.
fn admin(api: &TestApi, tenant: &SeedTenant, method: Method, path: &str) -> reqwest::RequestBuilder {
    api.request(method, tenant, path).header("accept-language", "en")
}

/// Creates a configuration as the tenant admin; its id.
async fn config(api: &TestApi, tenant: &SeedTenant, en: &str) -> String {
    let body = json!({"nameTranslations": {"en": en}});
    let (status, config) =
        send(admin(api, tenant, Method::POST, "/api/v1/project-configs").json(&body)).await;
    assert_eq!(status, 201, "{config}");
    config["id"].as_str().unwrap().to_owned()
}

fn base(config: &str) -> String {
    format!("/api/v1/project-configs/{config}")
}

async fn get(api: &TestApi, tenant: &SeedTenant, path: &str) -> Value {
    let (status, body) = send(admin(api, tenant, Method::GET, path)).await;
    assert_eq!(status, 200, "{body}");
    body
}

async fn fields(api: &TestApi, tenant: &SeedTenant, config: &str) -> Value {
    get(api, tenant, &format!("{}/fields", base(config))).await
}

fn by_key<'a>(list: &'a Value, key: &str) -> &'a Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["key"] == key)
        .unwrap_or_else(|| panic!("no {key} in {list}"))
}

/// The ids of the default types and of the standard statuses, by key.
struct Ids {
    types: Value,
    statuses: Value,
}

impl Ids {
    async fn of(api: &TestApi, tenant: &SeedTenant, config: &str) -> Self {
        let types = get(api, tenant, &format!("{}/issue-types", base(config))).await;
        let workflows = get(api, tenant, &format!("{}/workflows", base(config))).await;
        Self {
            types,
            statuses: json!({"items": workflows["items"][0]["statuses"]}),
        }
    }

    fn of_type(&self, key: &str) -> Value {
        by_key(&self.types, key)["id"].clone()
    }

    fn of_status(&self, key: &str) -> Value {
        by_key(&self.statuses, key)["id"].clone()
    }
}

fn labels(fr: &str, en: &str) -> Value {
    json!({"fr": fr, "en": en})
}

/// The English labels of the values of `field`, with whether each is active.
fn values(field: &Value) -> Vec<(String, bool)> {
    field["options"]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["label"].as_str().unwrap().to_owned(),
                v["active"].as_bool().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn application_is_created_with_its_values_and_rules_then_its_values_deactivated() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let ids = Ids::of(&api, t, &id).await;

    // The system fields, from the published version.
    let list = fields(&api, t, &id).await;
    assert_eq!(list["stage"], "published");
    let points = by_key(&list, "story_points");
    assert_eq!(
        (&points["label"], &points["kind"], &points["isSystem"]),
        (&json!("Points"), &json!("number"), &json!(true))
    );
    assert_eq!(
        points["issueTypeIds"],
        json!([ids.of_type("story"), ids.of_type("task")])
    );
    let remaining = by_key(&list, "remaining");
    assert_eq!(remaining["options"], json!({"decimals": 2, "unit": "hours"}));
    assert_eq!(remaining["allTypes"], true);

    // Application: a single select on Story, Bug and Task; Bug required at
    // creation, Task hidden there, Story required once done.
    let path = format!("{}/fields", base(&id));
    let body = json!({
        "key": "application",
        "labelTranslations": labels("Application", "Application"),
        "kind": "single_select",
        "options": {"values": [
            {"labelTranslations": labels("Annuaire", "Directory")},
            {"labelTranslations": labels("Messagerie", "Mail")},
            {"labelTranslations": labels("ERP", "ERP")},
        ]},
        "issueTypeIds": [ids.of_type("story"), ids.of_type("bug"), ids.of_type("task")],
        "rules": [
            {"issueTypeId": ids.of_type("bug"), "statusId": null, "mode": "required"},
            {"issueTypeId": ids.of_type("task"), "statusId": null, "mode": "hidden"},
            {"issueTypeId": ids.of_type("story"), "statusId": ids.of_status("done"), "mode": "required"},
            {"issueTypeId": ids.of_type("story"), "statusId": ids.of_status("in_progress"), "mode": "visible"},
        ],
    });
    let response = admin(&api, t, Method::POST, &path)
        .header("idempotency-key", "application-1")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["etag"], "\"1\"");
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    let (status, added) = json(response).await;
    assert_eq!(status, 201, "{added}");
    assert!(location.ends_with(added["id"].as_str().unwrap()));
    assert_eq!(added["kind"], "single_select");
    assert_eq!(
        values(&added),
        [
            ("Directory".into(), true),
            ("Mail".into(), true),
            ("ERP".into(), true)
        ]
    );
    assert_eq!(
        added["options"]["values"][0]["published"], false,
        "only in the draft"
    );
    // Stored per type and step, in the order of the types; Visible is the default.
    assert_eq!(
        added["rules"],
        json!([
            {"issueTypeId": ids.of_type("story"), "statusId": ids.of_status("done"), "mode": "required"},
            {"issueTypeId": ids.of_type("task"), "statusId": null, "mode": "hidden"},
            {"issueTypeId": ids.of_type("bug"), "statusId": null, "mode": "required"},
        ])
    );
    let list = fields(&api, t, &id).await;
    assert_eq!(list["stage"], "draft");
    assert_eq!(by_key(&list, "application")["id"], added["id"]);
    let read = get(&api, t, &base(&id)).await;
    // The field, and the general creation and view screens it joined (L04-US06).
    assert_eq!(read["draft"]["changeCount"], 3);

    // Published: its values are now the projects'.
    let publish = format!("{}/publish", base(&id));
    let (status, _) = send(admin(&api, t, Method::POST, &publish).header("if-match", "*")).await;
    assert_eq!(status, 200);
    let field_path = format!("{path}/{}", added["id"].as_str().unwrap());
    let published = get(&api, t, &field_path).await;
    assert!(
        published["options"]["values"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["published"] == true)
    );

    // ERP left out, Mail renamed, a new value: ERP stays, deactivated.
    let kept = |i: usize| published["options"]["values"][i]["id"].clone();
    let change = |values: Value| {
        json!({
            "labelTranslations": labels("Application", "Application"),
            "options": {"values": values},
            "issueTypeIds": published["issueTypeIds"],
            "rules": published["rules"],
        })
    };
    let first = change(json!([
        {"id": kept(0), "labelTranslations": labels("Annuaire", "Directory")},
        {"id": kept(1), "labelTranslations": labels("Courriel", "Email")},
        {"labelTranslations": labels("Portail client", "Customer portal")},
    ]));
    let (status, changed) = send(
        admin(&api, t, Method::PUT, &field_path)
            .header("if-match", "\"1\"")
            .json(&first),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(
        values(&changed),
        [
            ("Directory".into(), true),
            ("Email".into(), true),
            ("Customer portal".into(), true),
            ("ERP".into(), false),
        ]
    );
    assert_eq!(
        changed["options"]["values"][3]["id"],
        kept(2),
        "the same value, for its tickets"
    );
    // The new value goes for good: it was never published. ERP comes back.
    let portal = changed["options"]["values"][2].clone();
    assert_eq!(portal["published"], false);
    let second = change(json!([
        {"id": kept(0), "labelTranslations": labels("Annuaire", "Directory")},
        {"id": kept(1), "labelTranslations": labels("Courriel", "Email")},
        {"id": kept(2), "labelTranslations": labels("ERP", "ERP"), "active": true},
    ]));
    let (status, changed) = send(
        admin(&api, t, Method::PUT, &field_path)
            .header("if-match", "\"2\"")
            .json(&second),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(
        values(&changed),
        [
            ("Directory".into(), true),
            ("Email".into(), true),
            ("ERP".into(), true)
        ]
    );
    // A stale ETag is refused.
    let (status, stale) = send(
        admin(&api, t, Method::PUT, &field_path)
            .header("if-match", "\"2\"")
            .json(&second),
    )
    .await;
    assert_problem(status, &stale, 412, "precondition_failed");

    // Removed from the draft; the published version keeps it until published.
    let (status, _) = send(admin(&api, t, Method::DELETE, &field_path).header("if-match", "\"3\"")).await;
    assert_eq!(status, 204);
    let (status, gone) = send(admin(&api, t, Method::GET, &field_path)).await;
    assert_problem(status, &gone, 404, "not_found");
}

#[tokio::test]
async fn every_field_type_takes_its_own_options() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let path = format!("{}/fields", base(&id));
    let value = |en: &str| json!({"labelTranslations": {"en": en}});
    let kinds = [
        ("short_text", json!({"maxLength": 80})),
        ("rich_text", json!({})),
        (
            "number",
            json!({"decimals": 1, "min": 0, "max": 100, "unit": "percent"}),
        ),
        ("checkbox", json!({})),
        ("date", json!({})),
        ("date_time", json!({})),
        ("single_select", json!({"values": [value("Low"), value("High")]})),
        ("multi_select", json!({"values": [value("Web"), value("Mobile")]})),
        (
            "cascading_select",
            json!({"values": [{"labelTranslations": {"en": "ERP"}, "children": [value("Finance"), value("HR")]}]}),
        ),
        ("user", json!({"multiple": true})),
        ("group", json!({})),
        ("labels", json!({})),
        ("url", json!({})),
        ("issue_link", json!({})),
    ];
    for (kind, options) in &kinds {
        let body = json!({
            "key": kind,
            "labelTranslations": {"en": kind},
            "kind": kind,
            "options": options,
            "allTypes": true,
        });
        let (status, added) = send(admin(&api, t, Method::POST, &path).json(&body)).await;
        assert_eq!(status, 201, "{kind}: {added}");
        assert_eq!(added["kind"], *kind);
    }
    let list = fields(&api, t, &id).await;
    assert_eq!(list["items"].as_array().unwrap().len(), 2 + kinds.len());
    let cascade = by_key(&list, "cascading_select");
    let erp = &cascade["options"]["values"][0];
    assert_eq!(erp["children"].as_array().unwrap().len(), 2);
    assert_eq!(by_key(&list, "number")["options"]["unit"], "percent");

    // Options of another type, a list without values, and a key taken.
    for (body, field, code) in [
        (
            json!({"key": "notes", "labelTranslations": {"en": "Notes"}, "kind": "rich_text", "options": {"maxLength": 10}}),
            "options.maxLength",
            "not_applicable",
        ),
        (
            json!({"key": "env", "labelTranslations": {"en": "Env"}, "kind": "single_select"}),
            "options.values",
            "required",
        ),
        (
            json!({"key": "env", "labelTranslations": {"en": "Env"}, "kind": "single_select",
                   "options": {"values": [{"labelTranslations": {"en": "A"}, "children": [value("B")]}]}}),
            "options.values[0].children",
            "not_applicable",
        ),
        (
            json!({"key": "Env", "labelTranslations": {"en": "Env"}, "kind": "url"}),
            "key",
            "format",
        ),
    ] {
        let (status, refused) = send(admin(&api, t, Method::POST, &path).json(&body)).await;
        assert_problem(status, &refused, 400, "validation_failed");
        assert_eq!(refused["errors"][0]["field"], field, "{refused}");
        assert_eq!(refused["errors"][0]["code"], code, "{refused}");
    }
    let body = json!({"key": "url", "labelTranslations": {"en": "Again"}, "kind": "url"});
    let (status, taken) = send(admin(&api, t, Method::POST, &path).json(&body)).await;
    assert_problem(status, &taken, 409, "config_field.key_taken");
    let body = json!({"key": "x", "labelTranslations": {"en": "X"}, "kind": "formula"});
    let (status, unknown) = send(admin(&api, t, Method::POST, &path).json(&body)).await;
    assert_eq!(status, 400, "{unknown}");
}

#[tokio::test]
async fn rules_name_the_field_types_and_the_statuses_of_their_workflow() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let other = config(&api, t, "Support").await;
    let ids = Ids::of(&api, t, &id).await;
    let foreign = Ids::of(&api, t, &other).await;
    let path = format!("{}/fields", base(&id));
    let body = json!({
        "key": "environment",
        "labelTranslations": {"en": "Environment"},
        "kind": "short_text",
        "issueTypeIds": [ids.of_type("bug"), foreign.of_type("bug")],
        "rules": [
            {"issueTypeId": ids.of_type("story"), "statusId": null, "mode": "required"},
            {"issueTypeId": ids.of_type("bug"), "statusId": foreign.of_status("done"), "mode": "required"},
            {"issueTypeId": ids.of_type("bug"), "statusId": null, "mode": "required"},
            {"issueTypeId": ids.of_type("bug"), "statusId": null, "mode": "hidden"},
        ],
    });
    let (status, refused) = send(admin(&api, t, Method::POST, &path).json(&body)).await;
    assert_problem(status, &refused, 400, "validation_failed");
    let errors: Vec<(&str, &str)> = refused["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["field"].as_str().unwrap(), e["code"].as_str().unwrap()))
        .collect();
    assert_eq!(
        errors,
        [
            ("issueTypeIds[1]", "not_found"),
            ("rules[0].issueTypeId", "not_applicable"),
            ("rules[1].statusId", "not_found"),
            ("rules[3]", "duplicate"),
        ]
    );
    // Nothing went to a draft.
    assert_eq!(fields(&api, t, &id).await["stage"], "published");
}

#[tokio::test]
async fn system_fields_keep_their_label_and_options_and_change_their_rules() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let t = &db.tenant_a;
    let id = config(&api, t, "Software").await;
    let ids = Ids::of(&api, t, &id).await;
    let points = by_key(&fields(&api, t, &id).await, "story_points").clone();
    let path = format!("{}/fields/{}", base(&id), points["id"].as_str().unwrap());

    // Another label or other options: refused.
    for (labels, options) in [
        (labels("Estimation", "Estimate"), points["options"].clone()),
        (points["labelTranslations"].clone(), json!({"decimals": 0})),
    ] {
        let body =
            json!({"labelTranslations": labels, "options": options, "issueTypeIds": points["issueTypeIds"]});
        let (status, refused) = send(
            admin(&api, t, Method::PUT, &path)
                .header("if-match", "*")
                .json(&body),
        )
        .await;
        assert_problem(status, &refused, 409, "config_field.system");
    }
    // Its types and rules: Bug too, required on stories once in review.
    let body = json!({
        "labelTranslations": points["labelTranslations"],
        "options": points["options"],
        "issueTypeIds": [ids.of_type("story"), ids.of_type("task"), ids.of_type("bug")],
        "rules": [{"issueTypeId": ids.of_type("story"), "statusId": ids.of_status("in_review"), "mode": "required"}],
    });
    let (status, changed) = send(
        admin(&api, t, Method::PUT, &path)
            .header("if-match", "\"1\"")
            .json(&body),
    )
    .await;
    assert_eq!(status, 200, "{changed}");
    assert_eq!(changed["issueTypeIds"].as_array().unwrap().len(), 3);
    assert_eq!(changed["rules"][0]["mode"], "required");
    assert_eq!(changed["label"], "Points");

    // It stays.
    let (status, refused) = send(admin(&api, t, Method::DELETE, &path).header("if-match", "*")).await;
    assert_problem(status, &refused, 409, "config_field.system");

    // Bug removed: Points no longer lists it.
    let bug = format!(
        "{}/issue-types/{}",
        base(&id),
        ids.of_type("bug").as_str().unwrap()
    );
    let (status, _) = send(admin(&api, t, Method::DELETE, &bug).header("if-match", "*")).await;
    assert_eq!(status, 204);
    let after = get(&api, t, &path).await;
    assert_eq!(
        after["issueTypeIds"],
        json!([ids.of_type("story"), ids.of_type("task")])
    );
    assert_eq!(after["version"], 3);
}

#[tokio::test]
async fn only_configuration_admins_of_the_tenant_edit_fields() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let id = config(&api, a, "Software").await;
    let points = by_key(&fields(&api, a, &id).await, "story_points")["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let member = db.add_member(a, TenantRole::Member).await.account_id;
    let settings = common::session_settings();
    let session = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: settings.idle,
            max_age: settings.max_age,
        },
    )
    .create(member, a.id)
    .await
    .unwrap();
    let as_member = |method: Method, path: &str| {
        api.anonymous(method, a, path)
            .header("cookie", format!("mlbpp_session={}", session.token))
            .header("x-csrf-token", &session.session.csrf_token)
    };
    let new = json!({"key": "x", "labelTranslations": {"en": "X"}, "kind": "url"});
    let change = json!({"labelTranslations": {"en": "X"}});
    let list = format!("{}/fields", base(&id));
    let one = format!("{list}/{points}");
    for (method, path, body) in [
        (Method::GET, list.clone(), &new),
        (Method::POST, list.clone(), &new),
        (Method::PUT, one.clone(), &change),
        (Method::DELETE, one.clone(), &change),
    ] {
        let (status, refused) = send(
            as_member(method.clone(), &path)
                .header("if-match", "*")
                .json(body),
        )
        .await;
        assert_problem(status, &refused, 403, "forbidden");
        let (status, hidden) = send(admin(&api, b, method, &path).header("if-match", "*").json(body)).await;
        assert_problem(status, &hidden, 404, "not_found");
    }
    assert_eq!(fields(&api, a, &id).await["items"].as_array().unwrap().len(), 2);
}
