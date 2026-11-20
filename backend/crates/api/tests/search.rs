//! Integration tests of `POST /api/v1/search` (L06-US01) on real PostgreSQL
//! and Valkey: results with the fields asked for, Problem Details with the
//! position of an error, pages and their cursor, the projects in sight,
//! tenant isolation, full text (L06-US03), and the time it takes on 100,000
//! tickets.

#![allow(clippy::unwrap_used)]

mod common;

use std::time::{Duration, Instant};

use common::{TestApi, json, multi};
use infra::accounts::TenantRole;
use infra::uuid::Uuid;
use reqwest::Method;
use serde_json::{Value, json};
use testkit::{SeedTenant, TestDb};

async fn send(request: reqwest::RequestBuilder) -> (u16, Value) {
    json(request.send().await.unwrap()).await
}

fn search_request(api: &TestApi, tenant: &SeedTenant, body: &Value) -> reqwest::RequestBuilder {
    api.request(Method::POST, tenant, "/api/v1/search")
        .header("accept-language", "en")
        .json(body)
}

async fn search(api: &TestApi, tenant: &SeedTenant, body: Value) -> Value {
    let (status, page) = send(search_request(api, tenant, &body)).await;
    assert_eq!(status, 200, "{body}: {page}");
    page
}

fn keys(page: &Value) -> Vec<&str> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["key"].as_str().unwrap())
        .collect()
}

async fn node(api: &TestApi, tenant: &SeedTenant, body: Value) -> Uuid {
    let (status, node) = send(api.request(Method::POST, tenant, "/api/v1/nodes").json(&body)).await;
    assert_eq!(status, 201, "{node}");
    node["id"].as_str().unwrap().parse().unwrap()
}

/// `count` tickets of `project` straight in SQL, numbered from `first`:
/// statuses, priorities, assignees, due dates and one of five topics in the
/// description in rotation.
async fn seed(
    db: &TestDb,
    tenant: &SeedTenant,
    project: Uuid,
    key: &str,
    first: i32,
    count: i32,
    assignees: &[Uuid],
) {
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, tenant.id).await.unwrap();
    infra::sqlx::query(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, priority,
                           assignee_id, due_date, created_at, description)
        SELECT uuidv7(), app_current_tenant(), $1, n, $2 || '-' || n,
               (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                AND v.state = 'published' WHERE t.key = 'task' LIMIT 1),
               (SELECT s.item_id FROM workflow_status s JOIN project_config_version v ON v.id = s.version_id
                AND v.state = 'published' WHERE s.key = (ARRAY['todo', 'in_progress', 'in_review', 'done'])[n % 4 + 1]
                LIMIT 1),
               'Ticket ' || n,
               (ARRAY['lowest', 'low', 'medium', 'high', 'highest'])[n % 5 + 1],
               ($5::uuid[])[n % (cardinality($5::uuid[]) + 1)],
               CASE WHEN n % 3 = 0 THEN NULL ELSE CURRENT_DATE + (n % 30) END,
               now() - make_interval(mins => n),
               jsonb_build_object('type', 'doc', 'content', jsonb_build_array(jsonb_build_object(
                   'type', 'paragraph', 'content', jsonb_build_array(jsonb_build_object('type', 'text', 'text',
                   (ARRAY['Outage of the directory', 'Printer jam on the second floor', 'Password reset for a newcomer',
                          'VPN access from home', 'Laptop replacement after a fall'])[n % 5 + 1])))))
        FROM generate_series($3::int, $3::int + $4::int - 1) AS n
        "#,
    )
    .bind(project)
    .bind(key)
    .bind(first)
    .bind(count)
    .bind(assignees)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn my_open_tickets_come_sorted_with_the_fields_asked_for() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let me = api.session(a).member.account_id;
    let plat = node(
        &api,
        a,
        json!({"type": "project", "name": "Platform", "key": "PLAT"}),
    )
    .await;
    // 8 tickets: statuses todo, in_progress, in_review, done in turn; mine
    // when n % 2 = 1 (assignees [me]: n % 2 picks me or nobody).
    seed(&db, a, plat, "PLAT", 1, 8, &[me]).await;

    let page = search(
        &api,
        a,
        json!({
            "query": "assignee = currentUser() AND status IN (todo, in_progress) ORDER BY priority DESC",
            "fields": ["key", "priority", "status"]
        }),
    )
    .await;
    // Mine: the odd ones; open: status todo or in_progress; by priority, highest first.
    let expected: Vec<String> = {
        let statuses = ["todo", "in_progress", "in_review", "done"];
        let mut open: Vec<usize> = (1..=8_usize)
            .filter(|n| n % 2 == 1 && ["todo", "in_progress"].contains(&statuses[n % 4]))
            .collect();
        // The rank of a ticket's priority is n % 5 (lowest 0, highest 4): highest first, then by key.
        open.sort_by_key(|n| (std::cmp::Reverse(n % 5), *n));
        open.into_iter().map(|n| format!("PLAT-{n}")).collect()
    };
    assert!(!expected.is_empty());
    assert_eq!(keys(&page), expected);
    assert_eq!(page["total"], expected.len());
    assert_eq!(page["nextCursor"], Value::Null);
    let first = &page["items"][0];
    let mut names: Vec<&str> = first.as_object().unwrap().keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["id", "key", "priority", "status"],
        "only the fields asked for, and the id"
    );
    assert!(first["status"]["label"].as_str().is_some(), "{first}");

    // Every field by default.
    let all = search(&api, a, json!({"query": "key = PLAT-1"})).await;
    let item = &all["items"][0];
    for field in [
        "id",
        "key",
        "summary",
        "project",
        "type",
        "status",
        "priority",
        "assignee",
        "reporter",
        "dueDate",
        "createdAt",
    ] {
        assert!(item.get(field).is_some(), "{field} in {item}");
    }
    assert_eq!(item["project"]["key"], "PLAT");
    assert_eq!(item["assignee"]["id"], me.to_string());
}

#[tokio::test]
async fn a_query_that_cannot_run_answers_problem_details_with_its_position() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let problem = |body: Value| {
        let api = &api;
        async move {
            let (status, problem) = send(search_request(api, a, &body)).await;
            assert_eq!(status, 400, "{problem}");
            assert_eq!(problem["code"], "query.invalid", "{problem}");
            problem
        }
    };

    let syntax = problem(json!({"query": "status = "})).await;
    assert_eq!(
        syntax["query"],
        json!({"reason": "syntax", "position": 9, "length": 0, "expected": ["a value"], "found": null})
    );
    assert_eq!(
        syntax["detail"],
        "Expected a value at position 9, found the end of the query."
    );
    let unexpected = problem(json!({"query": "status = todo priority = high"})).await;
    assert_eq!(unexpected["query"]["position"], 14);
    assert_eq!(unexpected["query"]["found"], "priority");
    assert_eq!(
        unexpected["query"]["expected"],
        json!(["AND", "OR", "ORDER BY", "the end of the query"])
    );

    let field = problem(json!({"query": "text = outage"})).await;
    assert_eq!(
        field["query"],
        json!({"reason": "operator_not_allowed", "position": 0, "length": 13, "found": null, "field": "text", "allowed": ["~", "!~"]})
    );
    assert_eq!(
        problem(json!({"query": "severity = critical"})).await["query"]["reason"],
        "unknown_field"
    );
    assert_eq!(
        problem(json!({"query": "sprint IN openSprints()"})).await["query"]["reason"],
        "unsupported"
    );
    assert_eq!(
        problem(json!({"query": "due < tomorrow"})).await["query"]["reason"],
        "invalid_date"
    );
    assert_eq!(
        problem(json!({"query": "summary ~ \"open"})).await["query"]["reason"],
        "unterminated_string"
    );

    // A SQL fragment is a value: the query runs, finds nothing.
    let page = search(
        &api,
        a,
        json!({"query": "summary ~ \"x'; DROP TABLE issue; --\""}),
    )
    .await;
    assert_eq!(
        (page["items"].clone(), page["total"].clone()),
        (json!([]), json!(0))
    );
}

#[tokio::test]
async fn pages_follow_the_cursor_and_requests_are_checked() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let plat = node(
        &api,
        a,
        json!({"type": "project", "name": "Platform", "key": "PLAT"}),
    )
    .await;
    seed(&db, a, plat, "PLAT", 1, 7, &[]).await;
    let query = "ORDER BY key DESC";

    let first = search(&api, a, json!({"query": query, "limit": 3})).await;
    assert_eq!(
        (keys(&first), first["total"].clone()),
        (vec!["PLAT-7", "PLAT-6", "PLAT-5"], json!(7))
    );
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = search(&api, a, json!({"query": query, "limit": 3, "cursor": cursor})).await;
    assert_eq!(
        (keys(&second), second["total"].clone()),
        (vec!["PLAT-4", "PLAT-3", "PLAT-2"], Value::Null)
    );
    let third = search(
        &api,
        a,
        json!({"query": query, "limit": 3, "cursor": second["nextCursor"]}),
    )
    .await;
    assert_eq!(
        (keys(&third), third["nextCursor"].clone()),
        (vec!["PLAT-1"], Value::Null)
    );

    for (body, field) in [
        (json!({"query": "ORDER BY key", "cursor": cursor}), "cursor"),
        (json!({"limit": 0}), "limit"),
        (json!({"limit": 101}), "limit"),
        (json!({"fields": ["key", "nope"]}), "fields.nope"),
        (json!({"expand": ["children"]}), "expand.children"),
    ] {
        let (status, problem) = send(search_request(&api, a, &body)).await;
        assert_eq!(
            (status, problem["code"].as_str()),
            (400, Some("validation_failed")),
            "{body}: {problem}"
        );
        assert_eq!(problem["errors"][0]["field"], field, "{problem}");
    }
    let (status, problem) = send(search_request(&api, a, &json!({"query": "", "where": 1}))).await;
    assert_eq!((status, problem["code"].as_str()), (400, Some("invalid_json")));

    // The parent, in a few words, on demand.
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    infra::sqlx::query(
        "UPDATE issue SET parent_id = (SELECT id FROM issue WHERE key = 'PLAT-1') WHERE key = 'PLAT-2'",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let page = search(
        &api,
        a,
        json!({"query": "parent = PLAT-1", "expand": ["parent"], "fields": ["key", "parent"]}),
    )
    .await;
    assert_eq!(
        page["items"],
        json!([{"id": page["items"][0]["id"], "key": "PLAT-2", "parent": {"id": page["items"][0]["parent"]["id"], "key": "PLAT-1", "summary": "Ticket 1"}}])
    );
}

#[tokio::test]
async fn members_find_only_the_tickets_they_see_and_tenants_stay_apart() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let (a, b) = (&db.tenant_a, &db.tenant_b);
    let hr = node(
        &api,
        a,
        json!({"type": "portfolio", "name": "Human resources", "key": "PORTF-HR"}),
    )
    .await;
    let pay = node(
        &api,
        a,
        json!({"type": "project", "name": "Payroll", "key": "PAY", "parentId": hr}),
    )
    .await;
    let secret = node(
        &api,
        a,
        json!({"type": "project", "name": "Restructuring", "key": "SECRET", "parentId": hr}),
    )
    .await;
    let (status, body) = send(
        api.request(Method::PUT, a, &format!("/api/v1/nodes/{secret}/confidentiality"))
            .header("if-match", "*")
            .json(&json!({"confidential": true})),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    seed(&db, a, pay, "PAY", 1, 2, &[]).await;
    seed(&db, a, secret, "SECRET", 1, 2, &[]).await;
    let member = db.add_member(a, TenantRole::Member).await;
    let reader: Uuid =
        infra::sqlx::query_scalar("SELECT id FROM role WHERE tenant_id = $1 AND label ->> 'en' = 'Reader'")
            .bind(a.id)
            .fetch_one(&db.owner_pool)
            .await
            .unwrap();
    let (status, body) = send(
        api.request(Method::POST, a, &format!("/api/v1/nodes/{hr}/grants"))
            .json(&json!({"roleId": reader, "accountId": member.account_id})),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let session = infra::sessions::SessionStore::new(
        api.valkey.clone(),
        api.key_prefix.clone(),
        infra::sessions::SessionLifetime {
            idle: common::session_settings().idle,
            max_age: common::session_settings().max_age,
        },
    )
    .create(member.account_id, a.id)
    .await
    .unwrap();
    let query = json!({"query": "project IN descendantsOf(\"PORTF-HR\") ORDER BY key"});
    let (status, page) = send(
        api.anonymous(Method::POST, a, "/api/v1/search")
            .header("cookie", format!("mlbpp_session={}", session.token))
            .header("x-csrf-token", &session.session.csrf_token)
            .json(&query),
    )
    .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(
        (keys(&page), page["total"].clone()),
        (vec!["PAY-1", "PAY-2"], json!(2)),
        "not the confidential project"
    );
    let everything = search(&api, a, query.clone()).await;
    assert_eq!(
        keys(&everything),
        ["PAY-1", "PAY-2", "SECRET-1", "SECRET-2"],
        "the tenant admin sees it"
    );

    // Tenant B has its own PORTF-HR and PAY: never alpha's tickets.
    let hr_b = node(
        &api,
        b,
        json!({"type": "portfolio", "name": "RH", "key": "PORTF-HR"}),
    )
    .await;
    let pay_b = node(
        &api,
        b,
        json!({"type": "project", "name": "Paie", "key": "PAY", "parentId": hr_b}),
    )
    .await;
    seed(&db, b, pay_b, "PAY", 7, 1, &[]).await;
    assert_eq!(keys(&search(&api, b, query).await), ["PAY-7"]);
    assert_eq!(
        keys(&search(&api, b, json!({"query": "key IN (PAY-1, SECRET-1)"})).await),
        Vec::<&str>::new()
    );
}

#[tokio::test]
async fn text_finds_tickets_despite_a_typo_best_matches_first() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let plat = node(
        &api,
        a,
        json!({"type": "project", "name": "Platform", "key": "PLAT"}),
    )
    .await;
    // PLAT-5 and -10: "Outage of the directory" in the description (the
    // fifth topic of the rotation); PLAT-6 gets it in its title.
    seed(&db, a, plat, "PLAT", 1, 10, &[]).await;
    let mut tx = infra::db::begin_tenant_tx(&db.app_pool, a.id).await.unwrap();
    infra::sqlx::query("UPDATE issue SET summary = 'Directory outage, again' WHERE key = 'PLAT-6'")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // The title weighs more than the description; then the newest first.
    let page = search(&api, a, json!({"query": "text ~ \"directroy outgae\""})).await;
    assert_eq!(keys(&page), ["PLAT-6", "PLAT-5", "PLAT-10"]);
    assert_eq!(page["total"], 3);
    let page = search(&api, a, json!({"query": "text ~ outage ORDER BY key DESC"})).await;
    assert_eq!(keys(&page), ["PLAT-10", "PLAT-6", "PLAT-5"]);
    let page = search(&api, a, json!({"query": "text ~ printer AND text !~ floor"})).await;
    assert_eq!(page["total"], 0);
}

/// The 95th percentile of `times`.
fn p95(mut times: Vec<Duration>) -> Duration {
    times.sort();
    let rank = (times.len() * 95).div_ceil(100);
    times[rank.saturating_sub(1)]
}

#[tokio::test]
async fn searching_a_hundred_thousand_tickets_answers_within_200_ms_at_the_95th_percentile() {
    let db = TestDb::new().await;
    let api = common::start(&db, multi()).await;
    let a = &db.tenant_a;
    let me = api.session(a).member.account_id;
    let others: Vec<Uuid> = {
        let mut others = Vec::new();
        for _ in 0..4 {
            others.push(db.add_member(a, TenantRole::Member).await.account_id);
        }
        others
    };
    let mut assignees = vec![me];
    assignees.extend(others);
    let hr = node(
        &api,
        a,
        json!({"type": "portfolio", "name": "Human resources", "key": "PORTF-HR"}),
    )
    .await;
    // 20 projects of 5,000 tickets, half of them under the portfolio.
    for p in 0..20 {
        let key = format!("P{p:02}");
        let parent = if p % 2 == 0 { json!(hr) } else { Value::Null };
        let project = node(
            &api,
            a,
            json!({"type": "project", "name": format!("Project {p}"), "key": key, "parentId": parent}),
        )
        .await;
        seed(&db, a, project, &key, 1, 5_000, &assignees).await;
    }
    infra::sqlx::query("ANALYZE issue")
        .execute(&db.owner_pool)
        .await
        .unwrap();

    let queries = [
        "",
        "status IN (todo, in_progress) ORDER BY priority DESC",
        "assignee = currentUser() AND status != done ORDER BY due",
        "project IN descendantsOf(\"PORTF-HR\") AND priority >= high ORDER BY updated DESC",
        "due > now() AND assignee IS NOT EMPTY ORDER BY key",
        // A fifth of the tickets, ranked; a typo; a rare word (20 tickets).
        "text ~ \"directory outage\"",
        "text ~ \"pasword\" AND status != done",
        "text ~ \"4217\"",
    ];
    for query in queries {
        let page = search(&api, a, json!({"query": query})).await;
        assert!(page["total"].as_i64().unwrap() > 0, "{query}");
    }
    let mut times = Vec::new();
    for _ in 0..8 {
        for query in queries {
            let started = Instant::now();
            let (status, page) = send(search_request(&api, a, &json!({"query": query}))).await;
            times.push(started.elapsed());
            assert_eq!(status, 200, "{page}");
        }
    }
    let p95 = p95(times);
    assert!(p95 < Duration::from_millis(200), "95th percentile {p95:?}");
}
