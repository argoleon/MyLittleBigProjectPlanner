//! Integration tests on a real PostgreSQL with row-level security: the
//! query language compiled to SQL and run (L06-US01). Each acceptance
//! criterion of the story, plus custom fields, full text, the two-valued
//! logic of empty fields and cursor pagination.

#![allow(clippy::unwrap_used)]

use domain::hierarchy::{Health, NodeType};
use domain::query::{QueryErrorKind, parse};
use infra::accounts::TenantRole;
use infra::db::begin_tenant_tx;
use infra::full_text::Audience;
use infra::grants::{self, Grantee, NewGrant};
use infra::id::new_id;
use infra::nodes::{self, NewNode, NodeFields};
use infra::search::{self, Context, SearchError, SearchPage};
use infra::sqlx::{self, PgConnection};
use infra::uuid::Uuid;
use testkit::TestDb;
use time::Date;

async fn node(
    conn: &mut PgConnection,
    node_type: NodeType,
    name: &str,
    key: &str,
    parent: Option<Uuid>,
    by: Uuid,
) -> Uuid {
    let new = NewNode {
        id: new_id(),
        node_type,
        key,
        fields: NodeFields {
            key: None,
            name,
            description: "",
            objectives: "",
            start_date: None,
            end_date: None,
            health: Health::OnTrack,
            owners: &[],
        },
        parent,
        created_by: by,
    };
    nodes::insert(conn, &new).await.unwrap().unwrap().id
}

/// A ticket to insert, straight in SQL: the tests choose its dates.
#[derive(Debug, Clone)]
struct Ticket<'a> {
    project: Uuid,
    key: &'a str,
    summary: &'a str,
    status: &'a str,
    priority: &'a str,
    assignee: Option<Uuid>,
    due: Option<Date>,
    created_days_ago: i32,
    points: Option<f64>,
    description: Option<&'a str>,
}

fn ticket(project: Uuid, key: &str) -> Ticket<'_> {
    Ticket {
        project,
        key,
        summary: key,
        status: "todo",
        priority: "medium",
        assignee: None,
        due: None,
        created_days_ago: 0,
        points: None,
        description: None,
    }
}

async fn insert(conn: &mut PgConnection, t: &Ticket<'_>) {
    let number: i32 = t.key.rsplit('-').next().unwrap().parse().unwrap();
    let points: Uuid = sqlx::query_scalar(
        "SELECT f.item_id FROM field_def f JOIN project_config_version v ON v.id = f.version_id \
         AND v.state = 'published' WHERE f.key = 'story_points' LIMIT 1",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    let fields = match t.points {
        Some(n) => serde_json::json!({ points.to_string(): n }),
        None => serde_json::json!({}),
    };
    let description = t.description.map(|text| {
        serde_json::json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]})
    });
    sqlx::query(
        r#"
        INSERT INTO issue (id, tenant_id, project_id, key_number, key, type_id, status_id, summary, priority,
                           assignee_id, due_date, created_at, fields, description)
        VALUES ($1, app_current_tenant(), $2, $3, $4,
                (SELECT t.item_id FROM issue_type t JOIN project_config_version v ON v.id = t.version_id
                 AND v.state = 'published' WHERE t.key = 'task' LIMIT 1),
                (SELECT s.item_id FROM workflow_status s JOIN project_config_version v ON v.id = s.version_id
                 AND v.state = 'published' WHERE s.key = $5 LIMIT 1),
                $6, $7, $8, $9, now() - make_interval(days => $10), $11, $12)
        "#,
    )
    .bind(new_id())
    .bind(t.project)
    .bind(number)
    .bind(t.key)
    .bind(t.status)
    .bind(t.summary)
    .bind(t.priority)
    .bind(t.assignee)
    .bind(t.due)
    .bind(t.created_days_ago)
    .bind(fields)
    .bind(description)
    .execute(&mut *conn)
    .await
    .unwrap();
}

fn admin_context(account_id: Uuid) -> Context<'static> {
    Context {
        account_id,
        time_zone: "Europe/Paris",
        visible_projects: None,
        audience: Audience::Members,
    }
}

async fn run(conn: &mut PgConnection, query: &str, context: &Context<'_>) -> Result<SearchPage, SearchError> {
    search::search(conn, &parse(query).unwrap(), context, None, 100, true).await
}

async fn keys(conn: &mut PgConnection, query: &str, context: &Context<'_>) -> Vec<String> {
    let page = run(conn, query, context).await.unwrap();
    assert_eq!(
        page.total,
        Some(i64::try_from(page.items.len()).unwrap()),
        "{query}"
    );
    page.items.into_iter().map(|issue| issue.key).collect()
}

fn refusal(result: Result<SearchPage, SearchError>) -> QueryErrorKind {
    match result {
        Err(SearchError::Query(error)) => error.kind,
        other => panic!("not a query error: {other:?}"),
    }
}

#[tokio::test]
async fn my_open_tickets_come_sorted_by_priority() {
    let db = TestDb::new().await;
    let me = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let other = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, me).await;
    for (key, status, priority, assignee) in [
        ("PLAT-1", "todo", "high", me),
        ("PLAT-2", "in_progress", "highest", me),
        ("PLAT-3", "done", "highest", me),
        ("PLAT-4", "todo", "highest", other),
        ("PLAT-5", "todo", "low", me),
    ] {
        let t = Ticket {
            status,
            priority,
            assignee: Some(assignee),
            ..ticket(plat, key)
        };
        insert(&mut tx, &t).await;
    }

    let mine = keys(
        &mut tx,
        "assignee = currentUser() AND status IN (todo, in_progress) ORDER BY priority DESC",
        &admin_context(me),
    )
    .await;
    assert_eq!(mine, ["PLAT-2", "PLAT-1", "PLAT-5"]);
    let theirs = keys(&mut tx, "assignee = currentUser()", &admin_context(other)).await;
    assert_eq!(theirs, ["PLAT-4"]);
}

#[tokio::test]
async fn descendants_of_a_portfolio_come_once_each_and_only_those_in_sight() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let reader = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let hr = node(
        &mut tx,
        NodeType::Portfolio,
        "Human resources",
        "PORTF-HR",
        None,
        admin,
    )
    .await;
    let pay = node(&mut tx, NodeType::Program, "Payroll", "PROG-PAY", Some(hr), admin).await;
    let payroll = node(
        &mut tx,
        NodeType::Project,
        "Payroll engine",
        "PAY",
        Some(pay),
        admin,
    )
    .await;
    // Reached twice: under the portfolio and under its program.
    let hris = node(
        &mut tx,
        NodeType::Project,
        "HR information system",
        "HRIS",
        Some(hr),
        admin,
    )
    .await;
    nodes::link(&mut tx, pay, hris, 50).await.unwrap().unwrap();
    let secret = node(
        &mut tx,
        NodeType::Project,
        "Restructuring",
        "SECRET",
        Some(hr),
        admin,
    )
    .await;
    nodes::set_confidential(&mut tx, secret, None, true)
        .await
        .unwrap()
        .unwrap();
    let outside = node(&mut tx, NodeType::Project, "Website", "WEB", None, admin).await;
    for (project, key) in [
        (payroll, "PAY-1"),
        (hris, "HRIS-1"),
        (hris, "HRIS-2"),
        (secret, "SECRET-1"),
        (outside, "WEB-1"),
    ] {
        insert(&mut tx, &ticket(project, key)).await;
    }
    let reader_role: Uuid = sqlx::query_scalar("SELECT id FROM role WHERE label ->> 'en' = 'Reader'")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    grants::insert(
        &mut tx,
        &NewGrant {
            id: new_id(),
            node: hr,
            role: reader_role,
            grantee: Grantee::Member(reader),
            created_by: admin,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let query = "project IN descendantsOf(\"PORTF-HR\") ORDER BY key";

    // The member reads the portfolio: the confidential project stays out.
    let sight: Vec<Uuid> = grants::sight(&mut tx, reader)
        .await
        .unwrap()
        .visible
        .into_iter()
        .collect();
    let context = Context {
        account_id: reader,
        time_zone: "Europe/Paris",
        visible_projects: Some(&sight),
        audience: Audience::Members,
    };
    assert_eq!(
        keys(&mut tx, query, &context).await,
        ["HRIS-1", "HRIS-2", "PAY-1"]
    );
    // A tenant admin sees it; the project outside never comes.
    assert_eq!(
        keys(&mut tx, query, &admin_context(admin)).await,
        ["HRIS-1", "HRIS-2", "PAY-1", "SECRET-1"]
    );
    assert_eq!(
        keys(&mut tx, "project = hris ORDER BY key DESC", &admin_context(admin)).await,
        ["HRIS-2", "HRIS-1"]
    );
    assert_eq!(
        keys(
            &mut tx,
            "project IN descendantsOf(\"NOPE\")",
            &admin_context(admin)
        )
        .await,
        Vec::<String>::new()
    );
    tx.commit().await.unwrap();

    // Another tenant with the same keys: each sees its own only.
    let admin_b = db
        .add_member(&db.tenant_b, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut b = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
    let hr_b = node(
        &mut b,
        NodeType::Portfolio,
        "Ressources humaines",
        "PORTF-HR",
        None,
        admin_b,
    )
    .await;
    let pay_b = node(&mut b, NodeType::Project, "Paie", "PAY", Some(hr_b), admin_b).await;
    insert(&mut b, &ticket(pay_b, "PAY-7")).await;
    assert_eq!(keys(&mut b, query, &admin_context(admin_b)).await, ["PAY-7"]);
    assert_eq!(
        keys(&mut b, "key IN (HRIS-1, PAY-1)", &admin_context(admin_b)).await,
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn relative_dates_are_evaluated_in_the_members_time_zone() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, admin).await;
    // UTC+14 and UTC-11: 25 hours apart, so never the same date.
    let (east, west) = ("Pacific/Kiritimati", "Pacific/Pago_Pago");
    let today_east: Date = sqlx::query_scalar("SELECT (now() AT TIME ZONE $1)::date")
        .bind(east)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    insert(
        &mut tx,
        &Ticket {
            due: Some(today_east),
            created_days_ago: 2,
            ..ticket(plat, "PLAT-1")
        },
    )
    .await;
    insert(
        &mut tx,
        &Ticket {
            created_days_ago: 10,
            ..ticket(plat, "PLAT-2")
        },
    )
    .await;
    let at = |time_zone| Context {
        time_zone,
        ..admin_context(admin)
    };
    let due_today = "due <= endOfDay()";
    assert_eq!(
        keys(&mut tx, due_today, &at(east)).await,
        ["PLAT-1"],
        "today in Kiritimati"
    );
    assert_eq!(
        keys(&mut tx, due_today, &at(west)).await,
        Vec::<String>::new(),
        "tomorrow in Pago Pago"
    );
    assert_eq!(
        keys(&mut tx, "due < endOfWeek() AND due >= startOfDay()", &at(east)).await,
        ["PLAT-1"]
    );
    assert_eq!(keys(&mut tx, "created > -7d", &at(west)).await, ["PLAT-1"]);
    assert_eq!(
        keys(&mut tx, "created <= -1w ORDER BY created", &at(east)).await,
        ["PLAT-2"]
    );
    assert_eq!(
        keys(&mut tx, &format!("due = \"{today_east}\""), &at(west)).await,
        ["PLAT-1"],
        "a date literal is a local date"
    );

    // The member's own zone, else the tenant's.
    sqlx::query("UPDATE tenant SET time_zone = 'Asia/Tokyo' WHERE id = $1")
        .bind(db.tenant_a.id)
        .execute(&db.owner_pool)
        .await
        .unwrap();
    assert_eq!(search::time_zone_of(&mut tx, admin).await.unwrap(), "Asia/Tokyo");
    sqlx::query("UPDATE account SET time_zone = $1 WHERE id = $2")
        .bind(east)
        .bind(admin)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(search::time_zone_of(&mut tx, admin).await.unwrap(), east);
}

#[tokio::test]
async fn a_sql_fragment_in_a_value_is_only_a_value() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, admin).await;
    let fragment = "x'; DROP TABLE issue; --";
    insert(
        &mut tx,
        &Ticket {
            summary: fragment,
            ..ticket(plat, "PLAT-1")
        },
    )
    .await;
    insert(&mut tx, &ticket(plat, "PLAT-2")).await;
    let context = admin_context(admin);
    assert_eq!(
        keys(&mut tx, "summary ~ \"x'; DROP TABLE issue; --\"", &context).await,
        ["PLAT-1"]
    );
    for query in [
        "key = \"PLAT-1' OR '1'='1\"",
        "status IN (\"todo') OR true --\")",
        "assignee = \"x' OR 1=1 --\"",
        "summary ~ \"%\"",
        "summary ~ \"_\"",
    ] {
        assert_eq!(
            keys(&mut tx, query, &context).await,
            Vec::<String>::new(),
            "{query}"
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM issue")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(count, 2, "nothing was dropped");
}

#[tokio::test]
async fn custom_fields_full_text_and_empty_fields() {
    let db = TestDb::new().await;
    let me = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let other = db.add_member(&db.tenant_a, TenantRole::Member).await.account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, me).await;
    for t in [
        Ticket {
            points: Some(5.0),
            assignee: Some(me),
            description: Some("The login page is down: an outage of the SSO."),
            ..ticket(plat, "PLAT-1")
        },
        Ticket {
            points: Some(2.0),
            assignee: Some(other),
            summary: "Outage report",
            ..ticket(plat, "PLAT-2")
        },
        Ticket {
            priority: "high",
            ..ticket(plat, "PLAT-3")
        },
    ] {
        insert(&mut tx, &t).await;
    }
    let context = admin_context(me);
    assert_eq!(keys(&mut tx, "story_points > 3", &context).await, ["PLAT-1"]);
    assert_eq!(
        keys(
            &mut tx,
            "story_points IN (2, 5) ORDER BY story_points DESC",
            &context
        )
        .await,
        ["PLAT-1", "PLAT-2"]
    );
    assert_eq!(keys(&mut tx, "story_points IS EMPTY", &context).await, ["PLAT-3"]);
    assert_eq!(
        keys(&mut tx, "text ~ OUTAGE ORDER BY key", &context).await,
        ["PLAT-1", "PLAT-2"]
    );
    assert_eq!(keys(&mut tx, "description ~ \"sso\"", &context).await, ["PLAT-1"]);
    assert_eq!(keys(&mut tx, "summary ~ outage", &context).await, ["PLAT-2"]);
    // An empty field fails a condition, whatever NOT does around it.
    assert_eq!(
        keys(&mut tx, "assignee != currentUser()", &context).await,
        ["PLAT-2"]
    );
    assert_eq!(
        keys(&mut tx, "NOT assignee = currentUser() ORDER BY key", &context).await,
        ["PLAT-2", "PLAT-3"]
    );
    assert_eq!(
        keys(&mut tx, "assignee IS EMPTY OR priority > medium", &context).await,
        ["PLAT-3"]
    );

    let ctx = admin_context(me);
    assert_eq!(
        refusal(run(&mut tx, "severity = critical", &ctx).await),
        QueryErrorKind::UnknownField("severity".to_owned())
    );
    assert_eq!(
        refusal(run(&mut tx, "story_points ~ 3", &ctx).await),
        QueryErrorKind::OperatorNotAllowed {
            field: "story_points".to_owned(),
            operator: "~".to_owned(),
            allowed: [
                "=",
                "!=",
                "<",
                "<=",
                ">",
                ">=",
                "IN",
                "NOT IN",
                "IS EMPTY",
                "IS NOT EMPTY"
            ]
            .map(str::to_owned)
            .to_vec(),
        }
    );
    assert_eq!(
        refusal(run(&mut tx, "sprint IN openSprints()", &ctx).await),
        QueryErrorKind::Unsupported("sprint".to_owned())
    );
}

#[tokio::test]
async fn pages_follow_the_sort_order_through_a_cursor() {
    let db = TestDb::new().await;
    let admin = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, admin).await;
    let priorities = ["lowest", "low", "medium", "high", "highest"];
    let mut expected = Vec::new();
    for n in 1..=23_u32 {
        let key = format!("PLAT-{n}");
        let priority = priorities[(n % 5) as usize];
        // Every third ticket has no due date: those come last.
        let due = (n % 3 != 0).then(|| {
            Date::from_calendar_date(2026, time::Month::December, u8::try_from(n % 7 + 1).unwrap()).unwrap()
        });
        expected.push((5 - (n % 5), due.is_none(), due, key.clone()));
        insert(
            &mut tx,
            &Ticket {
                priority,
                due,
                ..ticket(plat, &key)
            },
        )
        .await;
    }
    let query = parse("ORDER BY priority DESC, due ASC").unwrap();
    let context = admin_context(admin);
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = search::search(&mut tx, &query, &context, cursor.as_deref(), 5, cursor.is_none())
            .await
            .unwrap();
        assert_eq!(page.total, if pages == 0 { Some(23) } else { None });
        pages += 1;
        seen.extend(page.items.into_iter().map(|issue| issue.key));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(pages, 5);
    // Priority rank, then due date with empty ones last, then id (insertion order).
    expected.sort_by_key(|e| (e.0, e.1, e.2));
    let in_order: Vec<String> = expected.into_iter().map(|e| e.3).collect();
    assert_eq!(seen, in_order);

    // A cursor fits only the order it came from.
    let first = search::search(&mut tx, &query, &context, None, 5, false)
        .await
        .unwrap();
    let other = parse("ORDER BY due ASC").unwrap();
    let result = search::search(&mut tx, &other, &context, first.next_cursor.as_deref(), 5, false).await;
    assert!(matches!(result, Err(SearchError::Cursor)));
    let result = search::search(&mut tx, &query, &context, Some("garbage"), 5, false).await;
    assert!(matches!(result, Err(SearchError::Cursor)));
}

// --- Full text (L06-US03) ----------------------------------------------------

/// A comment on the ticket `key`, straight in SQL; `None` text: deleted.
async fn comment(conn: &mut PgConnection, key: &str, text: Option<&str>, visibility: &str) {
    let content = text.map(|text| {
        serde_json::json!({"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": text}]}]})
    });
    sqlx::query(
        r#"
        INSERT INTO comment (id, tenant_id, issue_id, visibility, content, content_html, deleted_at)
        VALUES ($1, app_current_tenant(), (SELECT id FROM issue WHERE key = $2), $3, $4,
                CASE WHEN $4::jsonb IS NULL THEN NULL ELSE '<p>…</p>' END,
                CASE WHEN $4::jsonb IS NULL THEN now() END)
        "#,
    )
    .bind(new_id())
    .bind(key)
    .bind(visibility)
    .bind(content)
    .execute(&mut *conn)
    .await
    .unwrap();
}

/// The keys found by `query`, in key order (`keys` keeps the query's order).
async fn sorted_keys(conn: &mut PgConnection, query: &str, context: &Context<'_>) -> Vec<String> {
    let mut found = keys(conn, query, context).await;
    found.sort();
    found
}

#[tokio::test]
async fn text_matches_stemmed_words_in_french_and_in_english_without_accents() {
    let db = TestDb::new().await;
    let me = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, me).await;
    insert(
        &mut tx,
        &Ticket {
            summary: "Pannes électriques du bâtiment",
            ..ticket(plat, "PLAT-1")
        },
    )
    .await;
    insert(
        &mut tx,
        &Ticket {
            description: Some("The servers keep running out of memory"),
            ..ticket(plat, "PLAT-2")
        },
    )
    .await;
    insert(
        &mut tx,
        &Ticket {
            summary: "Nothing to see",
            ..ticket(plat, "PLAT-3")
        },
    )
    .await;
    let context = admin_context(me);
    // French: plural and singular, accents or not, any case.
    for query in [
        "text ~ panne",
        "text ~ electrique",
        "text ~ \"ÉLECTRIQUE\"",
        "text ~ batiment",
    ] {
        assert_eq!(keys(&mut tx, query, &context).await, ["PLAT-1"], "{query}");
    }
    // English: "running" and "servers" from their stems.
    for query in ["text ~ run", "text ~ server", "text ~ \"memory running\""] {
        assert_eq!(keys(&mut tx, query, &context).await, ["PLAT-2"], "{query}");
    }
    // Every word is needed.
    assert!(
        keys(&mut tx, "text ~ \"server panne\"", &context)
            .await
            .is_empty()
    );
    // The title only, the description only.
    assert_eq!(keys(&mut tx, "summary ~ panne", &context).await, ["PLAT-1"]);
    assert!(keys(&mut tx, "summary ~ memory", &context).await.is_empty());
    assert_eq!(keys(&mut tx, "description ~ memory", &context).await, ["PLAT-2"]);
    assert_eq!(
        sorted_keys(&mut tx, "text !~ panne", &context).await,
        ["PLAT-2", "PLAT-3"]
    );
}

#[tokio::test]
async fn a_typo_finds_the_ticket_but_not_the_words_it_merely_resembles() {
    let db = TestDb::new().await;
    let me = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, me).await;
    insert(
        &mut tx,
        &Ticket {
            description: Some("outage of the directory"),
            ..ticket(plat, "PLAT-1")
        },
    )
    .await;
    for (key, summary) in [
        ("PLAT-2", "Output of the export"),
        ("PLAT-3", "Outlook calendar"),
        ("PLAT-4", "Outbound mail"),
    ] {
        insert(
            &mut tx,
            &Ticket {
                summary,
                ..ticket(plat, key)
            },
        )
        .await;
    }
    let context = admin_context(me);
    for query in [
        "text ~ outgae",
        "text ~ otuage",
        "text ~ \"directroy outgae\"",
        "description ~ outage",
    ] {
        assert_eq!(keys(&mut tx, query, &context).await, ["PLAT-1"], "{query}");
    }
    // A word the tenant wrote is never corrected.
    assert_eq!(keys(&mut tx, "text ~ output", &context).await, ["PLAT-2"]);
    // Too far from any word, or too short to correct.
    assert!(keys(&mut tx, "text ~ outrageous", &context).await.is_empty());
    assert!(keys(&mut tx, "text ~ otu", &context).await.is_empty());
}

#[tokio::test]
async fn the_best_matches_come_first_and_their_pages_follow() {
    let db = TestDb::new().await;
    let me = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, me).await;
    // In the description only, the newest; in the title; in a comment only.
    insert(
        &mut tx,
        &Ticket {
            description: Some("an outage of the directory"),
            ..ticket(plat, "PLAT-1")
        },
    )
    .await;
    insert(
        &mut tx,
        &Ticket {
            summary: "Directory outage",
            created_days_ago: 3,
            ..ticket(plat, "PLAT-2")
        },
    )
    .await;
    insert(
        &mut tx,
        &Ticket {
            created_days_ago: 5,
            ..ticket(plat, "PLAT-3")
        },
    )
    .await;
    comment(&mut tx, "PLAT-3", Some("the directory outage again"), "internal").await;
    let context = admin_context(me);
    let ranked = ["PLAT-2", "PLAT-1", "PLAT-3"];
    assert_eq!(
        keys(&mut tx, "text ~ \"directory outage\"", &context).await,
        ranked
    );
    // One by one, each page after the cursor of the one before.
    let query = parse("text ~ \"directory outage\"").unwrap();
    let mut cursor: Option<String> = None;
    let mut paged = Vec::new();
    loop {
        let page = search::search(&mut tx, &query, &context, cursor.as_deref(), 1, false)
            .await
            .unwrap();
        paged.extend(page.items.into_iter().map(|found| found.key));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(paged, ranked);
    // An explicit order wins.
    assert_eq!(
        keys(&mut tx, "text ~ \"directory outage\" ORDER BY key", &context).await,
        ["PLAT-1", "PLAT-2", "PLAT-3"]
    );
}

#[tokio::test]
async fn comments_count_for_members_and_only_public_ones_for_the_portal() {
    let db = TestDb::new().await;
    let me = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, me).await;
    for key in ["PLAT-1", "PLAT-2", "PLAT-3"] {
        insert(&mut tx, &ticket(plat, key)).await;
    }
    comment(
        &mut tx,
        "PLAT-1",
        Some("Internal note: the directory outage"),
        "internal",
    )
    .await;
    comment(
        &mut tx,
        "PLAT-2",
        Some("We are fixing the directory outage"),
        "public",
    )
    .await;
    comment(&mut tx, "PLAT-3", None, "public").await;
    let members = admin_context(me);
    let portal = Context {
        audience: Audience::Portal,
        ..admin_context(me)
    };
    let query = "text ~ \"directory outage\"";
    assert_eq!(sorted_keys(&mut tx, query, &members).await, ["PLAT-1", "PLAT-2"]);
    assert_eq!(keys(&mut tx, query, &portal).await, ["PLAT-2"]);
    // A typo corrected through a word of an internal comment still finds
    // nothing the portal may not read.
    assert_eq!(
        keys(&mut tx, "text ~ \"directroy outgae\"", &portal).await,
        ["PLAT-2"]
    );
    // Comments are not the title.
    assert!(keys(&mut tx, "summary ~ outage", &members).await.is_empty());
}

#[tokio::test]
async fn neither_the_tickets_nor_the_words_of_another_tenant_are_used() {
    let db = TestDb::new().await;
    let a = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let b = db
        .add_member(&db.tenant_b, TenantRole::TenantAdmin)
        .await
        .account_id;
    {
        let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_b.id).await.unwrap();
        let other = node(&mut tx, NodeType::Project, "Other", "OTHR", None, b).await;
        insert(
            &mut tx,
            &Ticket {
                summary: "Directory outage",
                ..ticket(other, "OTHR-1")
            },
        )
        .await;
        tx.commit().await.unwrap();
    }
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, a).await;
    insert(
        &mut tx,
        &Ticket {
            summary: "Printer jam",
            ..ticket(plat, "PLAT-1")
        },
    )
    .await;
    let context = admin_context(a);
    assert!(keys(&mut tx, "text ~ outage", &context).await.is_empty());
    let words: Vec<String> = sqlx::query_scalar("SELECT word FROM search_word ORDER BY word")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        words,
        ["printer"],
        "only this tenant's words, of 4 letters or more"
    );
    // "outgae" is a typo of nothing tenant A wrote.
    insert(
        &mut tx,
        &Ticket {
            summary: "Outrage",
            ..ticket(plat, "PLAT-2")
        },
    )
    .await;
    assert!(keys(&mut tx, "text ~ outgae", &context).await.is_empty());
}

#[tokio::test]
async fn new_words_are_found_as_tickets_and_comments_change() {
    let db = TestDb::new().await;
    let me = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, me).await;
    insert(&mut tx, &ticket(plat, "PLAT-1")).await;
    let context = admin_context(me);
    assert!(keys(&mut tx, "text ~ firewal", &context).await.is_empty());
    sqlx::query("UPDATE issue SET summary = 'Firewall rules' WHERE key = 'PLAT-1'")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(keys(&mut tx, "text ~ firewal", &context).await, ["PLAT-1"]);
    sqlx::query(
        "UPDATE issue SET description = '{\"type\": \"doc\", \"content\": [{\"type\": \"text\", \"text\": \"certificate renewal\"}]}' \
         WHERE key = 'PLAT-1'",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        keys(&mut tx, "description ~ certifcate", &context).await,
        ["PLAT-1"]
    );
    comment(&mut tx, "PLAT-1", Some("Rebooting the gateway"), "internal").await;
    assert_eq!(
        keys(&mut tx, "text ~ \"rebootign gatewya\"", &context).await,
        ["PLAT-1"]
    );
}

#[tokio::test]
async fn any_text_is_searched_as_words_never_as_operators() {
    let db = TestDb::new().await;
    let me = db
        .add_member(&db.tenant_a, TenantRole::TenantAdmin)
        .await
        .account_id;
    let mut tx = begin_tenant_tx(&db.app_pool, db.tenant_a.id).await.unwrap();
    let plat = node(&mut tx, NodeType::Project, "Platform", "PLAT", None, me).await;
    insert(
        &mut tx,
        &Ticket {
            summary: "O'Brien's laptop",
            ..ticket(plat, "PLAT-1")
        },
    )
    .await;
    let context = admin_context(me);
    assert_eq!(keys(&mut tx, "text ~ \"o'brien\"", &context).await, ["PLAT-1"]);
    for query in [
        r#"text ~ "a | b & !c""#,
        r#"text ~ "x:* <-> (y""#,
        r#"text ~ "\\ ' '' \"""#,
        r#"text ~ "!!! ???""#,
        r#"text ~ "the of a""#,
    ] {
        let found = run(&mut tx, query, &context).await;
        assert!(found.is_ok(), "{query}: {found:?}");
    }
    assert!(
        keys(&mut tx, r#"text ~ "!!! ???""#, &context).await.is_empty(),
        "no word: nothing"
    );
}
