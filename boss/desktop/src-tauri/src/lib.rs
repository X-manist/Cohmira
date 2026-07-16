mod db;
mod error;
pub mod server;

pub use db::{database_path, BossCore};
pub use error::{BossError, BossResult};

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use base64::Engine;
use chrono::{Datelike, Duration, Local, NaiveDate, Utc, Weekday};
use regex::Regex;
use rusqlite::types::Type;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, ToSql};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use wait_timeout::ChildExt;

pub const TOOL_NAMES: &[&str] = &[
    "boss.owner_context",
    "boss.dashboard_summary",
    "boss.employee_work_report",
    "boss.save_week_review",
    "employee.report_work_event",
    "employee.list_week_reviews",
    "owner.create_binding_code",
    "owner.list_bound_employees",
    "ledger.add_transaction",
    "ledger.list_transactions",
    "ledger.update_transaction_status",
    "ledger.category_report",
    "ledger.report",
    "invoice.upload_and_extract",
    "invoice.extract_fields",
    "invoice.list_drafts",
    "invoice.post_to_ledger",
    "actual.integration_status",
    "actual.sync_transaction",
    "actual.export_import_file",
];

pub fn load_environment() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        PathBuf::from(".env"),
        manifest.join("../.env"),
        manifest.join("../../.env"),
    ];
    for path in candidates {
        if path.is_file() {
            let _ = dotenvy::from_path(path);
        }
    }
}

pub fn dispatch_tool(core: &BossCore, name: &str, arguments: Value) -> BossResult<Value> {
    if !TOOL_NAMES.contains(&name) {
        return Err(BossError::NotFound(format!("未知老板工具：{name}")));
    }
    let args = arguments
        .as_object()
        .ok_or_else(|| BossError::Validation("arguments 必须是 JSON 对象".to_string()))?;
    // Actual CLI can block for tens of seconds. It stages and finishes its DB job under
    // separate locks, and intentionally never holds a SQLite mutex while the process runs.
    if name == "actual.sync_transaction" {
        return actual_sync_transaction(core, args);
    }
    let mut connection = core.lock()?;
    match name {
        "boss.owner_context" => boss_owner_context(&mut connection, args),
        "boss.dashboard_summary" => boss_dashboard_summary(&connection, args),
        "boss.employee_work_report" => boss_employee_work_report(&connection, args),
        "boss.save_week_review" => boss_save_week_review(&mut connection, args),
        "employee.report_work_event" => employee_report_work_event(&mut connection, args),
        "employee.list_week_reviews" => employee_list_week_reviews(&connection, args),
        "owner.create_binding_code" => owner_create_binding_code(&mut connection, args),
        "owner.list_bound_employees" => owner_list_bound_employees(&connection, args),
        "ledger.add_transaction" => ledger_add_transaction(&mut connection, args),
        "ledger.list_transactions" => ledger_list_transactions(&connection, args),
        "ledger.update_transaction_status" => {
            ledger_update_transaction_status(&mut connection, args)
        }
        "ledger.category_report" => ledger_category_report(&connection, args),
        "ledger.report" => ledger_report(&connection, args),
        "invoice.upload_and_extract" | "invoice.extract_fields" => {
            invoice_upload_and_extract(core.data_dir(), &mut connection, args)
        }
        "invoice.list_drafts" => invoice_list_drafts(&connection, args),
        "invoice.post_to_ledger" => invoice_post_to_ledger(&mut connection, args),
        "actual.integration_status" => actual_integration_status(&connection, args),
        "actual.export_import_file" => {
            actual_export_import_file(core.data_dir(), &connection, args)
        }
        _ => unreachable!("tool whitelist and dispatch table must stay aligned"),
    }
}

impl BossCore {
    pub fn dispatch_tool(&self, name: &str, arguments: Value) -> BossResult<Value> {
        dispatch_tool(self, name, arguments)
    }
}

fn required_string(args: &Map<String, Value>, key: &str) -> BossResult<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| BossError::Validation(format!("缺少必填字符串：{key}")))
}

fn optional_string(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn integer_arg(args: &Map<String, Value>, key: &str, default: i64) -> BossResult<i64> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_i64()
            .ok_or_else(|| BossError::Validation(format!("{key} 必须是整数"))),
    }
}

fn bool_arg(args: &Map<String, Value>, key: &str) -> BossResult<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| BossError::Validation(format!("{key} 必须是布尔值"))),
    }
}

fn parse_date(value: &str, key: &str) -> BossResult<NaiveDate> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| BossError::Validation(format!("{key} 必须是有效 YYYY-MM-DD 日期")))
}

fn parse_month(value: &str) -> BossResult<String> {
    NaiveDate::parse_from_str(&format!("{value}-01"), "%Y-%m-%d")
        .map_err(|_| BossError::Validation("month 必须是有效 YYYY-MM".to_string()))?;
    if value.len() != 7 {
        return Err(BossError::Validation(
            "month 必须是有效 YYYY-MM".to_string(),
        ));
    }
    Ok(value.to_string())
}

fn current_month() -> String {
    Local::now().date_naive().format("%Y-%m").to_string()
}

fn current_week(reference: NaiveDate) -> (NaiveDate, NaiveDate) {
    let days = match reference.weekday() {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    };
    let start = reference - Duration::days(days);
    (start, start + Duration::days(6))
}

fn resolve_date_range(
    args: &Map<String, Value>,
    default_to_week: bool,
) -> BossResult<(NaiveDate, NaiveDate)> {
    let single = optional_string(args, "date")
        .map(|value| parse_date(&value, "date"))
        .transpose()?;
    let from = optional_string(args, "date_from")
        .map(|value| parse_date(&value, "date_from"))
        .transpose()?;
    let to = optional_string(args, "date_to")
        .map(|value| parse_date(&value, "date_to"))
        .transpose()?;
    let today = Local::now().date_naive();
    let (start, end) = if from.is_some() || to.is_some() {
        (from.or(to).unwrap(), to.or(from).unwrap())
    } else if let Some(date) = single {
        (date, date)
    } else if default_to_week {
        current_week(today)
    } else {
        (today, today)
    };
    if start > end {
        return Err(BossError::Validation(
            "date_from 不能晚于 date_to".to_string(),
        ));
    }
    Ok((start, end))
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn owner_exists(connection: &Connection, owner_code: &str) -> BossResult<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM owners WHERE owner_code = ?1)",
        [owner_code],
        |row| row.get(0),
    )?)
}

fn require_owner(connection: &Connection, owner_code: &str) -> BossResult<()> {
    if owner_exists(connection, owner_code)? {
        Ok(())
    } else {
        Err(BossError::NotFound("老板绑定码不存在".to_string()))
    }
}

fn employee_rows(
    connection: &Connection,
    owner_code: &str,
    range: Option<(NaiveDate, NaiveDate)>,
) -> BossResult<Vec<Value>> {
    let (event_filter, values): (&str, Vec<String>) = if let Some((start, end)) = range {
        (
            "AND event_date >= ?2 AND event_date <= ?3",
            vec![owner_code.to_string(), start.to_string(), end.to_string()],
        )
    } else {
        ("", vec![owner_code.to_string()])
    };
    let owner_parameter = if range.is_some() { "?4" } else { "?2" };
    let mut bind_values = values;
    bind_values.push(owner_code.to_string());
    let sql = format!(
        r#"
        WITH filtered_events AS (
            SELECT * FROM work_events WHERE owner_code = ?1 {event_filter}
        ), event_rollups AS (
            SELECT employee_id, COUNT(*) AS event_count,
                   COALESCE(SUM(material_count), 0) AS material_count,
                   COALESCE(SUM(cost_cents), 0) AS cost_cents,
                   COALESCE(AVG(quality_score), 0) AS work_quality
            FROM filtered_events GROUP BY employee_id
        ), ranked_events AS (
            SELECT employee_id, event_date, summary,
                   ROW_NUMBER() OVER (
                       PARTITION BY employee_id ORDER BY event_date DESC, id DESC
                   ) AS position
            FROM filtered_events
        )
        SELECT e.id, e.name, e.role, e.status, e.quality,
               COALESCE(r.event_count, 0), COALESCE(r.material_count, 0),
               COALESCE(r.cost_cents, 0), COALESCE(r.work_quality, e.quality),
               latest.event_date, latest.summary
        FROM employees e
        LEFT JOIN event_rollups r ON r.employee_id = e.id
        LEFT JOIN ranked_events latest
               ON latest.employee_id = e.id AND latest.position = 1
        WHERE e.owner_code = {owner_parameter}
        ORDER BY e.name
        "#
    );
    let refs = bind_values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(params_from_iter(refs), |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "role": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
                "quality": row.get::<_, i64>(4)?,
                "event_count": row.get::<_, i64>(5)?,
                "material_count": row.get::<_, i64>(6)?,
                "cost_cents": row.get::<_, i64>(7)?,
                "work_quality": row.get::<_, f64>(8)?,
                "last_event_date": row.get::<_, Option<String>>(9)?,
                "last_summary": row.get::<_, Option<String>>(10)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn work_event_rows(
    connection: &Connection,
    owner_code: &str,
    start: NaiveDate,
    end: NaiveDate,
    employee_id: Option<&str>,
) -> BossResult<Vec<Value>> {
    let mut sql = String::from(
        r#"
        SELECT w.id, w.employee_id, e.name, e.role, e.status, w.event_date,
               w.task_type, w.material_count, w.cost_cents, w.quality_score, w.summary
        FROM work_events w
        JOIN employees e ON e.id = w.employee_id AND e.owner_code = w.owner_code
        WHERE w.owner_code = ?1 AND w.event_date >= ?2 AND w.event_date <= ?3
        "#,
    );
    let mut values = vec![owner_code.to_string(), start.to_string(), end.to_string()];
    if let Some(employee_id) = employee_id {
        sql.push_str(" AND w.employee_id = ?4");
        values.push(employee_id.to_string());
    }
    sql.push_str(" ORDER BY w.event_date DESC, w.quality_score DESC, w.id DESC");
    let refs = values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    let mut statement = connection.prepare(&sql)?;
    let events = statement
        .query_map(params_from_iter(refs), |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "employee_id": row.get::<_, String>(1)?,
                "name": row.get::<_, String>(2)?,
                "role": row.get::<_, String>(3)?,
                "status": row.get::<_, String>(4)?,
                "event_date": row.get::<_, String>(5)?,
                "task_type": row.get::<_, String>(6)?,
                "material_count": row.get::<_, i64>(7)?,
                "cost_cents": row.get::<_, i64>(8)?,
                "quality_score": row.get::<_, i64>(9)?,
                "summary": row.get::<_, String>(10)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(events)
}

fn boss_dashboard_summary(connection: &Connection, args: &Map<String, Value>) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let (start, end) = resolve_date_range(args, false)?;
    let employees = employee_rows(connection, &owner_code, Some((start, end)))?;
    let (event_count, material_count, cost_cents, quality): (i64, i64, i64, f64) = connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(material_count), 0),
                    COALESCE(SUM(cost_cents), 0), COALESCE(AVG(quality_score), 0)
             FROM work_events
             WHERE owner_code = ?1 AND event_date >= ?2 AND event_date <= ?3",
            params![owner_code, start.to_string(), end.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
    let blocked_items = employees
        .iter()
        .filter(|employee| employee["status"] == "blocked")
        .map(|employee| {
            json!({
                "employee_id": employee["id"],
                "name": employee["name"],
                "summary": employee["last_summary"]
            })
        })
        .collect::<Vec<_>>();
    let active_count = employees
        .iter()
        .filter(|employee| employee["status"] == "active")
        .count();
    Ok(json!({
        "owner_code": owner_code,
        "date": end.to_string(),
        "date_from": start.to_string(),
        "date_to": end.to_string(),
        "employee_count": employees.len(),
        "active_count": active_count,
        "blocked_count": blocked_items.len(),
        "event_count": event_count,
        "material_count": material_count,
        "cost_cents": cost_cents,
        "average_quality": (quality * 10.0).round() / 10.0,
        "blocked_items": blocked_items,
    }))
}

fn boss_employee_work_report(
    connection: &Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let employee_id = optional_string(args, "employee_id");
    let (start, end) = resolve_date_range(args, false)?;
    let events = work_event_rows(connection, &owner_code, start, end, employee_id.as_deref())?;
    let material_count = events
        .iter()
        .filter_map(|event| event["material_count"].as_i64())
        .sum::<i64>();
    let cost_cents = events
        .iter()
        .filter_map(|event| event["cost_cents"].as_i64())
        .sum::<i64>();
    let quality = events
        .iter()
        .filter_map(|event| event["quality_score"].as_f64())
        .sum::<f64>();
    Ok(json!({
        "owner_code": owner_code,
        "date": end.to_string(),
        "date_from": start.to_string(),
        "date_to": end.to_string(),
        "employee_id": employee_id,
        "event_count": events.len(),
        "material_count": material_count,
        "cost_cents": cost_cents,
        "average_quality": if events.is_empty() { 0.0 } else {
            (quality / events.len() as f64 * 10.0).round() / 10.0
        },
        "events": events,
    }))
}

fn review_rows(
    connection: &Connection,
    owner_code: &str,
    employee_id: Option<&str>,
    start: Option<NaiveDate>,
    end: Option<NaiveDate>,
) -> BossResult<Vec<Value>> {
    let mut sql = String::from(
        "SELECT id, owner_code, employee_id, week_start, week_end, status, note,
                created_at, updated_at FROM week_reviews WHERE owner_code = ?1",
    );
    let mut values = vec![owner_code.to_string()];
    if let Some(employee_id) = employee_id {
        values.push(employee_id.to_string());
        sql.push_str(&format!(" AND employee_id = ?{}", values.len()));
    }
    if let Some(start) = start {
        values.push(start.to_string());
        sql.push_str(&format!(" AND week_end >= ?{}", values.len()));
    }
    if let Some(end) = end {
        values.push(end.to_string());
        sql.push_str(&format!(" AND week_start <= ?{}", values.len()));
    }
    sql.push_str(" ORDER BY week_start DESC, updated_at DESC");
    let refs = values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(params_from_iter(refs), |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "owner_code": row.get::<_, String>(1)?,
                "employee_id": row.get::<_, String>(2)?,
                "week_start": row.get::<_, String>(3)?,
                "week_end": row.get::<_, String>(4)?,
                "status": row.get::<_, String>(5)?,
                "note": row.get::<_, String>(6)?,
                "created_at": row.get::<_, String>(7)?,
                "updated_at": row.get::<_, String>(8)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn boss_save_week_review(
    connection: &mut Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let employee_id = required_string(args, "employee_id")?;
    let week_start = parse_date(&required_string(args, "week_start")?, "week_start")?;
    let week_end = parse_date(&required_string(args, "week_end")?, "week_end")?;
    if week_start > week_end || (week_end - week_start).num_days() > 6 {
        return Err(BossError::Validation(
            "week_start/week_end 必须构成不超过 7 天的周期".to_string(),
        ));
    }
    let status = required_string(args, "status")?;
    if !matches!(status.as_str(), "pending" | "reviewed" | "needs_supplement") {
        return Err(BossError::Validation(
            "status 必须是 pending、reviewed 或 needs_supplement".to_string(),
        ));
    }
    let note = optional_string(args, "note").unwrap_or_default();
    if status == "needs_supplement" && note.is_empty() {
        return Err(BossError::Validation("要求补充时必须填写 note".to_string()));
    }
    require_owner(connection, &owner_code)?;
    let employee_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM employees WHERE owner_code = ?1 AND id = ?2)",
        params![owner_code, employee_id],
        |row| row.get(0),
    )?;
    if !employee_exists {
        return Err(BossError::NotFound("员工不存在或未绑定".to_string()));
    }
    let now = now_iso();
    let id = new_id("review");
    connection.execute(
        r#"
        INSERT INTO week_reviews
            (id, owner_code, employee_id, week_start, week_end, status, note, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
        ON CONFLICT(owner_code, employee_id, week_start) DO UPDATE SET
            week_end = excluded.week_end,
            status = excluded.status,
            note = excluded.note,
            updated_at = excluded.updated_at
        "#,
        params![
            id,
            owner_code,
            employee_id,
            week_start.to_string(),
            week_end.to_string(),
            status,
            note,
            now
        ],
    )?;
    review_rows(
        connection,
        &owner_code,
        Some(&employee_id),
        Some(week_start),
        Some(week_start),
    )?
    .into_iter()
    .next()
    .ok_or_else(|| BossError::State("周审阅保存后无法读取".to_string()))
}

fn employee_list_week_reviews(
    connection: &Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let employee_id = required_string(args, "employee_id")?;
    let start = optional_string(args, "date_from")
        .or_else(|| optional_string(args, "week_start"))
        .map(|value| parse_date(&value, "date_from"))
        .transpose()?;
    let end = optional_string(args, "date_to")
        .or_else(|| optional_string(args, "week_end"))
        .map(|value| parse_date(&value, "date_to"))
        .transpose()?;
    let reviews = review_rows(connection, &owner_code, Some(&employee_id), start, end)?;
    Ok(json!({
        "owner_code": owner_code,
        "employee_id": employee_id,
        "reviews": reviews,
    }))
}

fn employee_report_work_event(
    connection: &mut Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    require_owner(connection, &owner_code)?;
    let event_id = optional_string(args, "event_id")
        .or_else(|| optional_string(args, "id"))
        .ok_or_else(|| BossError::Validation("缺少 event_id".to_string()))?;
    if event_id.len() > 160 {
        return Err(BossError::Validation("event_id 过长".to_string()));
    }
    let employee_id = required_string(args, "employee_id")?;
    let employee_name = required_string(args, "employee_name")?;
    let role = optional_string(args, "role").unwrap_or_else(|| "员工".to_string());
    let employee_status = optional_string(args, "employee_status")
        .or_else(|| optional_string(args, "status"))
        .unwrap_or_else(|| "active".to_string());
    if !matches!(employee_status.as_str(), "active" | "blocked" | "idle") {
        return Err(BossError::Validation(
            "employee_status 必须是 active、blocked 或 idle".to_string(),
        ));
    }
    let event_date = parse_date(
        &optional_string(args, "event_date")
            .unwrap_or_else(|| Local::now().date_naive().to_string()),
        "event_date",
    )?;
    let task_type = required_string(args, "task_type")?;
    let summary = required_string(args, "summary")?;
    let material_count = integer_arg(args, "material_count", 0)?;
    let cost_cents = integer_arg(args, "cost_cents", 0)?;
    let quality_score = integer_arg(args, "quality_score", 0)?;
    if material_count < 0 || cost_cents < 0 || !(0..=100).contains(&quality_score) {
        return Err(BossError::Validation(
            "material_count/cost_cents 不能为负，quality_score 必须在 0..100".to_string(),
        ));
    }
    let existing_owner: Option<String> = connection
        .query_row(
            "SELECT owner_code FROM work_events WHERE id = ?1",
            [&event_id],
            |row| row.get(0),
        )
        .optional()?;
    if existing_owner
        .as_deref()
        .is_some_and(|value| value != owner_code)
    {
        return Err(BossError::Conflict(
            "event_id 已被其他老板空间使用".to_string(),
        ));
    }
    let existing_employee_owner: Option<String> = connection
        .query_row(
            "SELECT owner_code FROM employees WHERE id = ?1",
            [&employee_id],
            |row| row.get(0),
        )
        .optional()?;
    if existing_employee_owner
        .as_deref()
        .is_some_and(|value| value != owner_code)
    {
        return Err(BossError::Conflict(
            "employee_id 已绑定到其他老板空间".to_string(),
        ));
    }
    let now = now_iso();
    let transaction = connection.transaction()?;
    transaction.execute(
        r#"
        INSERT INTO employees
            (id, owner_code, name, role, status, quality, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            role = excluded.role,
            status = excluded.status,
            quality = excluded.quality,
            updated_at = excluded.updated_at
        "#,
        params![
            employee_id,
            owner_code,
            employee_name,
            role,
            employee_status,
            quality_score,
            now
        ],
    )?;
    transaction.execute(
        r#"
        INSERT INTO work_events
            (id, owner_code, employee_id, event_date, task_type, material_count,
             cost_cents, quality_score, summary, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
        ON CONFLICT(id) DO UPDATE SET
            employee_id = excluded.employee_id,
            event_date = excluded.event_date,
            task_type = excluded.task_type,
            material_count = excluded.material_count,
            cost_cents = excluded.cost_cents,
            quality_score = excluded.quality_score,
            summary = excluded.summary,
            updated_at = excluded.updated_at
        "#,
        params![
            event_id,
            owner_code,
            employee_id,
            event_date.to_string(),
            task_type,
            material_count,
            cost_cents,
            quality_score,
            summary,
            now
        ],
    )?;
    transaction.commit()?;
    Ok(json!({
        "id": event_id,
        "event_id": event_id,
        "owner_code": owner_code,
        "employee_id": employee_id,
        "employee_name": employee_name,
        "event_date": event_date.to_string(),
        "task_type": task_type,
        "material_count": material_count,
        "cost_cents": cost_cents,
        "quality_score": quality_score,
        "summary": summary,
        "idempotent_key": format!("{}:{}", owner_code, event_id),
    }))
}

fn owner_create_binding_code(
    connection: &mut Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_id = required_string(args, "owner_id")?;
    let email = required_string(args, "email")?;
    if let Some(existing) = connection
        .query_row(
            "SELECT id, email, owner_code, created_at FROM owners WHERE id = ?1",
            [&owner_id],
            |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "email": row.get::<_, String>(1)?,
                    "owner_code": row.get::<_, String>(2)?,
                    "created_at": row.get::<_, String>(3)?,
                }))
            },
        )
        .optional()?
    {
        return Ok(existing);
    }
    let digest = Sha256::digest(format!("{owner_id}|{email}"));
    let suffix = u16::from_be_bytes([digest[0], digest[1]]) % 9000 + 1000;
    let owner_code = format!("BOSS-{suffix:04}");
    let now = now_iso();
    connection.execute(
        "INSERT INTO owners (id, email, owner_code, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![owner_id, email, owner_code, now],
    )?;
    Ok(json!({
        "id": owner_id,
        "email": email,
        "owner_code": owner_code,
        "created_at": now,
    }))
}

fn owner_list_bound_employees(
    connection: &Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let employees = employee_rows(connection, &owner_code, None)?;
    Ok(json!({
        "owner_code": owner_code,
        "employee_count": employees.len(),
        "employees": employees,
    }))
}

fn amount_to_cents(value: Option<&Value>) -> BossResult<i64> {
    let value = value.ok_or_else(|| BossError::Validation("缺少 amount".to_string()))?;
    let raw = match value {
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.trim().to_string(),
        _ => {
            return Err(BossError::Validation(
                "amount 必须是数字或十进制字符串".to_string(),
            ))
        }
    };
    let amount = Decimal::from_str(&raw)
        .map_err(|_| BossError::Validation("amount 必须是有效数字".to_string()))?;
    if amount <= Decimal::ZERO || amount.scale() > 2 {
        return Err(BossError::Validation(
            "amount 必须大于 0 且最多两位小数".to_string(),
        ));
    }
    (amount * Decimal::from(100))
        .to_i64()
        .filter(|cents| *cents > 0)
        .ok_or_else(|| BossError::Validation("amount 超出可记账范围".to_string()))
}

fn normalize_currency(args: &Map<String, Value>) -> BossResult<String> {
    let currency = optional_string(args, "currency")
        .unwrap_or_else(|| "CNY".to_string())
        .to_uppercase();
    if currency != "CNY" {
        return Err(BossError::Validation(
            "当前仅支持 CNY；配置币种映射后才能使用其他币种".to_string(),
        ));
    }
    Ok(currency)
}

fn transaction_from_row(row: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "owner_code": row.get::<_, String>(1)?,
        "tx_date": row.get::<_, String>(2)?,
        "type": row.get::<_, String>(3)?,
        "category": row.get::<_, String>(4)?,
        "counterparty": row.get::<_, String>(5)?,
        "amount_cents": row.get::<_, i64>(6)?,
        "currency": row.get::<_, String>(7)?,
        "source": row.get::<_, String>(8)?,
        "evidence_path": row.get::<_, String>(9)?,
        "status": row.get::<_, String>(10)?,
        "review_note": row.get::<_, String>(11)?,
        "created_at": row.get::<_, String>(12)?,
    }))
}

const TRANSACTION_COLUMNS: &str =
    "id, owner_code, tx_date, type, category, counterparty, amount_cents,
     currency, source, evidence_path, status, review_note, created_at";

fn ledger_add_transaction(
    connection: &mut Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    require_owner(connection, &owner_code)?;
    let kind = required_string(args, "type")?;
    if !matches!(kind.as_str(), "income" | "expense") {
        return Err(BossError::Validation(
            "type 必须是 income 或 expense".to_string(),
        ));
    }
    let category = required_string(args, "category")?;
    let cents = amount_to_cents(args.get("amount"))?;
    let currency = normalize_currency(args)?;
    let date = parse_date(
        &optional_string(args, "date").unwrap_or_else(|| Local::now().date_naive().to_string()),
        "date",
    )?;
    let status = optional_string(args, "status").unwrap_or_else(|| "needs_review".to_string());
    if !matches!(status.as_str(), "needs_review" | "posted") {
        return Err(BossError::Validation(
            "status 必须是 needs_review 或 posted".to_string(),
        ));
    }
    let counterparty = optional_string(args, "counterparty").unwrap_or_default();
    let source = optional_string(args, "source").unwrap_or_else(|| "agent_tool".to_string());
    let evidence_path = optional_string(args, "evidence_path").unwrap_or_default();
    let id = new_id("txn");
    let created_at = now_iso();
    connection.execute(
        r#"
        INSERT INTO ledger_transactions
            (id, owner_code, tx_date, type, category, counterparty, amount_cents,
             currency, source, evidence_path, status, review_note, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, '', ?12)
        "#,
        params![
            id,
            owner_code,
            date.to_string(),
            kind,
            category,
            counterparty,
            cents,
            currency,
            source,
            evidence_path,
            status,
            created_at
        ],
    )?;
    connection
        .query_row(
            &format!(
                "SELECT {TRANSACTION_COLUMNS} FROM ledger_transactions WHERE owner_code = ?1 AND id = ?2"
            ),
            params![owner_code, id],
            transaction_from_row,
        )
        .map_err(Into::into)
}

fn ledger_list_transactions(
    connection: &Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let month = optional_string(args, "month")
        .map(|month| parse_month(&month))
        .transpose()?;
    let status = optional_string(args, "status");
    if status
        .as_deref()
        .is_some_and(|value| !matches!(value, "needs_review" | "posted" | "synced"))
    {
        return Err(BossError::Validation("status 筛选值无效".to_string()));
    }
    let kind = optional_string(args, "type");
    if kind
        .as_deref()
        .is_some_and(|value| !matches!(value, "income" | "expense"))
    {
        return Err(BossError::Validation("type 筛选值无效".to_string()));
    }
    let limit = integer_arg(args, "limit", 50)?.clamp(1, 200);
    let offset = integer_arg(args, "offset", 0)?;
    if offset < 0 {
        return Err(BossError::Validation("offset 不能为负".to_string()));
    }
    let mut conditions = vec!["owner_code = ?1".to_string()];
    let mut values = vec![owner_code.clone()];
    for (column, value) in [
        ("substr(tx_date, 1, 7)", month.as_ref()),
        ("status", status.as_ref()),
        ("type", kind.as_ref()),
    ] {
        if let Some(value) = value {
            values.push(value.clone());
            conditions.push(format!("{column} = ?{}", values.len()));
        }
    }
    let where_sql = conditions.join(" AND ");
    let refs = values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    let total: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM ledger_transactions WHERE {where_sql}"),
        params_from_iter(refs.iter().copied()),
        |row| row.get(0),
    )?;
    let mut page_values = values;
    let limit_parameter = page_values.len() + 1;
    let offset_parameter = page_values.len() + 2;
    page_values.push(limit.to_string());
    page_values.push(offset.to_string());
    let page_refs = page_values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    let sql = format!(
        "SELECT {TRANSACTION_COLUMNS} FROM ledger_transactions
         WHERE {where_sql}
         ORDER BY tx_date DESC, created_at DESC, id DESC
         LIMIT ?{limit_parameter} OFFSET ?{offset_parameter}"
    );
    let mut statement = connection.prepare(&sql)?;
    let transactions = statement
        .query_map(params_from_iter(page_refs), transaction_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    let next_offset = offset + transactions.len() as i64;
    Ok(json!({
        "owner_code": owner_code,
        "filters": { "month": month, "status": status, "type": kind },
        "transactions": transactions,
        "pagination": {
            "offset": offset,
            "limit": limit,
            "total": total,
            "has_more": next_offset < total,
            "next_offset": next_offset,
        }
    }))
}

fn ledger_update_transaction_status(
    connection: &mut Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let transaction_id = required_string(args, "transaction_id")?;
    let status = required_string(args, "status")?;
    if !matches!(status.as_str(), "needs_review" | "posted") {
        return Err(BossError::Validation(
            "status 必须是 needs_review 或 posted".to_string(),
        ));
    }
    let existing: Option<(String, String)> = connection
        .query_row(
            "SELECT status, source FROM ledger_transactions WHERE owner_code = ?1 AND id = ?2",
            params![owner_code, transaction_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (previous_status, source) =
        existing.ok_or_else(|| BossError::NotFound("流水不存在".to_string()))?;
    if previous_status == "synced" {
        return Err(BossError::Conflict(
            "已同步流水不可修改，请在 Actual 中对账".to_string(),
        ));
    }
    let review_note = optional_string(args, "review_note").unwrap_or_default();
    let transaction = connection.transaction()?;
    transaction.execute(
        "UPDATE ledger_transactions SET status = ?1, review_note = ?2
         WHERE owner_code = ?3 AND id = ?4",
        params![status, review_note, owner_code, transaction_id],
    )?;
    if let Some(invoice_id) = source.strip_prefix("invoice:") {
        let invoice_status = if status == "posted" {
            "posted_to_ledger"
        } else {
            "ledger_draft"
        };
        transaction.execute(
            "UPDATE invoice_drafts SET status = ?1 WHERE owner_code = ?2 AND id = ?3",
            params![invoice_status, owner_code, invoice_id],
        )?;
    }
    transaction.commit()?;
    connection
        .query_row(
            &format!(
                "SELECT {TRANSACTION_COLUMNS} FROM ledger_transactions WHERE owner_code = ?1 AND id = ?2"
            ),
            params![owner_code, transaction_id],
            transaction_from_row,
        )
        .map_err(Into::into)
}

fn ledger_report(connection: &Connection, args: &Map<String, Value>) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let month = parse_month(&optional_string(args, "month").unwrap_or_else(current_month))?;
    let mut statement = connection.prepare(
        r#"
        SELECT type, category, UPPER(currency), status,
               COALESCE(SUM(amount_cents), 0), COUNT(*)
        FROM ledger_transactions
        WHERE owner_code = ?1 AND substr(tx_date, 1, 7) = ?2
        GROUP BY type, category, UPPER(currency), status
        ORDER BY type, SUM(amount_cents) DESC
        "#,
    )?;
    let rows = statement
        .query_map(params![owner_code, month], |row| {
            Ok(json!({
                "type": row.get::<_, String>(0)?,
                "category": row.get::<_, String>(1)?,
                "currency": row.get::<_, String>(2)?,
                "status": row.get::<_, String>(3)?,
                "amount_cents": row.get::<_, i64>(4)?,
                "count": row.get::<_, i64>(5)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut income_cents = 0_i64;
    let mut expense_cents = 0_i64;
    let mut needs_review_count = 0_i64;
    let mut pending_review_cents = 0_i64;
    let mut expense_by_category = BTreeMap::<String, i64>::new();
    for row in &rows {
        if row["currency"] != "CNY" {
            continue;
        }
        let cents = row["amount_cents"].as_i64().unwrap_or_default();
        let count = row["count"].as_i64().unwrap_or_default();
        if row["status"] == "needs_review" {
            needs_review_count += count;
            pending_review_cents += cents;
        } else if row["type"] == "income" {
            income_cents += cents;
        } else if row["type"] == "expense" {
            expense_cents += cents;
            if let Some(category) = row["category"].as_str() {
                *expense_by_category.entry(category.to_string()).or_default() += cents;
            }
        }
    }
    Ok(json!({
        "owner_code": owner_code,
        "month": month,
        "income_cents": income_cents,
        "expense_cents": expense_cents,
        "net_cents": income_cents - expense_cents,
        "currency": "CNY",
        "needs_review_count": needs_review_count,
        "pending_review_cents": pending_review_cents,
        "expense_by_category_cents": expense_by_category,
        "rows": rows,
    }))
}

fn ledger_category_report(connection: &Connection, args: &Map<String, Value>) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let month = parse_month(&optional_string(args, "month").unwrap_or_else(current_month))?;
    let mut statement = connection.prepare(
        r#"
        SELECT category,
               COALESCE(SUM(CASE WHEN type = 'income' AND status <> 'needs_review'
                            THEN amount_cents ELSE 0 END), 0),
               COALESCE(SUM(CASE WHEN type = 'expense' AND status <> 'needs_review'
                            THEN amount_cents ELSE 0 END), 0),
               COUNT(*),
               COALESCE(SUM(CASE WHEN status = 'needs_review' THEN 1 ELSE 0 END), 0)
        FROM ledger_transactions
        WHERE owner_code = ?1 AND substr(tx_date, 1, 7) = ?2 AND currency = 'CNY'
        GROUP BY category
        ORDER BY 3 DESC, 2 DESC
        "#,
    )?;
    let categories = statement
        .query_map(params![owner_code, month], |row| {
            Ok(json!({
                "category": row.get::<_, String>(0)?,
                "income_cents": row.get::<_, i64>(1)?,
                "expense_cents": row.get::<_, i64>(2)?,
                "count": row.get::<_, i64>(3)?,
                "needs_review_count": row.get::<_, i64>(4)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "owner_code": owner_code,
        "month": month,
        "categories": categories,
    }))
}

fn safe_upload_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .take(120)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches(['.', '-'])
        .to_string();
    if cleaned.is_empty() {
        "invoice-upload.bin".to_string()
    } else {
        cleaned
    }
}

fn normalize_invoice_date(value: &str) -> Option<String> {
    let regex = Regex::new(r"^([0-9]{4})[-/.年]([0-9]{1,2})[-/.月]([0-9]{1,2})(?:日)?$")
        .expect("invoice date regex is valid");
    let captures = regex.captures(value.trim())?;
    let year = captures.get(1)?.as_str().parse().ok()?;
    let month = captures.get(2)?.as_str().parse().ok()?;
    let day = captures.get(3)?.as_str().parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day).map(|date| date.to_string())
}

fn decimal_json(value: &str) -> Option<Value> {
    let decimal = Decimal::from_str(value).ok()?;
    if decimal < Decimal::ZERO || decimal.scale() > 2 {
        return None;
    }
    serde_json::Number::from_f64(decimal.to_f64()?).map(Value::Number)
}

fn ai_field(ai: Option<&Map<String, Value>>, key: &str) -> Option<Value> {
    ai.and_then(|fields| fields.get(key))
        .filter(|value| !value.is_null() && value.as_str() != Some(""))
        .cloned()
}

fn capture_value(regex: &Regex, text: &str) -> Option<String> {
    regex
        .captures(text)
        .and_then(|capture| capture.get(1))
        .map(|value| value.as_str().trim().to_string())
}

fn parse_invoice_fields(text: &str, ai: Option<&Map<String, Value>>) -> Value {
    let mut fields = Map::new();
    for key in [
        "invoice_number",
        "invoice_code",
        "invoice_date",
        "seller",
        "buyer",
        "amount",
        "tax",
        "total",
        "category_suggestion",
    ] {
        fields.insert(key.to_string(), ai_field(ai, key).unwrap_or(Value::Null));
    }
    fields.insert("currency".to_string(), json!("CNY"));
    fields.insert(
        "confidence".to_string(),
        ai_field(ai, "confidence").unwrap_or_else(|| json!(0.0)),
    );

    let patterns = [
        (
            "invoice_number",
            r"(?i)(?:发票号码|invoice\s*no\.?|number)[:：\s]*([A-Za-z0-9-]{6,})",
        ),
        (
            "invoice_code",
            r"(?i)(?:发票代码|invoice\s*code)[:：\s]*([A-Za-z0-9-]{6,})",
        ),
        (
            "invoice_date",
            r"(?i)(?:开票日期|发票日期|date)[:：\s]*([0-9]{4}[-/.年][0-9]{1,2}[-/.月][0-9]{1,2}(?:日)?)",
        ),
        (
            "amount",
            r"(?i)(?:不含税金额|金额|amount)[:：\s]*(?:CNY|RMB|￥)?\s*([0-9]+(?:\.[0-9]{1,2})?)",
        ),
        (
            "tax",
            r"(?i)(?:税额|tax)[:：\s]*(?:CNY|RMB|￥)?\s*([0-9]+(?:\.[0-9]{1,2})?)",
        ),
        (
            "total",
            r"(?i)(?:价税合计|总金额|合计金额|total)[:：\s]*(?:CNY|RMB|￥)?\s*([0-9]+(?:\.[0-9]{1,2})?)",
        ),
        ("seller", r"(?im)(?:销售方|销方|seller)[:：\s]*([^\r\n]+)"),
        ("buyer", r"(?im)(?:购买方|购方|buyer)[:：\s]*([^\r\n]+)"),
    ];
    let mut detected = 0_u32;
    for (key, pattern) in patterns {
        let regex = Regex::new(pattern).expect("literal invoice regex is valid");
        let Some(value) = capture_value(&regex, text) else {
            continue;
        };
        let parsed = match key {
            "invoice_date" => normalize_invoice_date(&value).map(Value::String),
            "amount" | "tax" | "total" => decimal_json(&value),
            "seller" | "buyer" => Some(Value::String(value.chars().take(120).collect())),
            _ => Some(Value::String(value)),
        };
        if let Some(value) = parsed {
            fields.insert(key.to_string(), value);
            detected += 1;
        }
    }
    if fields.get("total").is_none_or(Value::is_null) {
        if let Some(amount) = fields
            .get("amount")
            .filter(|value| !value.is_null())
            .cloned()
        {
            fields.insert("total".to_string(), amount);
        }
    }
    let upper = text.to_uppercase();
    let category = if text.contains("样品") || text.contains("达人") {
        Some("达人样品")
    } else if text.contains("工具") || upper.contains("API") {
        Some("AI 工具")
    } else if text.contains("素材") || text.contains("采集") {
        Some("素材采集")
    } else {
        None
    };
    if let Some(category) = category {
        fields.insert("category_suggestion".to_string(), json!(category));
    }
    let prior_confidence = fields
        .get("confidence")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if detected > 0 {
        fields.insert(
            "confidence".to_string(),
            json!(prior_confidence.max((0.35 + detected as f64 * 0.08).min(0.95))),
        );
    }
    if let Some(date) = fields
        .get("invoice_date")
        .and_then(Value::as_str)
        .and_then(normalize_invoice_date)
    {
        fields.insert("invoice_date".to_string(), json!(date));
    } else if !fields.get("invoice_date").is_none_or(Value::is_null) {
        fields.insert("invoice_date".to_string(), Value::Null);
    }
    Value::Object(fields)
}

struct InvoiceInput {
    text: String,
    file_path: String,
    source_status: String,
    content_hash: String,
}

fn invoice_input(data_dir: &Path, args: &Map<String, Value>) -> BossResult<InvoiceInput> {
    let provided_text = optional_string(args, "extracted_text");
    let ai = args.get("ai_ocr_json").and_then(Value::as_object);
    let mut file_path = optional_string(args, "file_path").unwrap_or_default();
    let mut raw_content: Option<Vec<u8>> = None;
    if let Some(encoded) = optional_string(args, "content_base64") {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|_| BossError::Validation("content_base64 不是有效 Base64".to_string()))?;
        if bytes.len() > 20 * 1024 * 1024 {
            return Err(BossError::Validation("票据文件超过 20MB 上限".to_string()));
        }
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let name = safe_upload_name(
            &optional_string(args, "file_name").unwrap_or_else(|| "invoice-upload.bin".to_string()),
        );
        let upload_dir = data_dir.join("invoice_uploads");
        fs::create_dir_all(&upload_dir)?;
        let path = upload_dir.join(format!("{}-{name}", &digest[..16]));
        if !path.exists() {
            fs::write(&path, &bytes)?;
        }
        file_path = path.to_string_lossy().into_owned();
        raw_content = Some(bytes);
    }

    if raw_content.is_none() && !file_path.is_empty() {
        let requested = PathBuf::from(&file_path);
        let canonical = requested
            .canonicalize()
            .map_err(|_| BossError::NotFound("票据文件不存在".to_string()))?;
        let trusted_root = data_dir
            .canonicalize()
            .unwrap_or_else(|_| data_dir.to_path_buf());
        let allow_external = matches!(
            std::env::var("BOSS_ALLOW_EXTERNAL_INVOICE_PATH").as_deref(),
            Ok("1" | "true" | "TRUE")
        );
        if !allow_external && !canonical.starts_with(&trusted_root) {
            return Err(BossError::Validation(
                "file_path 仅允许访问老板应用数据目录；请使用 content_base64 上传".to_string(),
            ));
        }
        file_path = canonical.to_string_lossy().into_owned();
    }

    let mut source_status = "needs_file".to_string();
    let text = if let Some(text) = provided_text {
        source_status = "provided_text".to_string();
        text
    } else if !file_path.is_empty() {
        let path = PathBuf::from(&file_path);
        if !path.is_file() {
            source_status = "file_not_found".to_string();
            String::new()
        } else {
            let metadata = fs::metadata(&path)?;
            if metadata.len() > 20 * 1024 * 1024 {
                return Err(BossError::Validation("票据文件超过 20MB 上限".to_string()));
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            if matches!(extension.as_str(), "txt" | "csv" | "json" | "md") {
                source_status = "text_file".to_string();
                fs::read_to_string(path)?
            } else {
                source_status = "needs_ai_ocr_adapter".to_string();
                String::new()
            }
        }
    } else {
        String::new()
    };
    if ai.is_some() && text.is_empty() {
        source_status = "provided_ai_ocr".to_string();
    }
    let mut hasher = Sha256::new();
    let mut has_content = false;
    if let Some(bytes) = raw_content {
        hasher.update(bytes);
        has_content = true;
    } else if !text.is_empty() {
        hasher.update(text.as_bytes());
        has_content = true;
    } else if !file_path.is_empty() && Path::new(&file_path).is_file() {
        hasher.update(fs::read(&file_path)?);
        has_content = true;
    }
    if let Some(ai) = ai {
        hasher.update(serde_json::to_vec(ai)?);
        has_content = true;
    }
    Ok(InvoiceInput {
        text,
        file_path,
        source_status,
        content_hash: if has_content {
            format!("{:x}", hasher.finalize())
        } else {
            String::new()
        },
    })
}

fn invoice_business_key(fields: &Value) -> String {
    let number = fields["invoice_number"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_uppercase();
    if number.is_empty() {
        return String::new();
    }
    let identity = format!(
        "{}|{}|{}|{}|{}",
        fields["invoice_code"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_uppercase(),
        number,
        fields["seller"].as_str().unwrap_or_default().trim(),
        fields["invoice_date"].as_str().unwrap_or_default().trim(),
        fields["total"]
    );
    format!("{:x}", Sha256::digest(identity.as_bytes()))
}

fn invoice_from_row(row: &Row<'_>) -> rusqlite::Result<Value> {
    let fields_json: String = row.get(4)?;
    let fields: Value = serde_json::from_str(&fields_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(4, Type::Text, Box::new(error))
    })?;
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "owner_code": row.get::<_, String>(1)?,
        "file_path": row.get::<_, String>(2)?,
        "source_status": row.get::<_, String>(3)?,
        "fields": fields,
        "status": row.get::<_, String>(5)?,
        "created_at": row.get::<_, String>(6)?,
    }))
}

fn invoice_upload_and_extract(
    data_dir: &Path,
    connection: &mut Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    require_owner(connection, &owner_code)?;
    let input = invoice_input(data_dir, args)?;
    let ai = args.get("ai_ocr_json").and_then(Value::as_object);
    let fields = parse_invoice_fields(&input.text, ai);
    let business_key = invoice_business_key(&fields);
    let status = if matches!(
        input.source_status.as_str(),
        "needs_file" | "file_not_found" | "needs_ai_ocr_adapter"
    ) {
        input.source_status.clone()
    } else if !fields["total"].is_null() || !fields["amount"].is_null() {
        "extracted".to_string()
    } else {
        "needs_review".to_string()
    };
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let existing = transaction
        .query_row(
            r#"
            SELECT id, owner_code, file_path, 'duplicate_upload', fields_json, status, created_at
            FROM invoice_drafts
            WHERE owner_code = ?1
              AND ((?2 <> '' AND content_hash = ?2) OR (?3 <> '' AND business_key = ?3))
            ORDER BY created_at ASC LIMIT 1
            "#,
            params![owner_code, input.content_hash, business_key],
            invoice_from_row,
        )
        .optional()?;
    if let Some(mut existing) = existing {
        transaction.commit()?;
        existing["idempotent"] = json!(true);
        existing["policy"] = json!("未知票据字段必须保持 null，直到 OCR/工具验证");
        return Ok(existing);
    }
    let id = new_id("inv");
    let created_at = now_iso();
    transaction.execute(
        r#"
        INSERT INTO invoice_drafts
            (id, owner_code, file_path, content_hash, business_key, fields_json, status, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            id,
            owner_code,
            input.file_path,
            input.content_hash,
            business_key,
            serde_json::to_string(&fields)?,
            status,
            created_at
        ],
    )?;
    transaction.commit()?;
    Ok(json!({
        "id": id,
        "owner_code": owner_code,
        "file_path": input.file_path,
        "source_status": input.source_status,
        "status": status,
        "fields": fields,
        "idempotent": false,
        "policy": "未知票据字段必须保持 null，直到 OCR/工具验证",
    }))
}

fn invoice_list_drafts(connection: &Connection, args: &Map<String, Value>) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let status = optional_string(args, "status");
    let limit = integer_arg(args, "limit", 50)?.clamp(1, 200);
    let (sql, mut values) = if let Some(status) = &status {
        (
            "SELECT id, owner_code, file_path, 'stored', fields_json, status, created_at
             FROM invoice_drafts WHERE owner_code = ?1 AND status = ?2
             ORDER BY created_at DESC LIMIT ?3",
            vec![owner_code.clone(), status.clone()],
        )
    } else {
        (
            "SELECT id, owner_code, file_path, 'stored', fields_json, status, created_at
             FROM invoice_drafts WHERE owner_code = ?1
             ORDER BY created_at DESC LIMIT ?2",
            vec![owner_code.clone()],
        )
    };
    values.push(limit.to_string());
    let refs = values
        .iter()
        .map(|value| value as &dyn ToSql)
        .collect::<Vec<_>>();
    let mut statement = connection.prepare(sql)?;
    let drafts = statement
        .query_map(params_from_iter(refs), invoice_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "owner_code": owner_code,
        "drafts": drafts,
    }))
}

fn value_as_amount(value: &Value) -> Option<Value> {
    if value.as_f64().is_some() {
        Some(value.clone())
    } else {
        value.as_str().and_then(decimal_json)
    }
}

fn invoice_post_to_ledger(
    connection: &mut Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let invoice_id = required_string(args, "invoice_id")?;
    let category = required_string(args, "category")?;
    let kind = optional_string(args, "type").unwrap_or_else(|| "expense".to_string());
    let status = optional_string(args, "status").unwrap_or_else(|| "needs_review".to_string());
    if !matches!(kind.as_str(), "income" | "expense") {
        return Err(BossError::Validation(
            "type 必须是 income 或 expense".to_string(),
        ));
    }
    if status != "needs_review" {
        return Err(BossError::Validation(
            "票据只能生成 needs_review 流水；请通过 ledger.update_transaction_status 人工审批"
                .to_string(),
        ));
    }
    let source = format!("invoice:{invoice_id}");
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let invoice: Option<(String, String)> = transaction
        .query_row(
            "SELECT fields_json, file_path FROM invoice_drafts
             WHERE owner_code = ?1 AND id = ?2",
            params![owner_code, invoice_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (fields_json, evidence_path) =
        invoice.ok_or_else(|| BossError::NotFound("票据草稿不存在".to_string()))?;
    let fields: Value = serde_json::from_str(&fields_json)?;
    let existing = transaction
        .query_row(
            &format!(
                "SELECT {TRANSACTION_COLUMNS} FROM ledger_transactions
                 WHERE owner_code = ?1 AND source = ?2 ORDER BY created_at ASC LIMIT 1"
            ),
            params![owner_code, source],
            transaction_from_row,
        )
        .optional()?;
    if let Some(existing) = existing {
        let invoice_status = if matches!(existing["status"].as_str(), Some("posted" | "synced")) {
            "posted_to_ledger"
        } else {
            "ledger_draft"
        };
        transaction.execute(
            "UPDATE invoice_drafts SET status = ?1 WHERE owner_code = ?2 AND id = ?3",
            params![invoice_status, owner_code, invoice_id],
        )?;
        transaction.commit()?;
        return Ok(json!({
            "invoice_id": invoice_id,
            "transaction": existing,
            "fields": fields,
            "idempotent": true,
        }));
    }
    let amount = value_as_amount(&fields["total"])
        .or_else(|| value_as_amount(&fields["amount"]))
        .ok_or_else(|| BossError::Validation("票据没有已验证金额".to_string()))?;
    let amount_cents = amount_to_cents(Some(&amount))?;
    let currency = fields["currency"].as_str().unwrap_or("CNY").to_uppercase();
    if currency != "CNY" {
        return Err(BossError::Validation("当前仅支持 CNY 票据".to_string()));
    }
    let counterparty = fields["seller"]
        .as_str()
        .or_else(|| fields["buyer"].as_str())
        .unwrap_or("发票对方");
    let date = optional_string(args, "date")
        .map(|value| parse_date(&value, "date").map(|date| date.to_string()))
        .transpose()?
        .or_else(|| {
            fields["invoice_date"]
                .as_str()
                .and_then(normalize_invoice_date)
        })
        .ok_or_else(|| {
            BossError::Validation("票据缺少已验证日期；请在 args.date 提供 YYYY-MM-DD".to_string())
        })?;
    let id = new_id("txn");
    let created_at = now_iso();
    transaction.execute(
        r#"
        INSERT INTO ledger_transactions
            (id, owner_code, tx_date, type, category, counterparty, amount_cents,
             currency, source, evidence_path, status, review_note, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'CNY', ?8, ?9, ?10, '', ?11)
        "#,
        params![
            id,
            owner_code,
            date,
            kind,
            category,
            counterparty,
            amount_cents,
            source,
            evidence_path,
            status,
            created_at
        ],
    )?;
    let ledger_transaction = transaction.query_row(
        &format!(
            "SELECT {TRANSACTION_COLUMNS} FROM ledger_transactions WHERE owner_code = ?1 AND id = ?2"
        ),
        params![owner_code, id],
        transaction_from_row,
    )?;
    transaction.execute(
        "UPDATE invoice_drafts SET status = ?1 WHERE owner_code = ?2 AND id = ?3",
        params!["ledger_draft", owner_code, invoice_id],
    )?;
    transaction.commit()?;
    Ok(json!({
        "invoice_id": invoice_id,
        "transaction": ledger_transaction,
        "fields": fields,
        "idempotent": false,
    }))
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    let configured = std::env::var_os("ACTUAL_CLI_BIN").filter(|value| !value.is_empty());
    if let Some(configured) = configured {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Some(path);
        }
        if path.components().count() > 1 {
            return None;
        }
        return find_named_executable(path.to_string_lossy().as_ref());
    }
    find_named_executable(name)
}

fn find_named_executable(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    find_named_executable_in(
        name,
        &paths,
        cfg!(windows),
        std::env::var("PATHEXT").ok().as_deref(),
    )
}

fn executable_file_names(name: &str, windows: bool, pathext: Option<&str>) -> Vec<OsString> {
    let mut names = vec![OsString::from(name)];
    if !windows || Path::new(name).extension().is_some() {
        return names;
    }
    let extensions = [".exe", ".cmd", ".bat"]
        .into_iter()
        .chain(pathext.unwrap_or(".COM;.EXE;.BAT;.CMD").split(';'));
    for extension in extensions {
        let trimmed = extension.trim();
        if trimmed.is_empty() {
            continue;
        }
        let extension = if trimmed.starts_with('.') {
            trimmed.to_string()
        } else {
            format!(".{trimmed}")
        };
        let candidate = OsString::from(format!("{name}{extension}"));
        if !names.iter().any(|existing| {
            existing
                .to_string_lossy()
                .eq_ignore_ascii_case(&candidate.to_string_lossy())
        }) {
            names.push(candidate);
        }
    }
    names
}

fn find_named_executable_in(
    name: &str,
    paths: &[PathBuf],
    windows: bool,
    pathext: Option<&str>,
) -> Option<PathBuf> {
    let names = executable_file_names(name, windows, pathext);
    paths
        .iter()
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}

fn package_version(package_json: &Path, package_name: &str) -> Option<String> {
    let payload: Value = serde_json::from_slice(&fs::read(package_json).ok()?).ok()?;
    payload
        .get("dependencies")
        .and_then(|value| value.get(package_name))
        .or_else(|| {
            payload
                .get("devDependencies")
                .and_then(|value| value.get(package_name))
        })
        .and_then(Value::as_str)
        .map(|value| value.trim_start_matches(['^', '~']).to_string())
}

fn actual_metadata() -> Value {
    let default_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../third_party/actual");
    let root = std::env::var_os("BOSS_ACTUAL_ROOT")
        .map(PathBuf::from)
        .unwrap_or(default_root);
    let cli = executable_on_path("actual");
    let cli_is_script = cli
        .as_ref()
        .and_then(|path| path.extension())
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "js" | "mjs" | "cjs"
            )
        });
    let cli_runnable = cli.is_some() && (!cli_is_script || find_named_executable("node").is_some());
    let server_url = std::env::var("ACTUAL_SERVER_URL")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let sync_id = std::env::var("ACTUAL_SYNC_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let credential = std::env::var("ACTUAL_PASSWORD")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("ACTUAL_TOKEN")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });
    let account = std::env::var("ACTUAL_DEFAULT_ACCOUNT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let api_version = package_version(&root.join("package.json"), "@actual-app/api");
    let cli_version = package_version(&root.join("package.json"), "@actual-app/sync-server");
    let ready = cli_runnable && server_url.is_some() && sync_id.is_some() && credential.is_some();
    json!({
        "repository": "https://github.com/actualbudget/actual",
        "local_path": root.to_string_lossy(),
        "source_present": root.is_dir(),
        "api_package": "@actual-app/api",
        "api_version": api_version,
        "cli_package": "actual",
        "cli_version": cli_version,
        "package_manager": "yarn",
        "cli_available": cli_runnable,
        "cli_bin": cli.map(|path| path.to_string_lossy().into_owned()),
        "server_url_configured": server_url.is_some(),
        "sync_id_configured": sync_id.is_some(),
        "credential_configured": credential.is_some(),
        "default_account_id_configured": account.is_some(),
        "ready_for_direct_cli_sync": ready,
        "required_env": [
            "ACTUAL_CLI_BIN", "ACTUAL_SERVER_URL", "ACTUAL_SYNC_ID",
            "ACTUAL_PASSWORD or ACTUAL_TOKEN", "ACTUAL_DEFAULT_ACCOUNT_ID"
        ],
        "capabilities": ["ledger", "transactions", "categories", "reports", "query", "sync"],
    })
}

fn actual_payload(transaction: &Value) -> Value {
    let cents = transaction["amount_cents"].as_i64().unwrap_or_default();
    let amount = if transaction["type"] == "expense" {
        -cents
    } else {
        cents
    };
    let category = transaction["category"].as_str().unwrap_or_default();
    let counterparty = transaction["counterparty"]
        .as_str()
        .filter(|value| !value.is_empty())
        .unwrap_or(category);
    json!({
        "date": transaction["tx_date"],
        "amount": amount,
        "payee_name": counterparty,
        "imported_id": transaction["id"],
        "notes": format!(
            "{} | boss:{} | category:{}",
            transaction["source"].as_str().unwrap_or_default(),
            transaction["id"].as_str().unwrap_or_default(),
            category
        ),
        "cleared": matches!(transaction["status"].as_str(), Some("posted" | "synced")),
    })
}

fn tail_text(bytes: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let tail = text.chars().rev().take(limit).collect::<String>();
    tail.chars().rev().collect()
}

fn cents_as_decimal(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let absolute = i128::from(cents).abs();
    format!("{sign}{}.{:02}", absolute / 100, absolute % 100)
}

fn record_actual_job(
    connection: &Connection,
    owner_code: &str,
    transaction_id: &str,
    status: &str,
    request: &Value,
    response: &Value,
) -> BossResult<Value> {
    let id = new_id("actual-job");
    let created_at = now_iso();
    connection.execute(
        r#"
        INSERT INTO actual_sync_jobs
            (id, owner_code, transaction_id, status, request_json, response_json, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
        params![
            id,
            owner_code,
            transaction_id,
            status,
            serde_json::to_string(request)?,
            serde_json::to_string(response)?,
            created_at
        ],
    )?;
    Ok(json!({
        "id": id,
        "owner_code": owner_code,
        "transaction_id": transaction_id,
        "status": status,
        "request_json": serde_json::to_string(request)?,
        "response_json": serde_json::to_string(response)?,
        "created_at": created_at,
    }))
}

enum ActualSyncStage {
    Finished(Value),
    Run {
        owner_code: String,
        transaction_id: String,
        account_id: String,
        cli: PathBuf,
        job: Value,
        request: Value,
        payload: Value,
    },
}

fn prepare_actual_sync(core: &BossCore, args: &Map<String, Value>) -> BossResult<ActualSyncStage> {
    let owner_code = required_string(args, "owner_code")?;
    let transaction_id = required_string(args, "transaction_id")?;
    let dry_run = bool_arg(args, "dry_run")?;
    let mut connection = core.lock()?;
    let transaction = connection
        .query_row(
            &format!(
                "SELECT {TRANSACTION_COLUMNS} FROM ledger_transactions WHERE owner_code = ?1 AND id = ?2"
            ),
            params![owner_code, transaction_id],
            transaction_from_row,
        )
        .optional()?
        .ok_or_else(|| BossError::NotFound("流水不存在".to_string()))?;
    if transaction["currency"] != "CNY" {
        return Err(BossError::Validation(
            "仅 CNY 流水可以同步 Actual".to_string(),
        ));
    }
    if transaction["status"] == "needs_review" {
        return Err(BossError::Conflict(
            "流水必须先经老板审批后才能同步 Actual".to_string(),
        ));
    }
    if transaction["status"] == "synced" {
        return Ok(ActualSyncStage::Finished(json!({
            "status": "synced",
            "transaction_id": transaction_id,
            "idempotent": true,
            "reason": "transaction is already synced",
        })));
    }
    let payload = actual_payload(&transaction);
    let metadata = actual_metadata();
    let account_id = optional_string(args, "actual_account_id")
        .or_else(|| std::env::var("ACTUAL_DEFAULT_ACCOUNT_ID").ok())
        .unwrap_or_default();
    let request = json!({
        "transaction_id": transaction_id,
        "actual_account_id": account_id,
        "payload": payload,
        "dry_run": dry_run,
    });
    if dry_run {
        let response = json!({
            "status": "dry_run",
            "actual_payload": payload,
            "actual": metadata,
        });
        let job = record_actual_job(
            &connection,
            &owner_code,
            &transaction_id,
            "dry_run",
            &request,
            &response,
        )?;
        return Ok(ActualSyncStage::Finished(json!({
            "job": job,
            "status": "dry_run",
            "actual_payload": payload,
            "actual": metadata,
        })));
    }
    let ready = metadata["ready_for_direct_cli_sync"]
        .as_bool()
        .unwrap_or(false);
    let cli = metadata["cli_bin"].as_str().map(PathBuf::from);
    if !ready || account_id.is_empty() || cli.is_none() {
        let db_transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<Value> = db_transaction
            .query_row(
                "SELECT id, owner_code, transaction_id, status, request_json, response_json, created_at
                 FROM actual_sync_jobs WHERE owner_code = ?1 AND transaction_id = ?2
                   AND status = 'queued' ORDER BY created_at DESC LIMIT 1",
                params![owner_code, transaction_id],
                actual_job_from_row,
            )
            .optional()?;
        if let Some(existing) = existing {
            let response: Value =
                serde_json::from_str(existing["response_json"].as_str().unwrap_or("{}"))?;
            db_transaction.commit()?;
            return Ok(ActualSyncStage::Finished(json!({
                "job": existing,
                "status": response["status"],
                "reason": response["reason"],
                "actual_payload": payload,
                "actual": metadata,
                "idempotent": true,
            })));
        }
        let response = json!({
            "status": "queued_needs_actual_credentials",
            "reason": "Configure Actual CLI, server, sync id, credential and account id before direct sync.",
            "actual_payload": payload,
            "actual": metadata,
        });
        let job = record_actual_job(
            &db_transaction,
            &owner_code,
            &transaction_id,
            "queued",
            &request,
            &response,
        )?;
        db_transaction.commit()?;
        return Ok(ActualSyncStage::Finished(json!({
            "job": job,
            "status": "queued_needs_actual_credentials",
            "reason": response["reason"],
            "actual_payload": payload,
            "actual": metadata,
        })));
    }
    let running_response = json!({
        "status": "sync_in_progress",
        "reason": "Actual import is running",
        "actual_payload": payload,
    });
    let db_transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let latest_status: Option<String> = db_transaction
        .query_row(
            "SELECT status FROM ledger_transactions WHERE owner_code = ?1 AND id = ?2",
            params![owner_code, transaction_id],
            |row| row.get(0),
        )
        .optional()?;
    match latest_status.as_deref() {
        None => return Err(BossError::NotFound("流水不存在".to_string())),
        Some("needs_review") => {
            return Err(BossError::Conflict(
                "流水必须先经老板审批后才能同步 Actual".to_string(),
            ))
        }
        Some("synced") => {
            db_transaction.commit()?;
            return Ok(ActualSyncStage::Finished(json!({
                "status": "synced",
                "transaction_id": transaction_id,
                "idempotent": true,
                "reason": "transaction is already synced",
            })));
        }
        Some(_) => {}
    }
    let running: Option<Value> = db_transaction
        .query_row(
            "SELECT id, owner_code, transaction_id, status, request_json, response_json, created_at
             FROM actual_sync_jobs WHERE owner_code = ?1 AND transaction_id = ?2
               AND status = 'running' ORDER BY created_at DESC LIMIT 1",
            params![owner_code, transaction_id],
            actual_job_from_row,
        )
        .optional()?;
    if let Some(running) = running {
        let fresh = running["created_at"]
            .as_str()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|created| {
                created.with_timezone(&Utc) >= Utc::now() - Duration::minutes(2)
            });
        if fresh {
            db_transaction.commit()?;
            return Ok(ActualSyncStage::Finished(json!({
                "job": running,
                "status": "sync_in_progress",
                "transaction_id": transaction_id,
                "idempotent": true,
                "reason": "an Actual import is already running for this transaction",
            })));
        }
        db_transaction.execute(
            "UPDATE actual_sync_jobs SET status = 'failed', response_json = ?1 WHERE id = ?2",
            params![
                serde_json::to_string(&json!({
                    "status": "failed",
                    "reason": "previous Actual import claim expired; retrying by imported_id"
                }))?,
                running["id"].as_str().unwrap_or_default()
            ],
        )?;
    }
    let job = record_actual_job(
        &db_transaction,
        &owner_code,
        &transaction_id,
        "running",
        &request,
        &running_response,
    )?;
    db_transaction.commit()?;
    // Explicitly release the connection before spawning the external process.
    drop(connection);
    Ok(ActualSyncStage::Run {
        owner_code,
        transaction_id,
        account_id,
        cli: cli.expect("checked above"),
        job,
        request,
        payload,
    })
}

fn actual_sync_transaction(core: &BossCore, args: &Map<String, Value>) -> BossResult<Value> {
    let (owner_code, transaction_id, account_id, cli, job, request, payload) =
        match prepare_actual_sync(core, args)? {
            ActualSyncStage::Finished(value) => return Ok(value),
            ActualSyncStage::Run {
                owner_code,
                transaction_id,
                account_id,
                cli,
                job,
                request,
                payload,
            } => (
                owner_code,
                transaction_id,
                account_id,
                cli,
                job,
                request,
                payload,
            ),
        };
    let data = serde_json::to_string(&vec![payload.clone()])?;
    let mut command = actual_cli_command(&cli)?;
    command.args([
        "transactions",
        "import",
        "--account",
        &account_id,
        "--data",
        &data,
        "--format",
        "json",
    ]);
    if let Ok(data_dir) = std::env::var("ACTUAL_DATA_DIR") {
        command.env("ACTUAL_DATA_DIR", data_dir);
    }
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let outcome = match command.spawn() {
        Ok(mut child) => match child.wait_timeout(std::time::Duration::from_secs(60)) {
            Ok(Some(_)) => child.wait_with_output().map(|output| {
                (
                    output.status.success(),
                    output.status.code().unwrap_or(-1),
                    output.stdout,
                    output.stderr,
                )
            }),
            Ok(None) => {
                let _ = child.kill();
                let mut output = child.wait_with_output().map_err(BossError::Io)?;
                output
                    .stderr
                    .extend_from_slice(b"\nActual CLI timed out after 60 seconds");
                Ok((false, -1, output.stdout, output.stderr))
            }
            Err(error) => Err(error),
        },
        Err(error) => Err(error),
    };
    let (mut succeeded, return_code, stdout, stderr) = match outcome {
        Ok((success, code, stdout, stderr)) => (
            success,
            code,
            tail_text(&stdout, 4000),
            tail_text(&stderr, 4000),
        ),
        Err(error) => (false, -1, String::new(), error.to_string()),
    };
    succeeded = actual_cli_output_succeeded(succeeded, &stdout);
    let response = json!({
        "status": if succeeded { "synced" } else { "actual_cli_failed" },
        "returncode": return_code,
        "stdout": stdout,
        "stderr": stderr,
        "actual_payload": payload,
    });
    let mut connection = core.lock()?;
    finish_actual_claim(
        &mut connection,
        &owner_code,
        &transaction_id,
        job["id"].as_str().unwrap_or_default(),
        &request,
        &response,
        succeeded,
    )?;
    let mut final_job = job;
    final_job["status"] = json!(if succeeded { "synced" } else { "failed" });
    final_job["response_json"] = json!(serde_json::to_string(&response)?);
    Ok(json!({
        "job": final_job,
        "status": response["status"],
        "returncode": return_code,
        "stdout": stdout,
        "stderr": stderr,
        "actual_payload": payload,
    }))
}

fn actual_cli_output_succeeded(exit_success: bool, stdout: &str) -> bool {
    if !exit_success {
        return false;
    }
    let Ok(parsed) = serde_json::from_str::<Value>(stdout) else {
        return false;
    };
    !parsed.get("errors").is_some_and(|errors| match errors {
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::String(value) => !value.trim().is_empty(),
        Value::Null => false,
        _ => true,
    })
}

fn finish_actual_claim(
    connection: &mut Connection,
    owner_code: &str,
    transaction_id: &str,
    job_id: &str,
    request: &Value,
    response: &Value,
    succeeded: bool,
) -> BossResult<()> {
    let db_transaction = connection.transaction()?;
    let claimed = db_transaction.execute(
        "UPDATE actual_sync_jobs SET status = ?1, request_json = ?2, response_json = ?3
         WHERE owner_code = ?4 AND id = ?5 AND status = 'running'",
        params![
            if succeeded { "synced" } else { "failed" },
            serde_json::to_string(&request)?,
            serde_json::to_string(&response)?,
            owner_code,
            job_id
        ],
    )?;
    if claimed != 1 {
        return Err(BossError::Conflict(
            "Actual 同步运行权已过期，拒绝提交本次结果".to_string(),
        ));
    }
    if succeeded {
        let updated = db_transaction.execute(
            "UPDATE ledger_transactions SET status = 'synced'
             WHERE owner_code = ?1 AND id = ?2 AND status = 'posted'",
            params![owner_code, transaction_id],
        )?;
        if updated != 1 {
            return Err(BossError::Conflict(
                "流水审批状态已变化，拒绝提交 Actual 同步结果".to_string(),
            ));
        }
    }
    db_transaction.commit()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliLaunchKind {
    Direct,
    Node,
    WindowsCommand,
}

fn cli_launch_kind(path: &Path, windows: bool) -> CliLaunchKind {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("js" | "mjs" | "cjs") => CliLaunchKind::Node,
        Some("cmd" | "bat") if windows => CliLaunchKind::WindowsCommand,
        _ => CliLaunchKind::Direct,
    }
}

fn actual_cli_command(cli: &Path) -> BossResult<Command> {
    match cli_launch_kind(cli, cfg!(windows)) {
        CliLaunchKind::Node => {
            let node = find_named_executable("node").ok_or_else(|| {
                BossError::State("ACTUAL_CLI_BIN 是 JavaScript 文件，但系统未找到 node".to_string())
            })?;
            let mut command = Command::new(node);
            command.arg(cli);
            Ok(command)
        }
        CliLaunchKind::WindowsCommand => {
            let mut command = Command::new("cmd.exe");
            command.args(["/D", "/S", "/C"]).arg(cli);
            Ok(command)
        }
        CliLaunchKind::Direct => Ok(Command::new(cli)),
    }
}

fn actual_job_from_row(row: &Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?,
        "owner_code": row.get::<_, String>(1)?,
        "transaction_id": row.get::<_, String>(2)?,
        "status": row.get::<_, String>(3)?,
        "request_json": row.get::<_, String>(4)?,
        "response_json": row.get::<_, String>(5)?,
        "created_at": row.get::<_, String>(6)?,
    }))
}

fn actual_integration_status(
    connection: &Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let mut jobs_statement = connection.prepare(
        "SELECT id, owner_code, transaction_id, status, request_json, response_json, created_at
         FROM actual_sync_jobs WHERE owner_code = ?1 ORDER BY created_at DESC LIMIT 20",
    )?;
    let jobs = jobs_statement
        .query_map([&owner_code], actual_job_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    let mut count_statement = connection.prepare(
        "SELECT status, COUNT(*) FROM actual_sync_jobs WHERE owner_code = ?1 GROUP BY status",
    )?;
    let counts = count_statement
        .query_map([&owner_code], |row| {
            Ok(json!({
                "status": row.get::<_, String>(0)?,
                "count": row.get::<_, i64>(1)?,
            }))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({
        "owner_code": owner_code,
        "actual": actual_metadata(),
        "sync_jobs": jobs,
        "sync_job_counts": counts,
        "adapter_policy": {
            "direct_cli_sync_requires_env": true,
            "fallback_when_unconfigured": "queue_jsonl_and_export_csv",
            "amount_convention": "Actual transaction amounts are integer cents; expenses are negative."
        }
    }))
}

fn actual_export_import_file(
    data_dir: &Path,
    connection: &Connection,
    args: &Map<String, Value>,
) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let month = parse_month(&optional_string(args, "month").unwrap_or_else(current_month))?;
    let mut statement = connection.prepare(&format!(
        "SELECT {TRANSACTION_COLUMNS} FROM ledger_transactions
         WHERE owner_code = ?1 AND substr(tx_date, 1, 7) = ?2
           AND status IN ('posted', 'synced') AND currency = 'CNY'
         ORDER BY tx_date ASC, created_at ASC"
    ))?;
    let transactions = statement
        .query_map(params![owner_code, month], transaction_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    let excluded_pending: i64 = connection.query_row(
        "SELECT COUNT(*) FROM ledger_transactions WHERE owner_code = ?1
         AND substr(tx_date, 1, 7) = ?2 AND status = 'needs_review'",
        params![owner_code, month],
        |row| row.get(0),
    )?;
    let excluded_currency: i64 = connection.query_row(
        "SELECT COUNT(*) FROM ledger_transactions WHERE owner_code = ?1
         AND substr(tx_date, 1, 7) = ?2 AND UPPER(currency) <> 'CNY'",
        params![owner_code, month],
        |row| row.get(0),
    )?;
    let export_dir = data_dir.join("actual_exports");
    fs::create_dir_all(&export_dir)?;
    let stamp = Utc::now().format("%Y%m%d%H%M%S%3f");
    let csv_path = export_dir.join(format!(
        "actual-transactions-{owner_code}-{month}-{stamp}.csv"
    ));
    let jsonl_path = export_dir.join(format!(
        "actual-transactions-{owner_code}-{month}-{stamp}.jsonl"
    ));
    let mut csv_writer = csv::Writer::from_path(&csv_path)
        .map_err(|error| BossError::Io(std::io::Error::other(error)))?;
    csv_writer
        .write_record([
            "date",
            "amount_cents",
            "amount",
            "payee_name",
            "category_name",
            "notes",
            "cleared",
            "imported_id",
            "boss_transaction_id",
        ])
        .map_err(|error| BossError::Io(std::io::Error::other(error)))?;
    let mut jsonl = BufWriter::new(File::create(&jsonl_path)?);
    for transaction in &transactions {
        let actual = actual_payload(transaction);
        let cents = actual["amount"].as_i64().unwrap_or_default();
        csv_writer
            .write_record([
                actual["date"].as_str().unwrap_or_default().to_string(),
                cents.to_string(),
                cents_as_decimal(cents),
                actual["payee_name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                transaction["category"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                actual["notes"].as_str().unwrap_or_default().to_string(),
                actual["cleared"].as_bool().unwrap_or(false).to_string(),
                actual["imported_id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                transaction["id"].as_str().unwrap_or_default().to_string(),
            ])
            .map_err(|error| BossError::Io(std::io::Error::other(error)))?;
        writeln!(
            jsonl,
            "{}",
            serde_json::to_string(&json!({
                "boss_transaction": transaction,
                "actual_payload": actual,
            }))?
        )?;
    }
    csv_writer
        .flush()
        .map_err(|error| BossError::Io(std::io::Error::other(error)))?;
    jsonl.flush()?;
    Ok(json!({
        "owner_code": owner_code,
        "month": month,
        "transaction_count": transactions.len(),
        "excluded_pending_count": excluded_pending,
        "excluded_currency_count": excluded_currency,
        "csv_path": csv_path.to_string_lossy(),
        "jsonl_path": jsonl_path.to_string_lossy(),
        "actual": actual_metadata(),
    }))
}

fn boss_owner_context(connection: &mut Connection, args: &Map<String, Value>) -> BossResult<Value> {
    let owner_code = required_string(args, "owner_code")?;
    let (start, end) = resolve_date_range(args, true)?;
    let month = parse_month(&optional_string(args, "month").unwrap_or_else(current_month))?;
    let period_args = json!({
        "owner_code": owner_code,
        "date_from": start.to_string(),
        "date_to": end.to_string(),
    });
    let period_args = period_args.as_object().expect("object literal");
    let dashboard = boss_dashboard_summary(connection, period_args)?;
    let work_report = boss_employee_work_report(connection, period_args)?;
    let employees = employee_rows(connection, &owner_code, Some((start, end)))?;
    let roster = employee_rows(connection, &owner_code, None)?;
    let ledger_args = json!({ "owner_code": owner_code, "month": month });
    let ledger = ledger_report(connection, ledger_args.as_object().expect("object literal"))?;
    let invoices_args = json!({ "owner_code": owner_code, "limit": 20 });
    let invoice_payload = invoice_list_drafts(
        connection,
        invoices_args.as_object().expect("object literal"),
    )?;
    let invoices = invoice_payload["drafts"].clone();
    let reviews = review_rows(connection, &owner_code, None, Some(start), Some(end))?;
    let owner = connection
        .query_row(
            "SELECT id, email, owner_code, created_at FROM owners WHERE owner_code = ?1",
            [&owner_code],
            |row| {
                Ok(json!({
                    "id": row.get::<_, String>(0)?,
                    "email": row.get::<_, String>(1)?,
                    "owner_code": row.get::<_, String>(2)?,
                    "created_at": row.get::<_, String>(3)?,
                }))
            },
        )
        .optional()?
        .unwrap_or_else(|| json!({ "owner_code": owner_code }));
    let actual = actual_metadata();
    Ok(json!({
        "owner": owner,
        "date": Local::now().date_naive().to_string(),
        "date_from": start.to_string(),
        "date_to": end.to_string(),
        "week": { "date_from": start.to_string(), "date_to": end.to_string() },
        "month": month,
        "employees": employees,
        "bindings": {
            "owner_code": owner_code,
            "employee_count": roster.len(),
            "employees": roster,
        },
        "dashboard": dashboard,
        "work_report": work_report,
        "weekly_work_events": work_report["events"],
        "reviews": reviews,
        "ledger_report": ledger,
        "invoice_drafts": invoices,
        "accounting_provider": {
            "repository": actual["repository"],
            "local_path": actual["local_path"],
            "source_present": actual["source_present"],
            "integration_mode": "Rust Actual CLI/API adapter plus unified boss UI",
            "status": if actual["source_present"].as_bool().unwrap_or(false) {
                "source_ready"
            } else {
                "source_missing"
            },
            "direct_sync_status": if actual["ready_for_direct_cli_sync"].as_bool().unwrap_or(false) {
                "ready"
            } else {
                "queued_until_actual_env_configured"
            }
        },
        "policy": {
            "employee_answers_require_tool": true,
            "ledger_answers_require_tool": true,
            "actual_answers_require_tool": true,
            "invoice_unknown_fields_must_remain_null": true,
            "week_reviews_are_persisted": true,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tempfile::TempDir;

    fn test_core() -> (TempDir, BossCore) {
        let directory = tempfile::tempdir().expect("tempdir");
        let core = BossCore::open(directory.path().join("boss.sqlite")).expect("open test db");
        (directory, core)
    }

    fn accepted_call(
        core: &BossCore,
        called: &mut BTreeSet<String>,
        name: &'static str,
        arguments: Value,
    ) -> Value {
        let result = core
            .dispatch_tool(name, arguments)
            .unwrap_or_else(|error| panic!("acceptance call {name} failed: {error}"));
        called.insert(name.to_string());
        result
    }

    #[test]
    fn every_registered_tool_passes_acceptance_and_persists() {
        let (directory, core) = test_core();
        let database_path = directory.path().join("boss.sqlite");
        let mut called = BTreeSet::new();
        let week_start = "2026-07-13";
        let week_end = "2026-07-19";
        let month = "2026-07";

        let owner = accepted_call(
            &core,
            &mut called,
            "owner.create_binding_code",
            json!({
                "owner_id": "acceptance-owner",
                "email": "acceptance-owner@example.com"
            }),
        );
        let second_owner_code = owner["owner_code"].as_str().unwrap().to_string();
        let owner_again = accepted_call(
            &core,
            &mut called,
            "owner.create_binding_code",
            json!({
                "owner_id": "acceptance-owner",
                "email": "acceptance-owner@example.com"
            }),
        );
        assert_eq!(owner_again["owner_code"], second_owner_code);

        let event = json!({
            "owner_code": "BOSS-7429",
            "event_id": "acceptance-event-1",
            "employee_id": "acceptance-employee-1",
            "employee_name": "验收员工",
            "role": "运营验收",
            "employee_status": "active",
            "event_date": "2026-07-16",
            "task_type": "acceptance_delivery",
            "material_count": 5,
            "cost_cents": 12345,
            "quality_score": 88,
            "summary": "完成第一版验收交付"
        });
        accepted_call(
            &core,
            &mut called,
            "employee.report_work_event",
            event.clone(),
        );
        let mut updated_event = event.clone();
        updated_event["material_count"] = json!(7);
        updated_event["quality_score"] = json!(94);
        updated_event["summary"] = json!("幂等更新后的验收交付");
        let updated = accepted_call(
            &core,
            &mut called,
            "employee.report_work_event",
            updated_event,
        );
        assert_eq!(updated["idempotent_key"], "BOSS-7429:acceptance-event-1");

        accepted_call(
            &core,
            &mut called,
            "employee.report_work_event",
            json!({
                "owner_code": second_owner_code,
                "event_id": "acceptance-other-event",
                "employee_id": "acceptance-other-employee",
                "employee_name": "另一老板员工",
                "role": "隔离验收",
                "employee_status": "active",
                "event_date": "2026-07-16",
                "task_type": "isolated_delivery",
                "material_count": 1,
                "cost_cents": 0,
                "quality_score": 90,
                "summary": "另一老板空间的事件"
            }),
        );
        let bindings = accepted_call(
            &core,
            &mut called,
            "owner.list_bound_employees",
            json!({ "owner_code": "BOSS-7429" }),
        );
        assert!(bindings["employees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|employee| employee["id"] == "acceptance-employee-1"));
        assert!(!bindings["employees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|employee| employee["id"] == "acceptance-other-employee"));

        let dashboard = accepted_call(
            &core,
            &mut called,
            "boss.dashboard_summary",
            json!({
                "owner_code": "BOSS-7429",
                "date_from": week_start,
                "date_to": week_end
            }),
        );
        assert!(dashboard["event_count"].as_i64().unwrap() >= 1);
        let work_report = accepted_call(
            &core,
            &mut called,
            "boss.employee_work_report",
            json!({
                "owner_code": "BOSS-7429",
                "employee_id": "acceptance-employee-1",
                "date_from": week_start,
                "date_to": week_end
            }),
        );
        assert_eq!(work_report["event_count"], 1);
        assert_eq!(work_report["events"][0]["material_count"], 7);

        let review = accepted_call(
            &core,
            &mut called,
            "boss.save_week_review",
            json!({
                "owner_code": "BOSS-7429",
                "employee_id": "acceptance-employee-1",
                "week_start": week_start,
                "week_end": week_end,
                "status": "needs_supplement",
                "note": "请补充转化截图"
            }),
        );
        assert_eq!(review["status"], "needs_supplement");
        let reviewed = accepted_call(
            &core,
            &mut called,
            "boss.save_week_review",
            json!({
                "owner_code": "BOSS-7429",
                "employee_id": "acceptance-employee-1",
                "week_start": week_start,
                "week_end": week_end,
                "status": "reviewed",
                "note": "截图已核验"
            }),
        );
        assert_eq!(reviewed["id"], review["id"]);
        let reviews = accepted_call(
            &core,
            &mut called,
            "employee.list_week_reviews",
            json!({
                "owner_code": "BOSS-7429",
                "employee_id": "acceptance-employee-1",
                "week_start": week_start,
                "week_end": week_end
            }),
        );
        assert_eq!(reviews["reviews"].as_array().unwrap().len(), 1);
        assert_eq!(reviews["reviews"][0]["status"], "reviewed");

        let income = accepted_call(
            &core,
            &mut called,
            "ledger.add_transaction",
            json!({
                "owner_code": "BOSS-7429",
                "type": "income",
                "amount": "888.88",
                "currency": "CNY",
                "category": "验收回款",
                "counterparty": "验收客户",
                "date": "2026-07-16",
                "source": "acceptance",
                "status": "posted"
            }),
        );
        assert_eq!(income["amount_cents"], 88888);
        let expense = accepted_call(
            &core,
            &mut called,
            "ledger.add_transaction",
            json!({
                "owner_code": "BOSS-7429",
                "type": "expense",
                "amount": 66.66,
                "category": "验收支出",
                "counterparty": "验收供应商",
                "date": "2026-07-16",
                "source": "acceptance",
                "status": "needs_review"
            }),
        );
        let pending = accepted_call(
            &core,
            &mut called,
            "ledger.add_transaction",
            json!({
                "owner_code": "BOSS-7429",
                "type": "expense",
                "amount": 12.34,
                "category": "导出排除项",
                "date": "2026-07-16",
                "status": "needs_review"
            }),
        );
        let expense_id = expense["id"].as_str().unwrap();
        let listed = accepted_call(
            &core,
            &mut called,
            "ledger.list_transactions",
            json!({
                "owner_code": "BOSS-7429",
                "month": month,
                "status": "needs_review",
                "limit": 200,
                "offset": 0
            }),
        );
        assert!(listed["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|transaction| transaction["id"] == expense["id"]));
        let approved = accepted_call(
            &core,
            &mut called,
            "ledger.update_transaction_status",
            json!({
                "owner_code": "BOSS-7429",
                "transaction_id": expense_id,
                "status": "posted",
                "review_note": "验收审批通过"
            }),
        );
        assert_eq!(approved["status"], "posted");
        assert_eq!(approved["review_note"], "验收审批通过");
        let category_report = accepted_call(
            &core,
            &mut called,
            "ledger.category_report",
            json!({ "owner_code": "BOSS-7429", "month": month }),
        );
        assert!(category_report["categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|category| category["category"] == "验收支出"));
        let report = accepted_call(
            &core,
            &mut called,
            "ledger.report",
            json!({ "owner_code": "BOSS-7429", "month": month }),
        );
        assert!(report["income_cents"].as_i64().unwrap() >= 88888);
        assert!(report["needs_review_count"].as_i64().unwrap() >= 1);

        let invoice_text = "发票号码: ACPT123456\n发票代码: CODE123456\n开票日期: 2026-07-16\n销售方: 验收票据公司\n金额: 100.00\n税额: 6.00\n价税合计: 106.00\n素材服务";
        let encoded = base64::engine::general_purpose::STANDARD.encode(invoice_text.as_bytes());
        let invoice = accepted_call(
            &core,
            &mut called,
            "invoice.upload_and_extract",
            json!({
                "owner_code": "BOSS-7429",
                "file_name": "acceptance-invoice.txt",
                "mime_type": "text/plain",
                "content_base64": encoded
            }),
        );
        assert_eq!(invoice["status"], "extracted");
        assert_eq!(invoice["fields"]["total"], 106.0);
        let stored_invoice_path = PathBuf::from(invoice["file_path"].as_str().unwrap());
        assert!(stored_invoice_path.is_file());
        assert!(stored_invoice_path.starts_with(directory.path()));
        let duplicate_invoice = accepted_call(
            &core,
            &mut called,
            "invoice.upload_and_extract",
            json!({
                "owner_code": "BOSS-7429",
                "file_name": "acceptance-invoice.txt",
                "mime_type": "text/plain",
                "content_base64": base64::engine::general_purpose::STANDARD.encode(invoice_text)
            }),
        );
        assert_eq!(duplicate_invoice["id"], invoice["id"]);
        assert_eq!(duplicate_invoice["idempotent"], true);
        let extracted = accepted_call(
            &core,
            &mut called,
            "invoice.extract_fields",
            json!({
                "owner_code": "BOSS-7429",
                "extracted_text": "发票号码: ACPT654321\n开票日期: 2026-07-16\n销售方: 第二票据公司\n合计金额: 20.50"
            }),
        );
        assert_eq!(extracted["fields"]["total"], 20.5);
        let drafts = accepted_call(
            &core,
            &mut called,
            "invoice.list_drafts",
            json!({ "owner_code": "BOSS-7429", "limit": 200 }),
        );
        assert!(drafts["drafts"].as_array().unwrap().len() >= 2);
        let invoice_id = invoice["id"].as_str().unwrap();
        let invoice_ledger = accepted_call(
            &core,
            &mut called,
            "invoice.post_to_ledger",
            json!({
                "owner_code": "BOSS-7429",
                "invoice_id": invoice_id,
                "type": "expense",
                "category": "验收票据"
            }),
        );
        assert_eq!(invoice_ledger["transaction"]["status"], "needs_review");
        let invoice_ledger_again = accepted_call(
            &core,
            &mut called,
            "invoice.post_to_ledger",
            json!({
                "owner_code": "BOSS-7429",
                "invoice_id": invoice_id,
                "type": "expense",
                "category": "验收票据"
            }),
        );
        assert_eq!(
            invoice_ledger_again["transaction"]["id"],
            invoice_ledger["transaction"]["id"]
        );
        assert_eq!(invoice_ledger_again["idempotent"], true);

        let actual_status = accepted_call(
            &core,
            &mut called,
            "actual.integration_status",
            json!({ "owner_code": "BOSS-7429" }),
        );
        assert_eq!(actual_status["owner_code"], "BOSS-7429");
        let dry_run = accepted_call(
            &core,
            &mut called,
            "actual.sync_transaction",
            json!({
                "owner_code": "BOSS-7429",
                "transaction_id": income["id"],
                "dry_run": true
            }),
        );
        assert_eq!(dry_run["status"], "dry_run");
        let queued = accepted_call(
            &core,
            &mut called,
            "actual.sync_transaction",
            json!({
                "owner_code": "BOSS-7429",
                "transaction_id": income["id"]
            }),
        );
        assert_eq!(queued["status"], "queued_needs_actual_credentials");
        let queued_again = accepted_call(
            &core,
            &mut called,
            "actual.sync_transaction",
            json!({
                "owner_code": "BOSS-7429",
                "transaction_id": income["id"]
            }),
        );
        assert_eq!(queued_again["idempotent"], true);
        assert_eq!(queued_again["job"]["id"], queued["job"]["id"]);
        let exported = accepted_call(
            &core,
            &mut called,
            "actual.export_import_file",
            json!({ "owner_code": "BOSS-7429", "month": month }),
        );
        assert!(exported["transaction_count"].as_u64().unwrap() >= 2);
        assert!(exported["excluded_pending_count"].as_i64().unwrap() >= 1);
        let csv_path = PathBuf::from(exported["csv_path"].as_str().unwrap());
        let jsonl_path = PathBuf::from(exported["jsonl_path"].as_str().unwrap());
        assert!(csv_path.is_file() && jsonl_path.is_file());
        assert!(csv_path.starts_with(directory.path()));
        assert!(jsonl_path.starts_with(directory.path()));
        let csv = fs::read_to_string(&csv_path).expect("read Actual CSV");
        assert!(csv.contains(income["id"].as_str().unwrap()));
        assert!(!csv.contains(pending["id"].as_str().unwrap()));

        let context = accepted_call(
            &core,
            &mut called,
            "boss.owner_context",
            json!({
                "owner_code": "BOSS-7429",
                "date_from": week_start,
                "date_to": week_end,
                "month": month
            }),
        );
        assert!(context["reviews"]
            .as_array()
            .unwrap()
            .iter()
            .any(|review| review["employee_id"] == "acceptance-employee-1"));

        assert!(core
            .dispatch_tool(
                "employee.report_work_event",
                json!({
                    "owner_code": "BOSS-7429",
                    "event_id": "bad-event",
                    "employee_id": "bad-employee",
                    "employee_name": "坏数据",
                    "task_type": "bad",
                    "event_date": "2026-02-30",
                    "material_count": -1,
                    "quality_score": 101,
                    "summary": "bad"
                })
            )
            .is_err());
        assert!(core
            .dispatch_tool(
                "boss.save_week_review",
                json!({
                    "owner_code": "BOSS-7429",
                    "employee_id": "acceptance-employee-1",
                    "week_start": week_start,
                    "week_end": week_end,
                    "status": "needs_supplement",
                    "note": ""
                })
            )
            .is_err());
        assert!(core
            .dispatch_tool(
                "ledger.add_transaction",
                json!({
                    "owner_code": "BOSS-7429",
                    "type": "expense",
                    "amount": 1.001,
                    "currency": "CNY",
                    "category": "非法金额"
                })
            )
            .is_err());
        assert!(core
            .dispatch_tool(
                "ledger.update_transaction_status",
                json!({
                    "owner_code": second_owner_code,
                    "transaction_id": expense["id"],
                    "status": "posted"
                })
            )
            .is_err());
        assert!(core
            .dispatch_tool(
                "invoice.post_to_ledger",
                json!({
                    "owner_code": "BOSS-7429",
                    "invoice_id": invoice_id,
                    "category": "禁止绕过审批",
                    "status": "posted"
                })
            )
            .is_err());
        assert!(core
            .dispatch_tool(
                "actual.sync_transaction",
                json!({
                    "owner_code": "BOSS-7429",
                    "transaction_id": invoice_ledger["transaction"]["id"]
                })
            )
            .is_err());
        let approved_invoice = accepted_call(
            &core,
            &mut called,
            "ledger.update_transaction_status",
            json!({
                "owner_code": "BOSS-7429",
                "transaction_id": invoice_ledger["transaction"]["id"],
                "status": "posted",
                "review_note": "票据验收审批"
            }),
        );
        assert_eq!(approved_invoice["status"], "posted");
        let approved_drafts = accepted_call(
            &core,
            &mut called,
            "invoice.list_drafts",
            json!({ "owner_code": "BOSS-7429", "limit": 200 }),
        );
        assert!(approved_drafts["drafts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|draft| draft["id"] == invoice["id"] && draft["status"] == "posted_to_ledger"));
        assert!(core.dispatch_tool("not.a.tool", json!({})).is_err());
        assert!(core
            .dispatch_tool("ledger.report", json!("not-an-object"))
            .is_err());

        let expected = TOOL_NAMES
            .iter()
            .map(|name| (*name).to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            called, expected,
            "acceptance matrix missed a registered tool"
        );

        drop(core);
        let reopened = BossCore::open(&database_path).expect("reopen persisted acceptance db");
        let persisted_other_owner = reopened
            .dispatch_tool(
                "owner.list_bound_employees",
                json!({ "owner_code": second_owner_code }),
            )
            .expect("persisted owner binding");
        assert!(persisted_other_owner["employees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|employee| employee["id"] == "acceptance-other-employee"));
        let persisted_work = reopened
            .dispatch_tool(
                "boss.employee_work_report",
                json!({
                    "owner_code": "BOSS-7429",
                    "employee_id": "acceptance-employee-1",
                    "date_from": week_start,
                    "date_to": week_end
                }),
            )
            .expect("persisted work report");
        assert_eq!(persisted_work["event_count"], 1);
        assert_eq!(persisted_work["events"][0]["material_count"], 7);
        let persisted_reviews = reopened
            .dispatch_tool(
                "employee.list_week_reviews",
                json!({
                    "owner_code": "BOSS-7429",
                    "employee_id": "acceptance-employee-1",
                    "week_start": week_start,
                    "week_end": week_end
                }),
            )
            .expect("persisted reviews");
        assert_eq!(persisted_reviews["reviews"].as_array().unwrap().len(), 1);
        let persisted_ledger = reopened
            .dispatch_tool(
                "ledger.list_transactions",
                json!({ "owner_code": "BOSS-7429", "month": month, "limit": 200 }),
            )
            .expect("persisted ledger");
        assert!(persisted_ledger["transactions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|transaction| transaction["id"] == income["id"]));
        let persisted_drafts = reopened
            .dispatch_tool(
                "invoice.list_drafts",
                json!({ "owner_code": "BOSS-7429", "limit": 200 }),
            )
            .expect("persisted invoices");
        assert!(persisted_drafts["drafts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|draft| draft["id"] == invoice["id"]));
        let persisted_actual = reopened
            .dispatch_tool(
                "actual.integration_status",
                json!({ "owner_code": "BOSS-7429" }),
            )
            .expect("persisted Actual jobs");
        assert!(persisted_actual["sync_jobs"].as_array().unwrap().len() >= 2);
    }

    #[test]
    fn windows_executable_discovery_covers_npm_shims() {
        let names = executable_file_names("actual", true, Some(".COM;.EXE;.BAT;.CMD"))
            .into_iter()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .collect::<Vec<_>>();
        for expected in ["actual.exe", "actual.cmd", "actual.bat"] {
            assert!(
                names.iter().any(|name| name == expected),
                "missing {expected}"
            );
        }
        assert_eq!(
            executable_file_names("actual.cmd", true, None),
            vec![OsString::from("actual.cmd")]
        );
        assert_eq!(
            cli_launch_kind(Path::new("C:/npm/actual.cmd"), true),
            CliLaunchKind::WindowsCommand
        );
        assert_eq!(
            cli_launch_kind(Path::new("C:/npm/actual.mjs"), true),
            CliLaunchKind::Node
        );
        assert_eq!(
            cli_launch_kind(Path::new("C:/bin/actual.exe"), true),
            CliLaunchKind::Direct
        );

        let directory = tempfile::tempdir().expect("tempdir");
        let shim = directory.path().join("actual.cmd");
        fs::write(&shim, "@echo off\r\n").expect("write shim");
        let discovered = find_named_executable_in(
            "actual",
            &[directory.path().to_path_buf()],
            true,
            Some(".EXE;.CMD;.BAT"),
        );
        assert_eq!(discovered, Some(shim));
    }

    #[test]
    fn cross_device_event_and_week_review_are_idempotent() {
        let (_directory, core) = test_core();
        let event = json!({
            "owner_code": "BOSS-7429",
            "event_id": "employee-device-event-1",
            "employee_id": "employee-device-1",
            "employee_name": "真实员工",
            "role": "内容运营",
            "employee_status": "active",
            "event_date": "2026-07-15",
            "task_type": "weekly_delivery",
            "material_count": 8,
            "cost_cents": 1234,
            "quality_score": 92,
            "summary": "完成真实员工端周工作回传"
        });
        core.dispatch_tool("employee.report_work_event", event.clone())
            .expect("first report");
        let mut updated = event;
        updated["material_count"] = json!(11);
        updated["summary"] = json!("同一事件幂等更新");
        core.dispatch_tool("employee.report_work_event", updated)
            .expect("idempotent update");

        let report = core
            .dispatch_tool(
                "boss.employee_work_report",
                json!({
                    "owner_code": "BOSS-7429",
                    "employee_id": "employee-device-1",
                    "date_from": "2026-07-14",
                    "date_to": "2026-07-20"
                }),
            )
            .expect("work report");
        assert_eq!(report["event_count"], 1);
        assert_eq!(report["events"][0]["material_count"], 11);

        let review_args = json!({
            "owner_code": "BOSS-7429",
            "employee_id": "employee-device-1",
            "week_start": "2026-07-14",
            "week_end": "2026-07-20",
            "status": "needs_supplement",
            "note": "请补充结果截图"
        });
        core.dispatch_tool("boss.save_week_review", review_args.clone())
            .expect("save review");
        let mut reviewed = review_args;
        reviewed["status"] = json!("reviewed");
        reviewed["note"] = json!("已核验");
        core.dispatch_tool("boss.save_week_review", reviewed)
            .expect("update review");

        let employee_reviews = core
            .dispatch_tool(
                "employee.list_week_reviews",
                json!({
                    "owner_code": "BOSS-7429",
                    "employee_id": "employee-device-1",
                    "week_start": "2026-07-14",
                    "week_end": "2026-07-20"
                }),
            )
            .expect("employee review poll");
        assert_eq!(employee_reviews["reviews"].as_array().unwrap().len(), 1);
        assert_eq!(employee_reviews["reviews"][0]["status"], "reviewed");

        let context = core
            .dispatch_tool(
                "boss.owner_context",
                json!({
                    "owner_code": "BOSS-7429",
                    "date_from": "2026-07-14",
                    "date_to": "2026-07-20",
                    "month": "2026-07"
                }),
            )
            .expect("owner context");
        assert_eq!(context["reviews"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn invoice_is_idempotent_and_cannot_bypass_review() {
        let (_directory, core) = test_core();
        let upload = json!({
            "owner_code": "BOSS-7429",
            "file_name": "receipt.txt",
            "extracted_text": "发票号码: 12345678\n开票日期: 2026-07-15\n销售方: 测试供应商\n价税合计: 188.25\n素材服务"
        });
        let first = core
            .dispatch_tool("invoice.upload_and_extract", upload.clone())
            .expect("upload invoice");
        let duplicate = core
            .dispatch_tool("invoice.upload_and_extract", upload)
            .expect("duplicate invoice");
        assert_eq!(first["id"], duplicate["id"]);
        assert_eq!(duplicate["idempotent"], true);

        let invoice_id = first["id"].as_str().unwrap();
        let bypass = core.dispatch_tool(
            "invoice.post_to_ledger",
            json!({
                "owner_code": "BOSS-7429",
                "invoice_id": invoice_id,
                "category": "素材采集",
                "status": "posted"
            }),
        );
        assert!(matches!(bypass, Err(BossError::Validation(_))));
        let posted = core
            .dispatch_tool(
                "invoice.post_to_ledger",
                json!({
                    "owner_code": "BOSS-7429",
                    "invoice_id": invoice_id,
                    "category": "素材采集"
                }),
            )
            .expect("create ledger draft");
        assert_eq!(posted["transaction"]["status"], "needs_review");
        let transaction_id = posted["transaction"]["id"].as_str().unwrap();
        let sync = core.dispatch_tool(
            "actual.sync_transaction",
            json!({ "owner_code": "BOSS-7429", "transaction_id": transaction_id }),
        );
        assert!(matches!(sync, Err(BossError::Conflict(_))));
    }

    #[test]
    fn amount_date_and_currency_validation_are_strict() {
        let (_directory, core) = test_core();
        for amount in [json!(0), json!(-1), json!(1.001), json!("abc")] {
            assert!(core
                .dispatch_tool(
                    "ledger.add_transaction",
                    json!({
                        "owner_code": "BOSS-7429",
                        "type": "expense",
                        "amount": amount,
                        "category": "测试"
                    })
                )
                .is_err());
        }
        assert!(core
            .dispatch_tool(
                "ledger.add_transaction",
                json!({
                    "owner_code": "BOSS-7429",
                    "type": "expense",
                    "amount": 1,
                    "currency": "USD",
                    "category": "测试"
                })
            )
            .is_err());
        assert!(core
            .dispatch_tool(
                "ledger.add_transaction",
                json!({
                    "owner_code": "BOSS-7429",
                    "type": "expense",
                    "amount": 1,
                    "date": "2026-02-30",
                    "category": "测试"
                })
            )
            .is_err());
    }

    #[test]
    fn actual_requires_json_success_and_current_running_claim() {
        assert!(actual_cli_output_succeeded(true, "{}"));
        assert!(!actual_cli_output_succeeded(true, "not-json"));
        assert!(!actual_cli_output_succeeded(
            true,
            r#"{"errors":["duplicate"]}"#
        ));
        assert!(!actual_cli_output_succeeded(false, "{}"));

        let (_directory, core) = test_core();
        let ledger = core
            .dispatch_tool(
                "ledger.add_transaction",
                json!({
                    "owner_code": "BOSS-7429",
                    "type": "expense",
                    "amount": 10,
                    "category": "测试",
                    "status": "posted"
                }),
            )
            .expect("posted transaction");
        let transaction_id = ledger["id"].as_str().unwrap();
        let mut connection = core.lock().expect("db lock");
        let request = json!({});
        let response = json!({ "status": "synced" });
        let job = record_actual_job(
            &connection,
            "BOSS-7429",
            transaction_id,
            "failed",
            &request,
            &response,
        )
        .expect("historical job");
        let result = finish_actual_claim(
            &mut connection,
            "BOSS-7429",
            transaction_id,
            job["id"].as_str().unwrap(),
            &request,
            &response,
            true,
        );
        assert!(matches!(result, Err(BossError::Conflict(_))));
        let status: String = connection
            .query_row(
                "SELECT status FROM ledger_transactions WHERE id = ?1",
                [transaction_id],
                |row| row.get(0),
            )
            .expect("transaction status");
        assert_eq!(status, "posted");
    }

    #[test]
    fn opens_and_migrates_legacy_python_schema() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("legacy.sqlite");
        let connection = Connection::open(&path).expect("legacy db");
        connection
            .execute_batch(
                r#"
                CREATE TABLE owners (id TEXT PRIMARY KEY, email TEXT NOT NULL,
                    owner_code TEXT NOT NULL UNIQUE, created_at TEXT NOT NULL);
                CREATE TABLE employees (id TEXT PRIMARY KEY, owner_code TEXT NOT NULL,
                    name TEXT NOT NULL, role TEXT NOT NULL, status TEXT NOT NULL,
                    quality INTEGER NOT NULL, created_at TEXT NOT NULL);
                CREATE TABLE work_events (id TEXT PRIMARY KEY, owner_code TEXT NOT NULL,
                    employee_id TEXT NOT NULL, event_date TEXT NOT NULL, task_type TEXT NOT NULL,
                    material_count INTEGER NOT NULL DEFAULT 0, cost_cents INTEGER NOT NULL DEFAULT 0,
                    quality_score INTEGER NOT NULL, summary TEXT NOT NULL);
                CREATE TABLE ledger_transactions (id TEXT PRIMARY KEY, owner_code TEXT NOT NULL,
                    tx_date TEXT NOT NULL, type TEXT NOT NULL, category TEXT NOT NULL,
                    counterparty TEXT NOT NULL DEFAULT '', amount_cents INTEGER NOT NULL,
                    currency TEXT NOT NULL DEFAULT 'CNY', source TEXT NOT NULL DEFAULT '',
                    evidence_path TEXT NOT NULL DEFAULT '', status TEXT NOT NULL DEFAULT 'needs_review',
                    created_at TEXT NOT NULL);
                CREATE TABLE invoice_drafts (id TEXT PRIMARY KEY, owner_code TEXT NOT NULL,
                    file_path TEXT NOT NULL DEFAULT '', fields_json TEXT NOT NULL,
                    status TEXT NOT NULL, created_at TEXT NOT NULL);
                INSERT INTO owners VALUES ('legacy-owner', 'legacy@example.com', 'BOSS-1000', '2026-01-01T00:00:00Z');
                "#,
            )
            .expect("create legacy schema");
        drop(connection);

        let core = BossCore::open(&path).expect("migrate legacy db");
        let connection = core.lock().expect("lock migrated db");
        for (table, column) in [
            ("invoice_drafts", "content_hash"),
            ("invoice_drafts", "business_key"),
            ("ledger_transactions", "review_note"),
            ("work_events", "updated_at"),
        ] {
            let found: bool = connection
                .query_row(
                    &format!(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1)"
                    ),
                    [column],
                    |row| row.get(0),
                )
                .expect("column check");
            assert!(found, "missing migrated column {table}.{column}");
        }
    }
}
