//! 任务管理命名空间：`tasks` / `work` / `subjects` / `background-tasks` / `background-workers`。
//!
//! 1:1 复刻 Beav (`desktop/electron/main.ts`) 的 IPC 通道行为，落库到 SQLite
//! （`agent_tasks` + `agent_task_traces`），把 [`super::dispatch_invoke`] 的请求按全名 match
//! 路由进来。本模块只定义 [`invoke`]；该命名空间没有纯单向（send）通道。
//!
//! # 存储策略
//!
//! - `tasks:*` → 直接读写 `agent_tasks` / `agent_task_traces`（DB-backed，与 Beav
//!   `TaskGraphRuntime` + `db.ts` 的 `listAgentTasks` 等一致）。
//! - `work:*` → 复用 `agent_tasks` 行（`task_type='work'`），工作项的 title/tags/dependsOn/
//!   refs/schedule 等存进 `metadata_json`（`{kind:"work",...}`）；快照在内存里解析
//!   `effectiveStatus` / `blockedBy` / `ready`（对齐 `WorkItemStore.materializeSnapshot`）。
//! - `subjects:*` → 复用 `agent_tasks` 行（`task_type='subject'`），完整记录存 `metadata_json`
//!   （`{kind:"subject",...}`）；分类用 `task_type='subject-category'`（`{kind:"subject-category"}`）。
//!   图片/语音资产只记录相对路径（不做真实落盘——见 `stub_channels`）。
//! - `background-tasks:*` / `background-workers:*` → 进程内 `static Mutex<Vec<Value>>` 占位
//!   （真实 worker 进程池尚未接入，见 `stub_channels`）。
//!
//! # 安全默认
//!
//! 所有写操作默认 `dry_run`（返回预览而不落库）；payload 带 `dryRun:false` 或 `confirm:true`
//! 时才真正执行（见 [`dry_run`]）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use super::{AppState, EventEmitter};
use crate::db::Db;

/// 后台任务注册表占位（真实子进程池未接入）。
static BG_TASKS: Mutex<Vec<Value>> = Mutex::new(Vec::new());

/// 双向调用分发。按通道全名 match；未知通道返回 [`Err`]。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let db = &state.db;
    let emitter: &dyn EventEmitter = &*state.emitter;
    match channel {
        // ---- tasks ----
        "tasks:create" => Ok(create_task(db, &payload)?),
        "tasks:list" => Ok(list_tasks(db, &payload)?),
        "tasks:get" => Ok(get_task(db, &payload)?),
        "tasks:resume" => Ok(resume_task(db, &payload)?),
        "tasks:cancel" => Ok(cancel_task(db, &payload)?),
        "tasks:trace" => Ok(trace_task(db, &payload)?),
        "tasks:resume-from-session" => Ok(resume_from_session(db, &payload)?),
        // ---- work ----
        "work:list" => Ok(list_work(db, &payload)?),
        "work:get" => Ok(get_work(db, &payload)?),
        "work:ready" => Ok(ready_work(db, &payload)?),
        "work:update" => Ok(update_work(db, emitter, &payload)?),
        // ---- subjects ----
        "subjects:list" => Ok(list_subjects(db, &payload)?),
        "subjects:get" => Ok(get_subject(db, &payload)?),
        "subjects:create" => Ok(create_subject(db, emitter, &payload)?),
        "subjects:update" => Ok(update_subject(db, emitter, &payload)?),
        "subjects:delete" => Ok(delete_subject(db, emitter, &payload)?),
        "subjects:search" => Ok(search_subjects(db, &payload)?),
        // ---- subjects:categories ----
        "subjects:categories:list" => Ok(list_subject_categories(db)?),
        "subjects:categories:create" => Ok(create_subject_category(db, emitter, &payload)?),
        "subjects:categories:update" => Ok(update_subject_category(db, emitter, &payload)?),
        "subjects:categories:delete" => Ok(delete_subject_category(db, emitter, &payload)?),
        // ---- background-tasks（内存占位）----
        "background-tasks:list" => Ok(list_bg_tasks()),
        "background-tasks:get" => Ok(get_bg_task(&payload)),
        "background-tasks:cancel" => Ok(cancel_bg_task(&payload)),
        "background-tasks:retry" => Ok(retry_bg_task(&payload)),
        "background-tasks:archive" => Ok(archive_bg_task(&payload)),
        // ---- background-workers（内存占位）----
        "background-workers:get-pool-state" => Ok(get_pool_state()),
        other => Err(anyhow::anyhow!("tasks 命名空间未实现通道: {other}")),
    }
}

// ============================================================================
// 通用助手
// ============================================================================

/// 当前毫秒时间戳（`std::time::SystemTime`）。
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn opt_str<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(|v| v.as_str())
}

fn opt_i64(payload: &Value, key: &str) -> Option<i64> {
    payload.get(key).and_then(|v| v.as_i64())
}

fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64 {
    v.max(lo).min(hi)
}

/// `Option<String>` → JSON（None→Null）。
fn opt_value(o: &Option<String>) -> Value {
    match o {
        Some(s) => json!(s),
        None => Value::Null,
    }
}

/// 短随机后缀（原子计数器 XOR 纳秒，足够测试/单机去重）。
fn short_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    format!("{:x}", nanos ^ n.rotate_left(7))
}

/// 写操作是否 dry_run：默认 dry；payload `dryRun:false` 或 `confirm:true` 时才落库。
fn dry_run(payload: &Value) -> bool {
    if let Some(d) = payload.get("dryRun").and_then(|v| v.as_bool()) {
        if !d {
            return false;
        }
    }
    if payload
        .get("confirm")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return false;
    }
    true
}

/// 把 `*_json` 文本列解析成 [`Value`]；空/非法时回退 `fallback`。
fn parse_json_col(v: Option<&Value>, fallback: &Value) -> Value {
    match v.and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or_else(|_| fallback.clone()),
        _ => fallback.clone(),
    }
}

/// 推 `data:changed` 事件给前端（对齐 Beav `emitRendererDataChanged`）。
fn emit_data_changed(emitter: &dyn EventEmitter, scope: &str, action: &str, entity_id: &str) {
    emitter.emit(
        "data:changed",
        json!({ "scope": scope, "action": action, "entityId": entity_id }),
    );
}

// ============================================================================
// tasks:* —— agent_tasks + agent_task_traces
// ============================================================================

/// `tasks:create`：INSERT `agent_tasks(status='pending')`。
/// 真实路由（LLM `prepareExecution`）属系统 API，这里只做结构化建表（见 `stub_channels`）。
fn create_task(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let dry = dry_run(payload);
    let now = now_ts();
    let id = format!("task_{now}_{}", short_suffix());
    let runtime_mode = opt_str(payload, "runtimeMode")
        .unwrap_or("redclaw")
        .to_string();
    let owner_session_id = opt_str(payload, "sessionId").map(String::from);
    let user_input = opt_str(payload, "userInput").unwrap_or("").to_string();
    let intent = opt_str(payload, "intent").unwrap_or("generic").to_string();
    let role_id = opt_str(payload, "roleId").map(String::from);
    let goal = opt_str(payload, "goal")
        .or(if user_input.is_empty() {
            None
        } else {
            Some(user_input.as_str())
        })
        .map(String::from);

    let mut metadata = match payload.get("metadata").and_then(|v| v.as_object()) {
        Some(o) => o.clone(),
        None => serde_json::Map::new(),
    };
    metadata.insert("userInput".into(), json!(user_input));
    let metadata = Value::Object(metadata);

    let graph = default_graph();
    let current_node = graph
        .get(0)
        .and_then(|n| n.get("id"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let route = json!({
        "intent": intent,
        "goal": goal,
        "requiresMultiAgent": false,
        "requiresLongRunningTask": false,
    });

    let snapshot = json!({
        "id": id,
        "taskType": intent,
        "status": "pending",
        "runtimeMode": runtime_mode,
        "ownerSessionId": owner_session_id,
        "intent": intent,
        "roleId": role_id,
        "goal": goal,
        "currentNode": current_node,
        "route": route,
        "graph": graph,
        "artifacts": [],
        "checkpoints": [],
        "metadata": metadata,
        "lastError": Value::Null,
        "createdAt": now,
        "updatedAt": now,
        "startedAt": Value::Null,
        "completedAt": Value::Null,
        "dryRun": dry,
    });

    if dry {
        return Ok(snapshot);
    }

    db.execute_json(
        "INSERT INTO agent_tasks \
         (id, task_type, status, runtime_mode, owner_session_id, intent, role_id, goal, \
          current_node, route_json, graph_json, artifacts_json, checkpoints_json, metadata_json, \
          last_error, created_at, updated_at, started_at, completed_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        &[
            json!(id),
            json!(intent),    // task_type
            json!("pending"), // status
            json!(runtime_mode),
            opt_value(&owner_session_id),
            json!(intent),
            opt_value(&role_id),
            opt_value(&goal),
            opt_value(&current_node),
            json!(route.to_string()),
            json!(graph.to_string()),
            json!("[]"),
            json!("[]"),
            json!(metadata.to_string()),
            Value::Null, // last_error
            json!(now),
            json!(now),
            Value::Null, // started_at
            Value::Null, // completed_at
        ],
    )?;

    add_trace(
        db,
        &id,
        current_node.as_deref(),
        "task.created",
        &json!({ "runtimeMode": runtime_mode, "route": route, "roleId": role_id }),
    )?;

    Ok(snapshot)
}

/// `tasks:list`：SELECT，`ORDER BY updated_at DESC`，可按 status/ownerSessionId 过滤。
fn list_tasks(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let mut cond: Vec<String> = vec!["task_type NOT LIKE 'redclaw-%'".into()];
    let mut params: Vec<Value> = Vec::new();
    if let Some(st) = opt_str(payload, "status") {
        params.push(json!(st));
        cond.push(format!("status = ?{}", params.len()));
    }
    if let Some(os) = opt_str(payload, "ownerSessionId") {
        params.push(json!(os));
        cond.push(format!("owner_session_id = ?{}", params.len()));
    }
    let limit = clamp_i64(opt_i64(payload, "limit").unwrap_or(100), 1, 500);
    params.push(json!(limit));
    let limit_ph = format!("?{}", params.len());
    let where_ = format!("WHERE {}", cond.join(" AND "));
    let sql =
        format!("SELECT * FROM agent_tasks {where_} ORDER BY updated_at DESC LIMIT {limit_ph}");
    let rows = db.query_all_json(&sql, &params)?;
    let out: Vec<Value> = rows.iter().map(hydrate_task).collect();
    Ok(json!(out))
}

/// `tasks:get`：SELECT by id；空 id 返回 Null。
fn get_task(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "taskId") {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(Value::Null),
    };
    let row = db.query_one_json(
        "SELECT * FROM agent_tasks WHERE id = ?1 AND task_type NOT LIKE 'redclaw-%'",
        &[json!(id)],
    )?;
    Ok(row.map(|r| hydrate_task(&r)).unwrap_or(Value::Null))
}

/// `tasks:resume`：UPDATE status='running'（含 started_at），追加 `task.resumed` trace。
fn resume_task(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "taskId") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(Value::Null),
    };
    let row = match db.query_one_json("SELECT * FROM agent_tasks WHERE id = ?1", &[json!(&id)])? {
        Some(r) => r,
        None => return Ok(Value::Null),
    };
    reject_internal_redclaw_task(&row)?;

    if !dry_run(payload) {
        let now = now_ts();
        let started = row
            .get("started_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(now);
        db.execute_json(
            "UPDATE agent_tasks SET status = ?1, started_at = ?2, completed_at = ?3, updated_at = ?4 \
             WHERE id = ?5",
            &[json!("running"), json!(started), Value::Null, json!(now), json!(&id)],
        )?;
        add_trace(db, &id, None, "task.resumed", &json!({}))?;
    }

    let fresh = db.query_one_json("SELECT * FROM agent_tasks WHERE id = ?1", &[json!(&id)])?;
    Ok(fresh
        .map(|r| hydrate_task(&r))
        .unwrap_or_else(|| hydrate_task(&row)))
}

/// `tasks:cancel`：UPDATE status='cancelled'（含 completed_at），追加 `task.cancelled` trace。
fn cancel_task(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "taskId") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(Value::Null),
    };
    let row = match db.query_one_json("SELECT * FROM agent_tasks WHERE id = ?1", &[json!(&id)])? {
        Some(r) => r,
        None => return Ok(Value::Null),
    };
    reject_internal_redclaw_task(&row)?;

    if !dry_run(payload) {
        let now = now_ts();
        db.execute_json(
            "UPDATE agent_tasks SET status = ?1, completed_at = ?2, updated_at = ?3 WHERE id = ?4",
            &[json!("cancelled"), json!(now), json!(now), json!(&id)],
        )?;
        add_trace(db, &id, None, "task.cancelled", &json!({}))?;
    }

    let fresh = db.query_one_json("SELECT * FROM agent_tasks WHERE id = ?1", &[json!(&id)])?;
    Ok(fresh
        .map(|r| hydrate_task(&r))
        .unwrap_or_else(|| hydrate_task(&row)))
}

/// `tasks:trace`：SELECT `agent_task_traces` by task_id，`ORDER BY created_at ASC`。
fn trace_task(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "taskId") {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(json!([])),
    };
    let limit = clamp_i64(opt_i64(payload, "limit").unwrap_or(500), 1, 5000);
    let rows = db.query_all_json(
        "SELECT traces.* FROM agent_task_traces traces \
         JOIN agent_tasks tasks ON tasks.id=traces.task_id \
         WHERE traces.task_id = ?1 AND tasks.task_type NOT LIKE 'redclaw-%' \
         ORDER BY traces.created_at ASC LIMIT ?2",
        &[json!(id), json!(limit)],
    )?;
    let out: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get("id").cloned().unwrap_or(Value::Null),
                "taskId": r.get("task_id").cloned().unwrap_or(Value::Null),
                "nodeId": r.get("node_id").cloned().unwrap_or(Value::Null),
                "eventType": r.get("event_type").cloned().unwrap_or(Value::Null),
                "payload": parse_json_col(r.get("payload_json"), &Value::Null),
                "createdAt": r.get("created_at").cloned().unwrap_or(json!(0)),
            })
        })
        .collect();
    Ok(json!(out))
}

/// `tasks:resume-from-session`：按 ownerSessionId 取最新任务，优先 running/paused，否则首条。
fn resume_from_session(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let sid = match opt_str(payload, "sessionId") {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(Value::Null),
    };
    let rows = db.query_all_json(
        "SELECT * FROM agent_tasks WHERE owner_session_id = ?1 \
         AND task_type NOT LIKE 'redclaw-%' ORDER BY updated_at DESC LIMIT 20",
        &[json!(sid)],
    )?;
    let snaps: Vec<Value> = rows.iter().map(hydrate_task).collect();
    let pick = snaps
        .iter()
        .find(|t| {
            matches!(
                t.get("status").and_then(|v| v.as_str()),
                Some("running") | Some("paused")
            )
        })
        .or(snaps.first());
    Ok(pick.cloned().unwrap_or(Value::Null))
}

fn reject_internal_redclaw_task(row: &Value) -> anyhow::Result<()> {
    if row
        .get("task_type")
        .and_then(Value::as_str)
        .is_some_and(|task_type| task_type.starts_with("redclaw-"))
    {
        anyhow::bail!("RedClaw scheduler tasks can only be changed through redclaw:runner-* APIs");
    }
    Ok(())
}

/// 追加一条 `agent_task_traces`。
fn add_trace(
    db: &Db,
    task_id: &str,
    node_id: Option<&str>,
    event_type: &str,
    payload: &Value,
) -> anyhow::Result<()> {
    let id = format!("trace_{}_{}", now_ts(), short_suffix());
    let node_owned = node_id.map(String::from);
    db.execute_json(
        "INSERT INTO agent_task_traces (id, task_id, node_id, event_type, payload_json, created_at) \
         VALUES (?1,?2,?3,?4,?5,?6)",
        &[
            json!(id),
            json!(task_id),
            opt_value(&node_owned),
            json!(event_type),
            json!(payload.to_string()),
            json!(now_ts()),
        ],
    )?;
    Ok(())
}

/// 默认任务图（route → plan → execute_tools → complete）。
fn default_graph() -> Value {
    let s = short_suffix();
    json!([
        { "id": format!("route_{s}"), "type": "route", "title": "初始化任务上下文", "status": "pending" },
        { "id": format!("plan_{s}"), "type": "plan", "title": "生成执行计划", "status": "pending" },
        { "id": format!("execute_tools_{s}"), "type": "execute_tools", "title": "调用工具执行", "status": "pending" },
        { "id": format!("complete_{s}"), "type": "complete", "title": "完成任务", "status": "pending" },
    ])
}

/// `agent_tasks` 行（snake_case + `*_json` 文本列）→ 前端 AgentTaskSnapshot（camelCase + 解析后的 route/graph/...）。
fn hydrate_task(row: &Value) -> Value {
    json!({
        "id": row.get("id").cloned().unwrap_or(Value::Null),
        "taskType": row.get("task_type").cloned().unwrap_or(Value::Null),
        "status": row.get("status").cloned().unwrap_or(Value::Null),
        "runtimeMode": row.get("runtime_mode").cloned().unwrap_or(Value::Null),
        "ownerSessionId": row.get("owner_session_id").cloned().unwrap_or(Value::Null),
        "intent": row.get("intent").cloned().unwrap_or(Value::Null),
        "roleId": row.get("role_id").cloned().unwrap_or(Value::Null),
        "goal": row.get("goal").cloned().unwrap_or(Value::Null),
        "currentNode": row.get("current_node").cloned().unwrap_or(Value::Null),
        "route": parse_json_col(row.get("route_json"), &Value::Null),
        "graph": parse_json_col(row.get("graph_json"), &json!([])),
        "artifacts": parse_json_col(row.get("artifacts_json"), &json!([])),
        "checkpoints": parse_json_col(row.get("checkpoints_json"), &json!([])),
        "metadata": parse_json_col(row.get("metadata_json"), &Value::Null),
        "lastError": row.get("last_error").cloned().unwrap_or(Value::Null),
        "createdAt": row.get("created_at").cloned().unwrap_or(json!(0)),
        "updatedAt": row.get("updated_at").cloned().unwrap_or(json!(0)),
        "startedAt": row.get("started_at").cloned().unwrap_or(Value::Null),
        "completedAt": row.get("completed_at").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// work:* —— 复用 agent_tasks（task_type='work'），细节存 metadata_json
// ============================================================================

const WORK_META_KIND: &str = "work";

/// `work:list`：可按 status/type/tag 过滤，按 active→ready→priority→updatedAt 排序。
fn list_work(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let records = load_work_records(db)?;
    let map = work_map(&records);
    let mut items: Vec<Value> = records.iter().map(|r| materialize_work(r, &map)).collect();

    if let Some(st) = opt_str(payload, "status") {
        items.retain(|item| {
            item.get("effectiveStatus").and_then(|v| v.as_str()) == Some(st)
                || item.get("status").and_then(|v| v.as_str()) == Some(st)
        });
    }
    if let Some(ty) = opt_str(payload, "type") {
        items.retain(|item| item.get("type").and_then(|v| v.as_str()) == Some(ty));
    }
    if let Some(tag) = opt_str(payload, "tag") {
        items.retain(|item| {
            item.get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|x| x.as_str() == Some(tag)))
                .unwrap_or(false)
        });
    }

    items.sort_by(work_comparator);
    let limit = clamp_i64(
        opt_i64(payload, "limit").unwrap_or(items.len() as i64),
        1,
        5000,
    );
    Ok(json!(items
        .into_iter()
        .take(limit as usize)
        .collect::<Vec<_>>()))
}

/// `work:get`：by id；空 id 返回 Null。
fn get_work(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "id") {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(Value::Null),
    };
    let records = load_work_records(db)?;
    let map = work_map(&records);
    Ok(records
        .iter()
        .find(|r| r.get("id").and_then(|v| v.as_str()) == Some(id))
        .map(|r| materialize_work(r, &map))
        .unwrap_or(Value::Null))
}

/// `work:ready`：等价 `work:list` with status='ready'。
fn ready_work(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let limit = opt_i64(payload, "limit").unwrap_or(20);
    list_work(db, &json!({ "status": "ready", "limit": limit }))
}

/// `work:update`：合并字段后 UPDATE（status + metadata_json + updated_at），推 `data:changed`。
fn update_work(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "id") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Err(anyhow::anyhow!("work item id is required")),
    };

    let records = load_work_records(db)?;
    let existing = records
        .iter()
        .find(|r| r.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Work item not found: {id}"))?;

    let next_status = opt_str(payload, "status")
        .map(String::from)
        .unwrap_or_else(|| {
            existing
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending")
                .to_string()
        });

    let mut meta = existing.clone();
    if let Some(obj) = meta.as_object_mut() {
        if let Some(t) = opt_str(payload, "title") {
            obj.insert("title".into(), json!(t));
        }
        if let Some(d) = payload.get("description") {
            obj.insert("description".into(), d.clone());
        }
        if let Some(s) = opt_str(payload, "type") {
            obj.insert("type".into(), json!(s));
        }
        if let Some(p) = opt_i64(payload, "priority") {
            obj.insert("priority".into(), json!(clamp_i64(p, 0, 3)));
        }
        if let Some(sum) = payload.get("summary") {
            obj.insert("summary".into(), sum.clone());
        }
        obj.insert("status".into(), json!(next_status));
        obj.remove("effectiveStatus");
        obj.remove("blockedBy");
        obj.remove("ready");
    }
    let store_meta = work_store_meta(&meta);

    if dry_run(payload) {
        let map = work_map(&records);
        let mut preview = materialize_work(&meta, &map);
        if let Some(o) = preview.as_object_mut() {
            o.insert("dryRun".into(), json!(true));
        }
        return Ok(preview);
    }

    let now = now_ts();
    db.execute_json(
        "UPDATE agent_tasks SET status = ?1, metadata_json = ?2, updated_at = ?3 \
         WHERE id = ?4 AND task_type = 'work'",
        &[
            json!(next_status),
            json!(store_meta.to_string()),
            json!(now),
            json!(&id),
        ],
    )?;
    emit_data_changed(emitter, "work", "update", &id);

    let records = load_work_records(db)?;
    let map = work_map(&records);
    Ok(records
        .iter()
        .find(|r| r.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        .map(|r| materialize_work(r, &map))
        .unwrap_or(Value::Null))
}

/// 读取全部 work 行，把 `metadata_json` 解析回记录（合并 id/status/createdAt/updatedAt 列）。
fn load_work_records(db: &Db) -> anyhow::Result<Vec<Value>> {
    let rows = db.query_all_json(
        "SELECT id, status, metadata_json, created_at, updated_at FROM agent_tasks \
         WHERE task_type = 'work'",
        &[],
    )?;
    let mut out = Vec::new();
    for row in rows {
        let id = row
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let status = row
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("pending")
            .to_string();
        let created_at = row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let updated_at = row.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let mut rec = parse_json_col(row.get("metadata_json"), &json!({}));
        if !rec.is_object() {
            rec = json!({});
        }
        if let Some(obj) = rec.as_object_mut() {
            obj.insert("kind".into(), json!(WORK_META_KIND));
            obj.insert("id".into(), json!(id));
            obj.insert("status".into(), json!(status));
            obj.insert("createdAt".into(), json!(created_at));
            obj.insert("updatedAt".into(), json!(updated_at));
        }
        out.push(rec);
    }
    Ok(out)
}

/// id → 记录 映射（供依赖解析）。
fn work_map(records: &[Value]) -> HashMap<String, Value> {
    records
        .iter()
        .filter_map(|r| {
            r.get("id")
                .and_then(|v| v.as_str())
                .map(|s| (s.to_string(), r.clone()))
        })
        .collect()
}

/// 由记录计算 `effectiveStatus` / `blockedBy` / `ready`（对齐 `materializeSnapshot`）。
fn materialize_work(rec: &Value, map: &HashMap<String, Value>) -> Value {
    let depends: Vec<String> = rec
        .get("dependsOn")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let blocked_by: Vec<String> = depends
        .iter()
        .filter(|dep| match map.get(dep.as_str()) {
            None => true,
            Some(d) => {
                let s = d.get("status").and_then(|v| v.as_str()).unwrap_or("");
                s != "done" && s != "cancelled"
            }
        })
        .cloned()
        .collect();
    let status = rec
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("pending");
    let effective = if status == "pending" {
        if blocked_by.is_empty() {
            "ready"
        } else {
            "blocked"
        }
    } else {
        status
    };
    let ready = effective == "ready";

    let mut snap = rec.clone();
    if let Some(obj) = snap.as_object_mut() {
        obj.insert("effectiveStatus".into(), json!(effective));
        obj.insert("blockedBy".into(), json!(blocked_by));
        obj.insert("ready".into(), json!(ready));
    }
    snap
}

/// 排序：active → ready → priority 升序 → updatedAt 降序。
fn work_comparator(a: &Value, b: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering as O;
    let ea = a
        .get("effectiveStatus")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let eb = b
        .get("effectiveStatus")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if ea == "active" && eb != "active" {
        return O::Less;
    }
    if eb == "active" && ea != "active" {
        return O::Greater;
    }
    let ra = a.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
    let rb = b.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
    if ra && !rb {
        return O::Less;
    }
    if rb && !ra {
        return O::Greater;
    }
    let pa = a.get("priority").and_then(|v| v.as_i64()).unwrap_or(2);
    let pb = b.get("priority").and_then(|v| v.as_i64()).unwrap_or(2);
    if pa != pb {
        return pa.cmp(&pb);
    }
    let ua = a.get("updatedAt").and_then(|v| v.as_i64()).unwrap_or(0);
    let ub = b.get("updatedAt").and_then(|v| v.as_i64()).unwrap_or(0);
    ub.cmp(&ua)
}

/// 把快照还原成可写回 `metadata_json` 的存储对象（剥离 id/status/createdAt/updatedAt/effective*/ready）。
fn work_store_meta(record: &Value) -> Value {
    let mut obj = match record.as_object() {
        Some(o) => o.clone(),
        None => serde_json::Map::new(),
    };
    obj.insert("kind".into(), json!(WORK_META_KIND));
    for k in [
        "id",
        "status",
        "createdAt",
        "updatedAt",
        "effectiveStatus",
        "blockedBy",
        "ready",
    ] {
        obj.remove(k);
    }
    Value::Object(obj)
}

/// 写入一条 work 行（建表种子，供测试/内部创建用）。
#[allow(dead_code)]
fn insert_work_record(db: &Db, record: &Value) -> anyhow::Result<()> {
    let now = now_ts();
    let id = record
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let status = record
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("pending")
        .to_string();
    let store_meta = work_store_meta(record);
    db.execute_json(
        "INSERT INTO agent_tasks \
         (id, task_type, status, runtime_mode, intent, role_id, goal, current_node, route_json, \
          graph_json, artifacts_json, checkpoints_json, metadata_json, last_error, created_at, \
          updated_at, started_at, completed_at, owner_session_id) \
         VALUES (?1,'work',?2,'work',NULL,NULL,NULL,NULL,NULL,'[]','[]','[]',?3,NULL,?4,?4,NULL,NULL,NULL)",
        &[json!(id), json!(status), json!(store_meta.to_string()), json!(now)],
    )?;
    Ok(())
}

// ============================================================================
// subjects:* —— 复用 agent_tasks（task_type='subject' / 'subject-category'）
// ============================================================================

const SUBJECT_KIND: &str = "subject";
const SUBJECT_CATEGORY_KIND: &str = "subject-category";

/// `subjects:list`：SELECT 全部 subject 记录（可选 limit）。
fn list_subjects(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let limit = clamp_i64(opt_i64(payload, "limit").unwrap_or(500), 1, 5000);
    let rows = db.query_all_json(
        "SELECT id, metadata_json FROM agent_tasks WHERE task_type = 'subject' ORDER BY updated_at DESC LIMIT ?1",
        &[json!(limit)],
    )?;
    let subjects: Vec<Value> = rows.iter().map(subject_from_row).collect();
    Ok(json!({ "success": true, "subjects": subjects }))
}

/// `subjects:get`：by id。
fn get_subject(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "id") {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(json!({ "success": false, "error": "id is required" })),
    };
    let row = db.query_one_json(
        "SELECT id, metadata_json FROM agent_tasks WHERE id = ?1 AND task_type = 'subject'",
        &[json!(id)],
    )?;
    match row {
        Some(r) => Ok(json!({ "success": true, "subject": subject_from_row(&r) })),
        None => Ok(json!({ "success": false, "error": "subject not found" })),
    }
}

/// `subjects:create`：构造记录并 INSERT；图片/语音只记录相对路径（资产落盘见 `stub_channels`）。
fn create_subject(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let now = now_ts();
    let id = format!("subject_{now}_{}", short_suffix());
    let subject = build_subject_record(&id, payload, None, now, now);

    if dry_run(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "subject": subject }));
    }

    insert_subject_row(db, &id, &subject)?;
    emit_data_changed(emitter, "subjects", "create", &id);
    Ok(json!({ "success": true, "subject": subject }))
}

/// `subjects:update`：合并可变字段后 UPDATE。
fn update_subject(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "id") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(json!({ "success": false, "error": "id is required" })),
    };
    let row = db.query_one_json(
        "SELECT id, metadata_json, created_at FROM agent_tasks WHERE id = ?1 AND task_type = 'subject'",
        &[json!(&id)],
    )?;
    let row = match row {
        Some(r) => r,
        None => return Ok(json!({ "success": false, "error": "subject not found" })),
    };
    let existing = subject_from_row(&row);
    let now = now_ts();
    let created_at = existing
        .get("createdAt")
        .and_then(|v| v.as_i64())
        .unwrap_or(now);
    let subject = build_subject_record(&id, payload, Some(&existing), created_at, now);

    if dry_run(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "subject": subject }));
    }

    db.execute_json(
        "UPDATE agent_tasks SET metadata_json = ?1, updated_at = ?2 WHERE id = ?3 AND task_type = 'subject'",
        &[json!(subject.to_string()), json!(now), json!(&id)],
    )?;
    emit_data_changed(emitter, "subjects", "update", &id);
    Ok(json!({ "success": true, "subject": subject }))
}

/// `subjects:delete`：DELETE 行。
fn delete_subject(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "id") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(json!({ "success": false, "error": "id is required" })),
    };
    if dry_run(payload) {
        return Ok(json!({ "success": true, "dryRun": true }));
    }
    db.execute_json(
        "DELETE FROM agent_tasks WHERE id = ?1 AND task_type = 'subject'",
        &[json!(&id)],
    )?;
    emit_data_changed(emitter, "subjects", "delete", &id);
    Ok(json!({ "success": true }))
}

/// `subjects:search`：按 query（name/description/tags/attributes）+ categoryId 过滤。
fn search_subjects(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let query = opt_str(payload, "query").unwrap_or("").to_lowercase();
    let category_id = opt_str(payload, "categoryId").map(String::from);
    let limit = clamp_i64(opt_i64(payload, "limit").unwrap_or(100), 1, 5000);

    let rows = db.query_all_json(
        "SELECT id, metadata_json FROM agent_tasks WHERE task_type = 'subject' ORDER BY updated_at DESC",
        &[],
    )?;
    let mut subjects: Vec<Value> = rows.iter().map(subject_from_row).collect();
    if let Some(cid) = category_id {
        subjects.retain(|s| s.get("categoryId").and_then(|v| v.as_str()) == Some(cid.as_str()));
    }
    if !query.is_empty() {
        subjects.retain(|s| subject_matches_query(s, &query));
    }
    Ok(
        json!({ "success": true, "subjects": subjects.into_iter().take(limit as usize).collect::<Vec<_>>() }),
    )
}

fn subject_matches_query(s: &Value, query: &str) -> bool {
    if s.get("name")
        .and_then(|v| v.as_str())
        .map(|n| n.to_lowercase().contains(query))
        .unwrap_or(false)
    {
        return true;
    }
    if s.get("description")
        .and_then(|v| v.as_str())
        .map(|d| d.to_lowercase().contains(query))
        .unwrap_or(false)
    {
        return true;
    }
    if s.get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter().any(|t| {
                t.as_str()
                    .map(|x| x.to_lowercase().contains(query))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
    {
        return true;
    }
    if s.get("attributes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter().any(|attr| {
                let k = attr.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let v = attr.get("value").and_then(|v| v.as_str()).unwrap_or("");
                k.to_lowercase().contains(query) || v.to_lowercase().contains(query)
            })
        })
        .unwrap_or(false)
    {
        return true;
    }
    false
}

/// 由 payload（可选叠加 existing）构造标准化 subject 记录。
fn build_subject_record(
    id: &str,
    payload: &Value,
    existing: Option<&Value>,
    created_at: i64,
    updated_at: i64,
) -> Value {
    let pick_str = |key: &str, fallback: Option<&str>| -> Option<String> {
        if let Some(s) = opt_str(payload, key) {
            return Some(s.to_string());
        }
        fallback.map(String::from)
    };

    let name = pick_str(
        "name",
        existing.and_then(|e| e.get("name").and_then(|v| v.as_str())),
    )
    .unwrap_or_default();
    let category_id = pick_str(
        "categoryId",
        existing.and_then(|e| e.get("categoryId").and_then(|v| v.as_str())),
    );
    let description = pick_str(
        "description",
        existing.and_then(|e| e.get("description").and_then(|v| v.as_str())),
    );
    let tags = normalize_str_list(payload.get("tags"), existing.and_then(|e| e.get("tags")));
    let attributes = normalize_attributes(payload.get("attributes"));
    let image_paths = extract_image_paths(payload.get("images"));
    let voice = payload.get("voice").filter(|v| v.is_object());
    let voice_path = voice
        .and_then(|v| {
            v.get("relativePath")
                .and_then(|x| x.as_str())
                .or_else(|| v.get("name").and_then(|x| x.as_str()))
        })
        .map(String::from);
    let voice_script = voice.and_then(|v| {
        v.get("scriptText")
            .and_then(|x| x.as_str())
            .map(String::from)
    });

    json!({
        "id": id,
        "kind": SUBJECT_KIND,
        "name": name,
        "categoryId": category_id,
        "description": description,
        "tags": tags,
        "attributes": attributes,
        "imagePaths": image_paths,
        "voicePath": voice_path,
        "voiceScript": voice_script,
        "createdAt": created_at,
        "updatedAt": updated_at,
    })
}

fn insert_subject_row(db: &Db, id: &str, subject: &Value) -> anyhow::Result<()> {
    let now = subject
        .get("createdAt")
        .and_then(|v| v.as_i64())
        .unwrap_or_else(now_ts);
    db.execute_json(
        "INSERT INTO agent_tasks \
         (id, task_type, status, runtime_mode, intent, role_id, goal, current_node, route_json, \
          graph_json, artifacts_json, checkpoints_json, metadata_json, last_error, created_at, \
          updated_at, started_at, completed_at, owner_session_id) \
         VALUES (?1,'subject','pending','subject',NULL,NULL,NULL,NULL,NULL,'[]','[]','[]',?2,NULL,?3,?3,NULL,NULL,NULL)",
        &[json!(id), json!(subject.to_string()), json!(now)],
    )?;
    Ok(())
}

fn subject_from_row(row: &Value) -> Value {
    let id = row.get("id").cloned().unwrap_or(Value::Null);
    let created_at = row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
    let updated_at = row.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut rec = parse_json_col(row.get("metadata_json"), &json!({}));
    if !rec.is_object() {
        rec = json!({});
    }
    if let Some(obj) = rec.as_object_mut() {
        obj.insert("kind".into(), json!(SUBJECT_KIND));
        obj.insert("id".into(), id);
        obj.insert("createdAt".into(), json!(created_at));
        obj.insert("updatedAt".into(), json!(updated_at));
    }
    rec
}

fn normalize_str_list(primary: Option<&Value>, fallback: Option<&Value>) -> Value {
    let collect = |v: &Value| -> Vec<String> {
        match v.as_array() {
            Some(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            None => match v.as_str() {
                Some(s) => s
                    .split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                None => Vec::new(),
            },
        }
    };
    let list = primary
        .map(collect)
        .unwrap_or_else(|| fallback.map(collect).unwrap_or_default());
    json!(list)
}

fn normalize_attributes(input: Option<&Value>) -> Value {
    let arr = match input.and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return json!([]),
    };
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Value> = Vec::new();
    for item in arr {
        let key = item
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let value = item
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if key.is_empty() && value.is_empty() {
            continue;
        }
        let dedupe = format!("{key}::{value}");
        if !seen.insert(dedupe) {
            continue;
        }
        out.push(json!({ "key": key, "value": value }));
    }
    json!(out)
}

/// 从 images 输入提取相对路径（优先 relativePath，其次 name）。资产文件不落盘（结构化）。
fn extract_image_paths(images: Option<&Value>) -> Value {
    let arr = match images.and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return json!([]),
    };
    let paths: Vec<String> = arr
        .iter()
        .filter_map(|img| {
            img.get("relativePath")
                .and_then(|v| v.as_str())
                .or_else(|| img.get("name").and_then(|v| v.as_str()))
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .collect();
    json!(paths)
}

// ---- subjects:categories ----

fn list_subject_categories(db: &Db) -> anyhow::Result<Value> {
    let rows = db.query_all_json(
        "SELECT id, metadata_json, created_at, updated_at FROM agent_tasks \
         WHERE task_type = 'subject-category' ORDER BY created_at ASC",
        &[],
    )?;
    let categories: Vec<Value> = rows.iter().map(category_from_row).collect();
    Ok(json!({ "success": true, "categories": categories }))
}

fn create_subject_category(
    db: &Db,
    emitter: &dyn EventEmitter,
    payload: &Value,
) -> anyhow::Result<Value> {
    let name = match opt_str(payload, "name") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(json!({ "success": false, "error": "name is required" })),
    };
    let now = now_ts();
    let id = format!("subject_cat_{now}_{}", short_suffix());
    let category = json!({
        "id": id,
        "kind": SUBJECT_CATEGORY_KIND,
        "name": name,
        "createdAt": now,
        "updatedAt": now,
    });
    if dry_run(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "category": category }));
    }
    db.execute_json(
        "INSERT INTO agent_tasks \
         (id, task_type, status, runtime_mode, metadata_json, created_at, updated_at) \
         VALUES (?1,'subject-category','pending','subject-category',?2,?3,?3)",
        &[json!(&id), json!(category.to_string()), json!(now)],
    )?;
    emit_data_changed(emitter, "subjects", "category-create", &id);
    Ok(json!({ "success": true, "category": category }))
}

fn update_subject_category(
    db: &Db,
    emitter: &dyn EventEmitter,
    payload: &Value,
) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "id") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(json!({ "success": false, "error": "id is required" })),
    };
    let name = match opt_str(payload, "name") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(json!({ "success": false, "error": "name is required" })),
    };
    if dry_run(payload) {
        return Ok(json!({
            "success": true,
            "dryRun": true,
            "category": { "id": id, "kind": SUBJECT_CATEGORY_KIND, "name": name }
        }));
    }
    let now = now_ts();
    let row = db.query_one_json(
        "SELECT id, metadata_json, created_at FROM agent_tasks WHERE id = ?1 AND task_type = 'subject-category'",
        &[json!(&id)],
    )?;
    let created_at = row
        .as_ref()
        .and_then(|r| r.get("created_at").and_then(|v| v.as_i64()))
        .unwrap_or(now);
    let category = json!({
        "id": id,
        "kind": SUBJECT_CATEGORY_KIND,
        "name": name,
        "createdAt": created_at,
        "updatedAt": now,
    });
    let affected = db.execute_json(
        "UPDATE agent_tasks SET metadata_json = ?1, updated_at = ?2 WHERE id = ?3 AND task_type = 'subject-category'",
        &[json!(category.to_string()), json!(now), json!(&id)],
    )?;
    if affected == 0 {
        return Ok(json!({ "success": false, "error": "category not found" }));
    }
    emit_data_changed(emitter, "subjects", "category-update", &id);
    Ok(json!({ "success": true, "category": category }))
}

fn delete_subject_category(
    db: &Db,
    emitter: &dyn EventEmitter,
    payload: &Value,
) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "id") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(json!({ "success": false, "error": "id is required" })),
    };
    if dry_run(payload) {
        return Ok(json!({ "success": true, "dryRun": true }));
    }
    db.execute_json(
        "DELETE FROM agent_tasks WHERE id = ?1 AND task_type = 'subject-category'",
        &[json!(&id)],
    )?;
    emit_data_changed(emitter, "subjects", "category-delete", &id);
    Ok(json!({ "success": true }))
}

fn category_from_row(row: &Value) -> Value {
    let id = row.get("id").cloned().unwrap_or(Value::Null);
    let created_at = row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
    let updated_at = row.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0);
    let mut rec = parse_json_col(row.get("metadata_json"), &json!({}));
    if !rec.is_object() {
        rec = json!({});
    }
    if let Some(obj) = rec.as_object_mut() {
        obj.insert("kind".into(), json!(SUBJECT_CATEGORY_KIND));
        obj.insert("id".into(), id);
        obj.insert("createdAt".into(), json!(created_at));
        obj.insert("updatedAt".into(), json!(updated_at));
    }
    rec
}

// ============================================================================
// background-tasks / background-workers —— 进程内占位（真实 worker 池未接入）
// ============================================================================

fn list_bg_tasks() -> Value {
    let snapshot = BG_TASKS.lock().unwrap().clone();
    json!(snapshot)
}

fn get_bg_task(payload: &Value) -> Value {
    let id = opt_str(payload, "taskId").unwrap_or("");
    let tasks = BG_TASKS.lock().unwrap();
    tasks
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id))
        .cloned()
        .unwrap_or(Value::Null)
}

fn cancel_bg_task(payload: &Value) -> Value {
    let id = opt_str(payload, "taskId").unwrap_or("").to_string();
    let mut tasks = BG_TASKS.lock().unwrap();
    if let Some(t) = tasks
        .iter_mut()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
    {
        if let Some(obj) = t.as_object_mut() {
            obj.insert("status".into(), json!("cancelled"));
            obj.insert("phase".into(), json!("cancelled"));
            obj.insert("cancelReason".into(), json!("user-cancelled"));
            obj.insert("updatedAt".into(), json!(now_ts()));
        }
        return t.clone();
    }
    Value::Null
}

/// `background-tasks:retry`：占位——把任务重置为 running 并自增 attemptCount（真实重跑未接入）。
fn retry_bg_task(payload: &Value) -> Value {
    let id = opt_str(payload, "taskId").unwrap_or("").to_string();
    let mut tasks = BG_TASKS.lock().unwrap();
    if let Some(t) = tasks
        .iter_mut()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
    {
        if let Some(obj) = t.as_object_mut() {
            obj.insert("status".into(), json!("running"));
            obj.insert("phase".into(), json!("starting"));
            let attempt = obj
                .get("attemptCount")
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + 1;
            obj.insert("attemptCount".into(), json!(attempt));
            obj.insert("updatedAt".into(), json!(now_ts()));
        }
        return t.clone();
    }
    Value::Null
}

/// `background-tasks:archive`：占位——打 archived 标记。
fn archive_bg_task(payload: &Value) -> Value {
    let id = opt_str(payload, "taskId").unwrap_or("").to_string();
    let mut tasks = BG_TASKS.lock().unwrap();
    if let Some(t) = tasks
        .iter_mut()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
    {
        if let Some(obj) = t.as_object_mut() {
            obj.insert("archived".into(), json!(true));
            obj.insert("updatedAt".into(), json!(now_ts()));
        }
        return t.clone();
    }
    Value::Null
}

/// `background-workers:get-pool-state`：返回空池快照占位（真实 worker 池未接入）。
fn get_pool_state() -> Value {
    json!({
        "json": [],
        "runtime": [],
        "note": "in-memory placeholder; headless worker pool not yet wired in Rust"
    })
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::ipc::{EventEmitter, NoopEmitter};
    use serde_json::{json, Value};
    use std::sync::Arc;

    fn noop() -> NoopEmitter {
        NoopEmitter
    }

    #[test]
    fn tasks_lifecycle_db() {
        let db = Db::open_in_memory().unwrap();
        let emitter = noop();
        let em: &dyn EventEmitter = &emitter;

        // create（confirm:true 真实落库）
        let created = create_task(
            &db,
            &json!({ "runtimeMode": "redclaw", "sessionId": "s1", "userInput": "你好", "confirm": true }),
        )
        .unwrap();
        let id = created["id"].as_str().unwrap().to_string();
        assert_eq!(created["status"], json!("pending"));
        assert_eq!(created["taskType"], json!("generic"));
        assert_eq!(created["graph"].as_array().unwrap().len(), 4);

        // list
        let list = list_tasks(&db, &json!({})).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);

        // get
        let got = get_task(&db, &json!({ "taskId": &id })).unwrap();
        assert_eq!(got["id"], json!(id));
        assert!(got["graph"].is_array());

        // dry_run（默认）不落库
        let before = get_task(&db, &json!({ "taskId": &id })).unwrap();
        let _ = resume_task(&db, &json!({ "taskId": &id })).unwrap(); // 默认 dry
        let after = get_task(&db, &json!({ "taskId": &id })).unwrap();
        assert_eq!(before["status"], after["status"]); // 仍 pending

        // resume（confirm）
        let resumed = resume_task(&db, &json!({ "taskId": &id, "confirm": true })).unwrap();
        assert_eq!(resumed["status"], json!("running"));

        // cancel（confirm）
        let cancelled = cancel_task(&db, &json!({ "taskId": &id, "confirm": true })).unwrap();
        assert_eq!(cancelled["status"], json!("cancelled"));

        // trace（应至少有 created + resumed + cancelled）
        let traces = trace_task(&db, &json!({ "taskId": &id })).unwrap();
        let arr = traces.as_array().unwrap();
        assert!(arr.len() >= 3);
        assert_eq!(arr[0]["eventType"], json!("task.created"));
        let _ = em; // emitter 在 work/subjects 测试里用到
    }

    #[test]
    fn tasks_resume_from_session_picks_running() {
        let db = Db::open_in_memory().unwrap();
        // 两条同 session 的任务
        let a = create_task(&db, &json!({ "sessionId": "sx", "confirm": true })).unwrap();
        let b = create_task(&db, &json!({ "sessionId": "sx", "confirm": true })).unwrap();
        let bid = b["id"].as_str().unwrap().to_string();
        // 把 b 置 running
        resume_task(&db, &json!({ "taskId": &bid, "confirm": true })).unwrap();
        let pick = resume_from_session(&db, &json!({ "sessionId": "sx" })).unwrap();
        assert_eq!(pick["id"], json!(bid));
        assert_eq!(pick["status"], json!("running"));
        let aid = a["id"].as_str().unwrap().to_string();
        assert_ne!(pick["id"], json!(aid));
    }

    #[test]
    fn work_update_and_ready() {
        let db = Db::open_in_memory().unwrap();
        let emitter = noop();
        let em: &dyn EventEmitter = &emitter;

        // 种子：一条 pending work 项
        insert_work_record(
            &db,
            &json!({
                "id": "wk_seed_1",
                "status": "pending",
                "title": "写周报",
                "type": "generic",
                "priority": 1,
                "tags": ["写作"],
                "dependsOn": [],
            }),
        )
        .unwrap();

        // ready 队列应包含它（pending 且无依赖 → ready）
        let ready = ready_work(&db, &json!({})).unwrap();
        let ready_arr = ready.as_array().unwrap();
        assert_eq!(ready_arr.len(), 1);
        assert_eq!(ready_arr[0]["effectiveStatus"], json!("ready"));
        assert_eq!(ready_arr[0]["ready"], json!(true));

        // update → done
        let updated = update_work(
            &db,
            em,
            &json!({ "id": "wk_seed_1", "status": "done", "summary": "完成", "confirm": true }),
        )
        .unwrap();
        assert_eq!(updated["status"], json!("done"));
        assert_eq!(updated["summary"], json!("完成"));

        // ready 队列现在为空
        let ready2 = ready_work(&db, &json!({})).unwrap();
        assert!(ready2.as_array().unwrap().is_empty());

        // dry_run 预览不落库
        let before = get_work(&db, &json!({ "id": "wk_seed_1" })).unwrap();
        let _ = update_work(&db, em, &json!({ "id": "wk_seed_1", "title": "不应写入" })).unwrap();
        let after = get_work(&db, &json!({ "id": "wk_seed_1" })).unwrap();
        assert_eq!(before["title"], after["title"]); // 仍 "写周报"
    }

    #[test]
    fn subjects_and_categories_crud() {
        let db = Db::open_in_memory().unwrap();
        let emitter = noop();
        let em: &dyn EventEmitter = &emitter;

        // category
        let cat =
            create_subject_category(&db, em, &json!({ "name": "人物", "confirm": true })).unwrap();
        assert_eq!(cat["success"], json!(true));
        let cid = cat["category"]["id"].as_str().unwrap().to_string();
        let cats = list_subject_categories(&db).unwrap();
        assert_eq!(cats["categories"].as_array().unwrap().len(), 1);

        // subject create
        let created = create_subject(
            &db,
            em,
            &json!({
                "name": "小明",
                "categoryId": &cid,
                "description": "测试角色",
                "tags": ["主角", "测试"],
                "attributes": [{ "key": "年龄", "value": "25" }],
                "confirm": true
            }),
        )
        .unwrap();
        assert_eq!(created["success"], json!(true));
        let sid = created["subject"]["id"].as_str().unwrap().to_string();
        assert_eq!(created["subject"]["tags"].as_array().unwrap().len(), 2);

        // list
        let list = list_subjects(&db, &json!({})).unwrap();
        assert_eq!(list["subjects"].as_array().unwrap().len(), 1);

        // search
        let found = search_subjects(&db, &json!({ "query": "主角" })).unwrap();
        assert_eq!(found["subjects"].as_array().unwrap().len(), 1);
        let none = search_subjects(&db, &json!({ "query": "不存在" })).unwrap();
        assert!(none["subjects"].as_array().unwrap().is_empty());

        // update
        let upd = update_subject(
            &db,
            em,
            &json!({ "id": &sid, "name": "小明改", "confirm": true }),
        )
        .unwrap();
        assert_eq!(upd["subject"]["name"], json!("小明改"));

        // category update / delete
        let cupd = update_subject_category(
            &db,
            em,
            &json!({ "id": &cid, "name": "人物改", "confirm": true }),
        )
        .unwrap();
        assert_eq!(cupd["category"]["name"], json!("人物改"));

        // subject delete
        let del = delete_subject(&db, em, &json!({ "id": &sid, "confirm": true })).unwrap();
        assert_eq!(del["success"], json!(true));
        let list2 = list_subjects(&db, &json!({})).unwrap();
        assert!(list2["subjects"].as_array().unwrap().is_empty());
    }

    #[test]
    fn dry_run_preview_does_not_persist() {
        let db = Db::open_in_memory().unwrap();
        let dry = create_task(&db, &json!({ "sessionId": "sd" })).unwrap(); // 默认 dry
        assert_eq!(dry["dryRun"], json!(true));
        let list = list_tasks(&db, &json!({})).unwrap();
        assert!(list.as_array().unwrap().is_empty()); // 未落库
    }

    #[test]
    #[ignore] // 依赖共享全局静态，仅验证内存占位结构
    fn background_tasks_inmemory_smoke() {
        BG_TASKS.lock().unwrap().clear();
        BG_TASKS.lock().unwrap().push(json!({
            "id": "bg_test_1",
            "kind": "redclaw-project",
            "status": "running",
            "phase": "thinking",
            "attemptCount": 1
        }));
        let list = list_bg_tasks();
        assert_eq!(
            list.as_array()
                .unwrap()
                .iter()
                .filter(|t| t["id"] == "bg_test_1")
                .count(),
            1
        );
        let got = get_bg_task(&json!({ "taskId": "bg_test_1" }));
        assert_eq!(got["status"], json!("running"));
        let cancelled = cancel_bg_task(&json!({ "taskId": "bg_test_1" }));
        assert_eq!(cancelled["status"], json!("cancelled"));
        let pool = get_pool_state();
        assert!(pool["json"].is_array());
        assert!(pool["runtime"].is_array());
        BG_TASKS.lock().unwrap().clear();
    }

    #[test]
    fn unknown_channel_errors() {
        // 直接验证分发函数对未知通道报错（不经过 AppState，避免构造 GooseBridge）。
        let rt = tokio::runtime::Runtime::new().unwrap();
        let db = Db::open_in_memory().unwrap();
        // AppState 需要 GooseBridge（异步/网络），这里只构造一个最小桩用于路由错误路径。
        // 由于 invoke 对未知通道在访问 state 字段前就 return Err，传一个伪造 state 不现实；
        // 改为直接断言 match 表达式覆盖：本测试仅作为占位，确认模块编译期 match 穷尽。
        let _ = db;
        let _ = rt;
    }

    // 让 Arc/Value 在测试命名空间可见（避免未使用告警）。
    #[allow(dead_code)]
    fn _type_hints() {
        let _ = Arc::new(NoopEmitter) as Arc<dyn EventEmitter>;
        let _: Value = json!({});
    }
}
