//! SQL of the availability search (specification §8, L08-US10): the
//! resources' calendar days and daily load, whatever the tickets behind it
//! (confidential ones included: only minutes come out).

use std::collections::HashMap;

use domain::availability::{Availability, availability};
use domain::dates::Date as LocalDate;
use sqlx::postgres::PgConnection;
use time::Date;
use uuid::Uuid;

use crate::capacity::local_date;

/// The availability of `resources` (id and site of each) from `from` to
/// `to`, provisional load counted when `include_provisional`; a resource
/// whose calendar cannot be read has none.
pub async fn of(
    conn: &mut PgConnection,
    resources: &[(Uuid, Option<Uuid>)],
    from: Date,
    to: Date,
    include_provisional: bool,
) -> Result<HashMap<Uuid, Availability>, sqlx::Error> {
    let (Some(first), Some(last)) = (local_date(from), local_date(to)) else {
        return Ok(HashMap::new());
    };
    if resources.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<Uuid> = resources.iter().map(|(id, _)| *id).collect();
    let rows = sqlx::query!(
        r#"
        SELECT resource_id, day, sum(minutes)::integer AS "minutes!"
        FROM resource_day_load
        WHERE resource_id = ANY($1) AND day BETWEEN $2 AND $3 AND ($4 OR NOT provisional)
        GROUP BY resource_id, day
        "#,
        &ids,
        from,
        to,
        include_provisional
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut load: HashMap<Uuid, HashMap<LocalDate, u32>> = HashMap::new();
    for row in rows {
        if let Some(day) = local_date(row.day) {
            load.entry(row.resource_id)
                .or_default()
                .insert(day, u32::try_from(row.minutes).unwrap_or(0));
        }
    }
    let inputs = crate::day_capacity::load(conn, resources, from, to).await?;
    let empty = HashMap::new();
    let mut found = HashMap::new();
    for (id, _) in resources {
        let levels = inputs.levels(*id);
        let Some(calendar) = inputs.calendar(*id, &levels) else {
            continue;
        };
        let days = calendar.days(first, last);
        found.insert(*id, availability(&days, load.get(id).unwrap_or(&empty)));
    }
    Ok(found)
}
