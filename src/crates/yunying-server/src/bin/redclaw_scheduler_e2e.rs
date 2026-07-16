//! RedClaw 真实墙钟验收：创建 +1 分钟一次性任务和 +5 分钟长周期任务，
//! 等待 Rust 调度器到点后通过 Goose + operations MCP 真实执行 create_note。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{SecondsFormat, TimeZone, Utc};
use serde_json::{json, Value};
use yunying_server::db::Db;
use yunying_server::goose_bridge::GooseBridge;
use yunying_server::ipc::redclaw_runner::RedClawScheduler;
use yunying_server::ipc::NoopEmitter;

const MAX_WAIT: Duration = Duration::from_secs(8 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(2);
const EARLY_TOLERANCE_MS: i64 = 250;
const MAX_START_LATENESS_MS: i64 = 5_000;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let artifact_root = std::env::var_os("YUNYING_SCHEDULER_E2E_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("e2e-artifacts/2026-07-16-real-business/scheduler"));
    if artifact_root.exists() {
        tokio::fs::remove_dir_all(&artifact_root).await?;
    }
    tokio::fs::create_dir_all(&artifact_root).await?;
    let data_dir = artifact_root.join("business-data");
    let workspace = artifact_root.join("workspace");
    let receipt_dir = artifact_root.join("receipts");
    let goose_root = artifact_root.join("goose-runtime");
    for directory in [&data_dir, &workspace, &receipt_dir, &goose_root] {
        tokio::fs::create_dir_all(directory).await?;
    }
    std::env::set_var("YUNYING_DATA_DIR", &data_dir);
    std::env::set_var("YUNYING_REDCLAW_RECEIPT_DIR", &receipt_dir);
    std::env::set_var("GOOSE_PATH_ROOT", &goose_root);

    let config = yunying_config::load(None)?;
    if config.goose.api_key.trim().is_empty() {
        anyhow::bail!("config.json 缺 goose.api_key，不能执行真实调度验收");
    }
    let bridge = GooseBridge::new(&config).await?;
    let ops_bin = std::env::var_os("YUNYING_OPS_MCP")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/debug/yunying-ops-mcp"));
    if !ops_bin.is_file() {
        anyhow::bail!(
            "缺少 operations MCP：{}；先构建 yunying-ops-mcp",
            ops_bin.display()
        );
    }
    bridge
        .register_operations_mcp(&ops_bin.to_string_lossy())
        .await?;
    let tools = bridge.tool_names().await?;
    if !tools.iter().any(|name| name.contains("create_note")) {
        anyhow::bail!("operations MCP 未暴露 create_note：{}", tools.join(", "));
    }

    let db = Db::open(&artifact_root.join("redconvert.db"))?;
    db.settings().save(&json!({
        "workspace_dir": workspace.to_string_lossy(),
        "active_space_id": "default",
    }))?;
    let scheduler = RedClawScheduler::start(db.clone(), bridge, Arc::new(NoopEmitter)).await?;
    let created_at = Utc::now().timestamp_millis();
    let run_at = Utc
        .timestamp_millis_opt(created_at + 60_000)
        .single()
        .ok_or_else(|| anyhow::anyhow!("无法计算 +1 分钟时间"))?
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let unique = uuid::Uuid::new_v4();
    let once_title = format!("RedClaw +1分钟真实验收 {unique}");
    let long_title = format!("RedClaw +5分钟长任务验收 {unique}");
    let once_prompt = format!(
        "这是定时业务验收。你必须立即调用 create_note 工具，title 严格使用“{once_title}”，body 写入 Markdown，必须包含：计划触发时间 {run_at}、执行类型 once、验收标识 {unique}。必须等待工具返回；只有 persisted=true 才能确认成功，禁止只输出计划或伪造路径。"
    );
    let long_prompt = format!(
        "这是长周期第 1 轮真实业务验收。你必须立即调用 create_note 工具，title 严格使用“{long_title}”，body 写入 Markdown，必须包含：执行类型 long-cycle、轮次 1/1、验收标识 {unique}。必须等待工具返回；只有 persisted=true 才能确认成功，禁止只输出计划或伪造路径。"
    );
    let once = scheduler
        .add_scheduled(&json!({
            "name":"墙钟 +1 分钟验收",
            "mode":"once",
            "prompt":once_prompt,
            "runAt":run_at,
            "requiredTools":["create_note"],
            "allowedTools":["create_note"],
            "enabled":true,
        }))
        .await?;
    let long = scheduler
        .add_long_cycle(&json!({
            "name":"墙钟 +5 分钟长任务验收",
            "objective":"验证长周期任务由 Rust 调度器在 5 分钟后真实调用 Goose 与 MCP，并落盘业务笔记",
            "stepPrompt":long_prompt,
            "intervalMinutes":5,
            "totalRounds":1,
            "requiredTools":["create_note"],
            "allowedTools":["create_note"],
            "enabled":true,
        }))
        .await?;
    let once_id = string_at(&once, &["task", "id"])?;
    let long_id = string_at(&long, &["task", "id"])?;
    write_json(
        &artifact_root.join("definitions.json"),
        &json!({
            "createdAt":created_at,
            "createdAtIso":Utc.timestamp_millis_opt(created_at).single().map(|value| value.to_rfc3339()),
            "once":once,
            "longCycle":long,
            "provider":config.goose.provider,
            "model":config.goose.model,
            "registeredCreateNote":true,
        }),
    )
    .await?;
    println!(
        "SCHEDULED created={} once={} (+1m) long={} (+5m)",
        created_at, once_id, long_id
    );

    let deadline = tokio::time::Instant::now() + MAX_WAIT;
    let mut polls = Vec::new();
    let mut last_console_bucket = -1i64;
    let (once_final, long_final) = loop {
        let now = Utc::now().timestamp_millis();
        let scheduled = scheduler.list_scheduled().await?;
        let long_cycle = scheduler.list_long_cycle().await?;
        let once_task = find_task(&scheduled, &once_id)?;
        let long_task = find_task(&long_cycle, &long_id)?;
        polls.push(json!({
            "observedAt":now,
            "elapsedMs":now-created_at,
            "once":compact_status(&once_task),
            "longCycle":compact_status(&long_task),
        }));
        let bucket = (now - created_at) / 30_000;
        if bucket != last_console_bucket {
            last_console_bucket = bucket;
            println!(
                "POLL elapsed={}s onceLastRun={} longRounds={}",
                (now - created_at) / 1000,
                once_task
                    .get("lastRunAt")
                    .is_some_and(|value| !value.is_null()),
                long_task
                    .get("completedRounds")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
            );
        }
        let once_done = once_task
            .get("lastRunAt")
            .is_some_and(|value| !value.is_null());
        let long_done = long_task
            .get("completedRounds")
            .and_then(Value::as_i64)
            .unwrap_or(0)
            >= 1;
        if once_done && long_done {
            break (once_task, long_task);
        }
        if tokio::time::Instant::now() >= deadline {
            write_json(&artifact_root.join("polls.json"), &json!(polls)).await?;
            anyhow::bail!("8 分钟内未完成 +1/+5 分钟调度任务");
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    write_json(&artifact_root.join("polls.json"), &json!(polls)).await?;

    let once_receipt = verify_task_receipt(&once_final, created_at + 60_000).await?;
    let long_receipt = verify_task_receipt(&long_final, created_at + 300_000).await?;
    let trace_evidence =
        verify_sqlite_traces(db, [(&once_id, &once_receipt), (&long_id, &long_receipt)]).await?;
    let note_files = list_markdown_files(&data_dir.join("operations/notes")).await?;
    if note_files.len() < 2 {
        anyhow::bail!(
            "create_note 工具回执存在，但业务目录仅发现 {} 个 Markdown 文件",
            note_files.len()
        );
    }
    let report = json!({
        "success":true,
        "createdAt":created_at,
        "completedAt":Utc::now().timestamp_millis(),
        "once":{
            "definition":once_final,
            "receipt":once_receipt,
            "expectedFireAt":created_at+60_000,
        },
        "longCycle":{
            "definition":long_final,
            "receipt":long_receipt,
            "expectedFireAt":created_at+300_000,
        },
        "persistedNotes":note_files,
        "sqliteTraceEvidence":trace_evidence,
        "assertions":{
            "wallClockOneMinute":true,
            "wallClockFiveMinutes":true,
            "gooseExecuted":true,
            "createNoteToolCalled":true,
            "toolPersistedTrue":true,
            "sqliteTraceAndJsonReceipt":true,
        }
    });
    write_json(&artifact_root.join("report.json"), &report).await?;
    println!(
        "PASS report={}",
        artifact_root.join("report.json").display()
    );
    Ok(())
}

async fn verify_task_receipt(task: &Value, expected_start: i64) -> anyhow::Result<Value> {
    if task.get("lastError").is_some_and(|value| !value.is_null()) {
        anyhow::bail!("任务执行错误：{}", task["lastError"]);
    }
    let path = task
        .get("lastReceiptPath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("任务没有 lastReceiptPath"))?;
    let receipt: Value = serde_json::from_slice(&tokio::fs::read(&path).await?)?;
    if receipt.get("status").and_then(Value::as_str) != Some("succeeded") {
        anyhow::bail!("执行回执不是 succeeded：{}", receipt);
    }
    let started_at = receipt
        .get("startedAt")
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow::anyhow!("执行回执缺 startedAt"))?;
    if started_at < expected_start - EARLY_TOLERANCE_MS {
        anyhow::bail!("任务过早触发：startedAt={started_at}, expected={expected_start}");
    }
    if started_at > expected_start + MAX_START_LATENESS_MS {
        anyhow::bail!("任务触发过晚：startedAt={started_at}, expected={expected_start}");
    }
    if receipt.get("requiredTools") != Some(&json!(["create_note"]))
        || receipt.get("allowedTools") != Some(&json!(["create_note"]))
        || receipt.get("authorizedTools") != Some(&json!([]))
    {
        anyhow::bail!("执行回执的工具权限不是最小 create_note 范围：{receipt}");
    }
    let tools = receipt
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("执行回执缺 tools"))?;
    if tools.len() != 1 {
        anyhow::bail!("每个验收执行必须且只能调用一次 create_note：{receipt}");
    }
    let create_note = tools
        .first()
        .filter(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains("create_note"))
        })
        .ok_or_else(|| anyhow::anyhow!("真实 Agent 未调用 create_note：{}", receipt))?;
    if !contains_truthy_field(create_note.get("output"), "persisted") {
        anyhow::bail!("create_note 没有返回 persisted=true：{}", create_note);
    }
    Ok(receipt)
}

async fn verify_sqlite_traces(db: Db, tasks: [(&str, &Value); 2]) -> anyhow::Result<Vec<Value>> {
    let first_id = tasks[0].0.to_string();
    let second_id = tasks[1].0.to_string();
    let rows = tokio::task::spawn_blocking(move || {
        db.query_all_json(
            "SELECT task_id,event_type,payload_json,created_at FROM agent_task_traces \
             WHERE task_id IN (?1,?2) ORDER BY created_at",
            &[json!(first_id), json!(second_id)],
        )
    })
    .await
    .map_err(|error| anyhow::anyhow!("读取 SQLite 轨迹任务失败：{error}"))??;

    let mut evidence = Vec::new();
    for (task_id, receipt) in tasks {
        let execution_id = receipt
            .get("executionId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("回执缺 executionId"))?;
        let matching: Vec<&Value> = rows
            .iter()
            .filter(|row| row.get("task_id").and_then(Value::as_str) == Some(task_id))
            .collect();
        if matching.len() != 2 {
            anyhow::bail!(
                "任务 {task_id} 应恰有 start/complete 两条轨迹，实际 {} 条",
                matching.len()
            );
        }
        for event_type in ["execution_started", "execution_completed"] {
            let row = matching
                .iter()
                .find(|row| row.get("event_type").and_then(Value::as_str) == Some(event_type))
                .ok_or_else(|| anyhow::anyhow!("任务 {task_id} 缺 {event_type} 轨迹"))?;
            let payload: Value = serde_json::from_str(
                row.get("payload_json")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("轨迹缺 payload_json"))?,
            )?;
            if payload.get("executionId").and_then(Value::as_str) != Some(execution_id) {
                anyhow::bail!("任务 {task_id} 的 {event_type} 与回执 executionId 不一致");
            }
            if event_type == "execution_completed"
                && payload.get("status").and_then(Value::as_str) != Some("succeeded")
            {
                anyhow::bail!("任务 {task_id} 的 completion 轨迹不是 succeeded");
            }
            evidence.push(json!({
                "taskId":task_id,
                "eventType":event_type,
                "executionId":execution_id,
                "createdAt":row.get("created_at"),
            }));
        }
    }
    Ok(evidence)
}

fn contains_truthy_field(value: Option<&Value>, key: &str) -> bool {
    let Some(value) = value else { return false };
    match value {
        Value::Object(map) => {
            map.get(key).and_then(Value::as_bool) == Some(true)
                || map
                    .values()
                    .any(|value| contains_truthy_field(Some(value), key))
        }
        Value::Array(values) => values
            .iter()
            .any(|value| contains_truthy_field(Some(value), key)),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .is_some_and(|value| contains_truthy_field(Some(&value), key)),
        _ => false,
    }
}

fn find_task(list: &Value, id: &str) -> anyhow::Result<Value> {
    list.get("tasks")
        .and_then(Value::as_array)
        .and_then(|tasks| tasks.iter().find(|task| task.get("id") == Some(&json!(id))))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("找不到任务 {id}"))
}

fn compact_status(task: &Value) -> Value {
    json!({
        "id":task.get("id"),
        "enabled":task.get("enabled"),
        "status":task.get("status"),
        "nextRunAt":task.get("nextRunAt"),
        "lastRunAt":task.get("lastRunAt"),
        "completedRounds":task.get("completedRounds"),
        "lastError":task.get("lastError"),
        "lastReceiptPath":task.get("lastReceiptPath"),
    })
}

fn string_at(value: &Value, path: &[&str]) -> anyhow::Result<String> {
    let mut current = value;
    for key in path {
        current = current
            .get(*key)
            .ok_or_else(|| anyhow::anyhow!("missing JSON field {}", path.join(".")))?;
    }
    current
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("JSON field {} is not a string", path.join(".")))
}

async fn list_markdown_files(directory: &Path) -> anyhow::Result<Vec<String>> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("md") {
            files.push(path.to_string_lossy().to_string());
        }
    }
    files.sort();
    Ok(files)
}

async fn write_json(path: &Path, value: &Value) -> anyhow::Result<()> {
    tokio::fs::write(path, serde_json::to_vec_pretty(value)?).await?;
    Ok(())
}
