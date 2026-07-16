//! `runtime` 命名空间：会话元数据、运行时驱动、会话桥与 MCP 配置通道。
//!
//! 覆盖四个前缀（按通道全名 match）：
//! - `sessions:*` —— 查询/恢复/分叉 Goose 会话元数据（`chat_sessions` +
//!   `session_transcript_records` + `session_tool_results`）。
//! - `runtime:*` —— 运行时查询/恢复/分叉/轨迹/检查点/工具结果/审批。
//! - `session-bridge:*` —— 原独立 HTTP/WS 桥；嵌入式 goosed 下退化为「经
//!   `chat:send-message` / `runtime:query` 驱动」的结构化状态 + 委托 goose。
//! - `mcp:*` —— 读写 `settings.mcp_servers_json`；真实 MCP 客户端连接
//!   （stdio/sse/streamable-http）为下一步，相关通道返回结构化占位。
//!
//! 由 [`super::dispatch_invoke`] 按前缀 `sessions`/`runtime`/`session-bridge`/`mcp`
//! 路由到本模块 [`invoke`]。本模块只提供双向 [`invoke`]（这些通道在 Beav 中
//! 均为 `ipcMain.handle`，无单向 `send`）。
//!
//! # 表可用性说明
//!
//! Rust schema（`db/schema.rs`）已建：`chat_sessions`、`session_transcript_records`、
//! `session_tool_results`、`agent_tasks(owner_session_id)`。Beav 的 `session_checkpoints`
//! 表尚未迁移到 Rust schema，故 `*:get-checkpoints` / `resume` 的检查点统一返回 `[]`/`null`，
//! `fork` 仅复制消息 + 转录（与 Beav `cloneChatSession` 一致，原本也不复制工具结果）。

use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt;
use serde_json::{json, Value};

use super::core::{bridge_event_payload, runtime_done_payload, runtime_stream_start_payload};
use super::AppState;
use crate::db::Db;
use crate::goose_bridge::BridgeEvent;

/// `sessions` / `runtime` / `session-bridge` / `mcp` 的双向通道分发。
///
/// 按通道**全名** match；未知通道返回 `Err`。DB 操作走 `state.db.query_*/execute_json`；
/// 嵌入式 Goose 回复通过 `state.goose` 起后台任务，事件经 `state.emitter` 以 `runtime:event`
/// 推给前端（对齐 `listen`）。写/真实操作尊重 payload 的 `dryRun`/`dry_run` 字段。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        // ============================ sessions:* ============================
        // 对齐 Beav `sessions:list`：每个会话附加 transcript/checkpoint 计数与 chatSession。
        "sessions:list" => {
            let rows = state.db.query_all_json(
                "SELECT id, title, created_at, updated_at, metadata FROM chat_sessions ORDER BY updated_at DESC",
                &[],
            )?;
            let mut out = Vec::with_capacity(rows.len());
            for s in rows {
                let id = s
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let transcript_count = count_transcript(&state.db, &id)?;
                out.push(json!({
                    "id": s.get("id").cloned().unwrap_or(Value::Null),
                    "transcriptCount": transcript_count,
                    "checkpointCount": 0_i64,
                    "chatSession": s,
                }));
            }
            Ok(json!(out))
        }
        // 对齐 `sessions:get`：聚合 chatSession + transcript + checkpoints + toolResults(200)。
        "sessions:get" => {
            let sid = match session_id_of(&payload) {
                Some(s) => s,
                None => return Ok(Value::Null),
            };
            let chat_session = state.db.query_one_json(
                "SELECT id, title, created_at, updated_at, metadata FROM chat_sessions WHERE id = ?1",
                &[json!(sid)],
            )?;
            let transcript = list_transcript(&state.db, &sid, None)?;
            let tool_results = list_tool_results(&state.db, &sid, Some(200))?;
            Ok(json!({
                "chatSession": chat_session.unwrap_or(Value::Null),
                "transcript": transcript,
                "checkpoints": [],
                "toolResults": tool_results,
            }))
        }
        // 对齐 `sessions:resume`：返回 chatSession + 最近 checkpoint（schema 缺失 → null）。
        "sessions:resume" => {
            let sid = match session_id_of(&payload) {
                Some(s) => s,
                None => return Ok(Value::Null),
            };
            let chat_session = state.db.query_one_json(
                "SELECT id, title, created_at, updated_at, metadata FROM chat_sessions WHERE id = ?1",
                &[json!(sid)],
            )?;
            Ok(json!({
                "chatSession": chat_session.unwrap_or(Value::Null),
                "lastCheckpoint": Value::Null,
            }))
        }
        // 对齐 `sessions:fork`：克隆会话（消息 + 转录）。
        "sessions:fork" => fork_channel(&payload, &state.db, false),
        "sessions:get-transcript" => {
            let sid = match session_id_of(&payload) {
                Some(s) => s,
                None => return Ok(json!([])),
            };
            Ok(json!(list_transcript(
                &state.db,
                &sid,
                opt_limit(&payload)
            )?))
        }
        "sessions:get-tool-results" => {
            let sid = match session_id_of(&payload) {
                Some(s) => s,
                None => return Ok(json!([])),
            };
            Ok(json!(list_tool_results(
                &state.db,
                &sid,
                opt_limit(&payload)
            )?))
        }

        // ============================ runtime:* ============================
        // 对齐 `runtime:query`：嵌入式 Goose 驱动。无 sessionId 则新建；落库用户消息；
        // 起后台流式回复（事件走 `runtime:event`），立即返回 {success, sessionId}。
        "runtime:query" => {
            let message = payload
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("text").and_then(|v| v.as_str()))
                .map(str::trim)
                .unwrap_or("");
            if message.is_empty() {
                return Ok(json!({ "success": false, "error": "message is required" }));
            }
            let session_id =
                session_id_of(&payload).unwrap_or_else(|| format!("session_{}", now_ts()));
            ensure_session_exists(&state.db, &session_id, "New Chat")?;
            if is_dry_run(&payload) {
                add_user_message(&state.db, &session_id, message)?;
                return Ok(json!({
                    "success": true,
                    "dryRun": true,
                    "sessionId": session_id,
                    "note": "dry-run：已记录用户消息，未触发 Goose 回复",
                }));
            }
            dispatch_embedded_reply(state, &session_id, message)?;
            Ok(json!({ "success": true, "sessionId": session_id }))
        }
        // 对齐 `runtime:resume`：返回 sessionId + 最近 checkpoint（schema 缺失 → null）。
        "runtime:resume" => {
            let session_id = match session_id_of(&payload) {
                Some(s) => s,
                None => return Ok(Value::Null),
            };
            Ok(json!({ "sessionId": session_id, "checkpoint": Value::Null }))
        }
        // 对齐 `runtime:fork-session`：返回 {success, session}。
        "runtime:fork-session" => fork_channel(&payload, &state.db, true),
        // 对齐 `runtime:get-trace`：等价 transcript 列表。
        "runtime:get-trace" => {
            let sid = match session_id_of(&payload) {
                Some(s) => s,
                None => return Ok(json!([])),
            };
            Ok(json!(list_transcript(
                &state.db,
                &sid,
                opt_limit(&payload)
            )?))
        }
        // `session_checkpoints` 表尚未迁移到 Rust schema → 返回空。
        "runtime:get-checkpoints" => Ok(json!([])),
        "runtime:get-tool-results" => {
            let sid = match session_id_of(&payload) {
                Some(s) => s,
                None => return Ok(json!([])),
            };
            Ok(json!(list_tool_results(
                &state.db,
                &sid,
                opt_limit(&payload)
            )?))
        }
        // 嵌入式模式不维护待审批队列（工具确认由 Goose 内部处理）。
        "runtime:list-approvals" => Ok(json!([])),

        // ============================ session-bridge:* ============================
        // 嵌入式 goosed：无独立 HTTP/WS 桥，返回结构化状态指示前端走 chat/runtime 通道。
        "session-bridge:status" => Ok(json!({
            "enabled": true,
            "listening": false,
            "embedded": true,
            "runtime": "embedded",
            "note": "嵌入式 goosed：会话经 chat:send-message / runtime:query 驱动",
            "subscriberCount": 0_i64,
            "lastError": Value::Null,
        })),
        // 对齐 `session-bridge:list-sessions`：列出会话 + 元数据派生字段 + owner 任务计数。
        "session-bridge:list-sessions" => {
            let rows = state.db.query_all_json(
                "SELECT id, title, created_at, updated_at, metadata FROM chat_sessions ORDER BY updated_at DESC",
                &[],
            )?;
            let mut out = Vec::with_capacity(rows.len());
            for s in rows {
                out.push(session_summary(&s, &state.db)?);
            }
            Ok(json!(out))
        }
        // 对齐 `session-bridge:get-session`：聚合 session + transcript + toolResults + tasks。
        "session-bridge:get-session" => {
            let sid = match session_id_of(&payload) {
                Some(s) => s,
                None => return Ok(Value::Null),
            };
            let session = state.db.query_one_json(
                "SELECT id, title, created_at, updated_at, metadata FROM chat_sessions WHERE id = ?1",
                &[json!(sid)],
            )?;
            let session = match session {
                Some(s) => s,
                None => return Ok(Value::Null),
            };
            let summary = session_summary(&session, &state.db)?;
            let transcript = list_transcript(&state.db, &sid, None)?;
            let tool_results = list_tool_results(&state.db, &sid, Some(200))?;
            let tasks = state.db.query_all_json(
                "SELECT * FROM agent_tasks WHERE owner_session_id = ?1 ORDER BY updated_at DESC",
                &[json!(sid)],
            )?;
            let meta = parse_metadata(session.get("metadata").and_then(|v| v.as_str()));
            Ok(json!({
                "session": {
                    "id": summary.get("id").cloned().unwrap_or(Value::Null),
                    "title": summary.get("title").cloned().unwrap_or(Value::Null),
                    "createdAt": summary.get("createdAt").cloned().unwrap_or(Value::Null),
                    "updatedAt": summary.get("updatedAt").cloned().unwrap_or(Value::Null),
                    "contextType": summary.get("contextType").cloned().unwrap_or(json!("chat")),
                    "runtimeMode": summary.get("runtimeMode").cloned().unwrap_or(json!("redclaw")),
                    "isBackgroundSession": summary.get("isBackgroundSession").cloned().unwrap_or(json!(false)),
                    "ownerTaskCount": tasks.len(),
                    "backgroundTaskCount": 0_i64,
                    "metadata": meta,
                },
                "transcript": transcript,
                "checkpoints": [],
                "toolResults": tool_results,
                "tasks": tasks,
                "backgroundTasks": [],
                "permissionRequests": [],
            }))
        }
        // 对齐 `session-bridge:create-session`：建会话并写桥来源元数据。
        "session-bridge:create-session" => {
            let title = opt_str(&payload, "title").unwrap_or("Bridge Session");
            let context_type = opt_str(&payload, "contextType").unwrap_or("redclaw");
            let runtime_mode = opt_str(&payload, "runtimeMode").unwrap_or("redclaw");
            let sid = next_id("session_bridge");
            let mut meta = serde_json::Map::new();
            meta.insert("contextType".into(), json!(context_type));
            meta.insert("runtimeMode".into(), json!(runtime_mode));
            meta.insert("createdBy".into(), json!("session-bridge"));
            if let Some(extra) = payload.get("metadata").and_then(|v| v.as_object()) {
                for (k, v) in extra {
                    meta.insert(k.clone(), v.clone());
                }
            }
            let metadata = Value::Object(meta);
            let ts = now_ts();
            state.db.execute_json(
                "INSERT INTO chat_sessions (id, title, created_at, updated_at, metadata) \
                 VALUES (?1, ?2, ?3, ?3, ?4)",
                &[
                    json!(sid),
                    json!(title),
                    json!(ts),
                    json!(metadata.to_string()),
                ],
            )?;
            Ok(json!({
                "id": sid,
                "title": title,
                "createdAt": ts,
                "updatedAt": ts,
                "contextType": context_type,
                "runtimeMode": runtime_mode,
                "isBackgroundSession": false,
                "ownerTaskCount": 0_i64,
                "backgroundTaskCount": 0_i64,
            }))
        }
        // 对齐 `session-bridge:send-message`：委托嵌入式 goose（复用 chat:send-message 模式）。
        "session-bridge:send-message" => {
            let session_id = match session_id_of(&payload) {
                Some(s) => s,
                None => {
                    return Ok(
                        json!({ "accepted": false, "error": "sessionId and message are required" }),
                    )
                }
            };
            let message = payload
                .get("message")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            if message.is_empty() {
                return Ok(
                    json!({ "accepted": false, "error": "sessionId and message are required" }),
                );
            }
            let exists = state.db.query_one_json(
                "SELECT id FROM chat_sessions WHERE id = ?1",
                &[json!(session_id)],
            )?;
            if exists.is_none() {
                return Ok(
                    json!({ "accepted": false, "error": format!("Session not found: {session_id}") }),
                );
            }
            if is_dry_run(&payload) {
                add_user_message(&state.db, &session_id, message)?;
                return Ok(json!({ "accepted": true, "dryRun": true, "sessionId": session_id }));
            }
            dispatch_embedded_reply(state, &session_id, message)?;
            Ok(json!({ "accepted": true, "sessionId": session_id }))
        }
        // 嵌入式模式不维护待审批权限请求队列。
        "session-bridge:list-permissions" => Ok(json!([])),
        "session-bridge:resolve-permission" => {
            if opt_str(&payload, "requestId").is_none() {
                return Ok(json!({ "success": false, "error": "requestId is required" }));
            }
            Ok(json!({
                "success": false,
                "error": "permission request not found",
                "note": "嵌入式模式无待审批权限请求",
            }))
        }

        // ============================ mcp:* ============================
        // 对齐 `mcp:list`：读取 settings.mcp_servers_json（JSON 数组）。
        "mcp:list" => Ok(json!({ "success": true, "servers": read_mcp_servers(&state.db)? })),
        // 对齐 `mcp:save`：序列化写入 settings.mcp_servers_json（白名单列已含）。
        "mcp:save" => {
            let servers = payload.get("servers").cloned().unwrap_or(json!([]));
            let normalized = normalize_servers(&servers);
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true, "servers": normalized }));
            }
            let payload_str = serde_json::to_string(&normalized)?;
            state.db.execute_json(
                "UPDATE settings SET mcp_servers_json = ?1 WHERE id = 1",
                &[json!(payload_str)],
            )?;
            Ok(json!({ "success": true, "servers": normalized }))
        }
        // 真实 MCP 客户端连接为下一步 —— 结构化占位。
        "mcp:test" => {
            if payload.get("server").is_none() {
                return Ok(json!({ "success": false, "message": "server is required" }));
            }
            Ok(mcp_unimplemented(
                "mcp:test",
                "MCP 客户端连接测试（stdio/sse/streamable-http）待接入真实 MCP client",
            ))
        }
        "mcp:list-tools" => {
            if payload.get("server").is_none() {
                return Ok(json!({ "success": false, "error": "server is required", "tools": [] }));
            }
            Ok(json!({
                "success": false,
                "implemented": false,
                "todo": true,
                "tools": [],
                "message": "MCP tools/list 待接入真实 MCP client",
            }))
        }
        "mcp:call" => {
            if payload.get("server").is_none() {
                return Ok(json!({ "success": false, "error": "server is required" }));
            }
            let tool_name = payload
                .get("toolName")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("name").and_then(|v| v.as_str()))
                .unwrap_or("")
                .trim()
                .to_string();
            if tool_name.is_empty() {
                return Ok(json!({ "success": false, "error": "toolName is required" }));
            }
            Ok(mcp_unimplemented(
                "mcp:call",
                "MCP tools/call 待接入真实 MCP client",
            ))
        }
        "mcp:sessions" => Ok(json!({
            "success": true,
            "sessions": [],
            "implemented": false,
            "todo": true,
            "message": "MCP 会话跟踪待接入",
        })),
        "mcp:disconnect" => Ok(json!({ "success": true })),
        "mcp:discover-local" => Ok(json!({
            "success": false,
            "implemented": false,
            "todo": true,
            "items": [],
            "message": "扫描本地 MCP 配置（~/.config 等路径）待实现",
        })),
        "mcp:import-local" => Ok(mcp_unimplemented(
            "mcp:import-local",
            "导入本地 MCP 配置待实现",
        )),
        "mcp:oauth-status" => {
            if opt_str(&payload, "serverId").is_none() {
                return Ok(json!({ "success": false, "error": "serverId is required" }));
            }
            Ok(mcp_unimplemented(
                "mcp:oauth-status",
                "MCP OAuth 状态查询待接入",
            ))
        }

        other => Err(anyhow::anyhow!("runtime 命名空间未实现通道: {other}")),
    }
}

// ---------------------------------------------------------------------------
// 嵌入式 Goose 回复（runtime:query / session-bridge:send-message 共用）
// ---------------------------------------------------------------------------

/// 落库用户消息并起后台 Goose 回复流：事件以 `runtime:event` 推给前端
/// （`stream-start` → 若干 `text-delta`/`done` → 可能 `error`）。
///
/// 复用 `ipc::core` 的 `chat:send-message` 模式：`goose.reply` 返回的事件经
/// [`bridge_event_payload`] 规整为前端信封。`state` 仅借用 —— `goose`/`emitter`
/// 均可廉价 clone 后 move 进后台任务。
fn dispatch_embedded_reply(
    state: &AppState,
    session_id: &str,
    message: &str,
) -> anyhow::Result<()> {
    add_user_message(&state.db, session_id, message)?;
    let goose = state.goose.clone();
    let emitter = state.emitter.clone();
    let db = state.db.clone();
    let sid = session_id.to_string();
    let msg = message.to_string();
    tokio::spawn(async move {
        emitter.emit("runtime:event", runtime_stream_start_payload(&sid));
        match goose.reply(&msg).await {
            Ok(stream) => {
                tokio::pin!(stream);
                let mut assistant_content = String::new();
                while let Some(event) = stream.next().await {
                    match &event {
                        BridgeEvent::TextDelta(text) => {
                            assistant_content.push_str(text);
                            emitter.emit("runtime:event", bridge_event_payload(&sid, &event));
                        }
                        BridgeEvent::Done => {
                            if !assistant_content.trim().is_empty() {
                                let timestamp = now_ts();
                                let _ = db.execute_json(
                                    "INSERT INTO chat_messages \
                                     (id, session_id, role, content, timestamp) \
                                     VALUES (?1, ?2, 'assistant', ?3, ?4)",
                                    &[
                                        json!(format!(
                                            "a-{timestamp}-{}",
                                            uuid::Uuid::new_v4().simple()
                                        )),
                                        json!(sid.clone()),
                                        json!(assistant_content.clone()),
                                        json!(timestamp),
                                    ],
                                );
                            }
                            emitter.emit(
                                "runtime:event",
                                runtime_done_payload(&sid, &assistant_content, "completed", ""),
                            );
                        }
                        _ => {
                            emitter.emit("runtime:event", bridge_event_payload(&sid, &event));
                        }
                    }
                }
            }
            Err(error) => {
                emitter.emit(
                    "runtime:event",
                    runtime_done_payload(&sid, &error.to_string(), "failed", "provider_error"),
                );
            }
        }
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// 会话/分叉
// ---------------------------------------------------------------------------

/// `sessions:fork` / `runtime:fork-session` 共用：校验 sessionId → 克隆会话。
/// `wrap_session` 为 true 时返回 `{success, session}`（runtime 形态），否则 `{success, session}`
/// （sessions 形态二者结构一致，保留参数以对齐两个通道的语义文档）。
fn fork_channel(payload: &Value, db: &Db, _wrap_session: bool) -> anyhow::Result<Value> {
    let source = match session_id_of(payload) {
        Some(s) => s,
        None => {
            return Ok(json!({ "success": false, "error": "sessionId is required" }));
        }
    };
    let title = opt_str(payload, "title");
    if is_dry_run(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "wouldForkFrom": source }));
    }
    let session = fork_session(db, &source, title)?;
    Ok(json!({ "success": true, "session": session }))
}

/// 克隆会话：新建 id，复制 chat_messages + session_transcript_records
/// （对齐 Beav `cloneChatSession`；`session_checkpoints` 表缺失故跳过，工具结果原本不复制）。
fn fork_session(db: &Db, source: &str, title: Option<&str>) -> anyhow::Result<Value> {
    let source_row = db.query_one_json(
        "SELECT id, title, metadata FROM chat_sessions WHERE id = ?1",
        &[json!(source)],
    )?;
    let source_row =
        source_row.ok_or_else(|| anyhow::anyhow!("Source chat session not found: {source}"))?;

    let new_id = next_id("session");
    let new_title = title.map(str::to_string).or_else(|| {
        source_row
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    });
    let ts = now_ts();
    db.execute_json(
        "INSERT INTO chat_sessions (id, title, created_at, updated_at, metadata) \
         VALUES (?1, ?2, ?3, ?3, ?4)",
        &[
            json!(new_id),
            json!(new_title),
            json!(ts),
            source_row.get("metadata").cloned().unwrap_or(Value::Null),
        ],
    )?;

    // 复制消息
    let msgs = db.query_all_json(
        "SELECT role, content, tool_calls, tool_call_id, timestamp \
         FROM chat_messages WHERE session_id = ?1 ORDER BY timestamp ASC",
        &[json!(source)],
    )?;
    for m in &msgs {
        let mid = next_id("msg");
        db.execute_json(
            "INSERT INTO chat_messages (id, session_id, role, content, tool_calls, tool_call_id, timestamp) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            &[
                json!(mid),
                json!(new_id),
                m.get("role").cloned().unwrap_or(Value::Null),
                m.get("content").cloned().unwrap_or(Value::Null),
                m.get("tool_calls").cloned().unwrap_or(Value::Null),
                m.get("tool_call_id").cloned().unwrap_or(Value::Null),
                m.get("timestamp").cloned().unwrap_or(json!(ts)),
            ],
        )?;
    }

    // 复制转录
    let transcripts = db.query_all_json(
        "SELECT record_type, role, content, payload_json \
         FROM session_transcript_records WHERE session_id = ?1 ORDER BY created_at ASC",
        &[json!(source)],
    )?;
    for t in &transcripts {
        let tid = next_id("transcript");
        db.execute_json(
            "INSERT INTO session_transcript_records \
             (id, session_id, record_type, role, content, payload_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            &[
                json!(tid),
                json!(new_id),
                t.get("record_type").cloned().unwrap_or(Value::Null),
                t.get("role").cloned().unwrap_or(Value::Null),
                t.get("content").cloned().unwrap_or(Value::Null),
                t.get("payload_json").cloned().unwrap_or(Value::Null),
                json!(ts),
            ],
        )?;
    }

    let transcript_count = transcripts.len() as i64;
    Ok(json!({
        "id": new_id,
        "transcriptCount": transcript_count,
        "checkpointCount": 0_i64,
    }))
}

/// 由 chat_sessions 行派生 session-bridge 摘要（含 owner 任务计数）。
fn session_summary(row: &Value, db: &Db) -> anyhow::Result<Value> {
    let id = row
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let meta = parse_metadata(row.get("metadata").and_then(|v| v.as_str()));
    let context_type = meta
        .get("contextType")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("chat");
    let runtime_mode = meta
        .get("runtimeMode")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("redclaw");
    let is_background = meta
        .get("isBackgroundSession")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let owner_task_count = count_owner_tasks(db, &id)?;
    Ok(json!({
        "id": row.get("id").cloned().unwrap_or(Value::Null),
        "title": row.get("title").cloned().unwrap_or(Value::Null),
        "createdAt": row.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": row.get("updated_at").cloned().unwrap_or(Value::Null),
        "contextType": context_type,
        "runtimeMode": runtime_mode,
        "isBackgroundSession": is_background,
        "ownerTaskCount": owner_task_count,
        "backgroundTaskCount": 0_i64,
    }))
}

// ---------------------------------------------------------------------------
// 转录 / 工具结果查询
// ---------------------------------------------------------------------------

/// 列出会话转录（对齐 `listTranscript`：payload_json 解析为对象，键名 camelCase）。
fn list_transcript(db: &Db, session_id: &str, limit: Option<i64>) -> anyhow::Result<Vec<Value>> {
    let rows = match limit {
        Some(l) => db.query_all_json(
            "SELECT id, session_id, record_type, role, content, payload_json, created_at \
             FROM session_transcript_records WHERE session_id = ?1 ORDER BY created_at ASC LIMIT ?2",
            &[json!(session_id), json!(l)],
        )?,
        None => db.query_all_json(
            "SELECT id, session_id, record_type, role, content, payload_json, created_at \
             FROM session_transcript_records WHERE session_id = ?1 ORDER BY created_at ASC",
            &[json!(session_id)],
        )?,
    };
    Ok(rows.into_iter().map(map_transcript_row).collect())
}

fn map_transcript_row(row: Value) -> Value {
    let payload = row
        .get("payload_json")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null);
    json!({
        "id": row.get("id").cloned().unwrap_or(Value::Null),
        "sessionId": row.get("session_id").cloned().unwrap_or(Value::Null),
        "recordType": row.get("record_type").cloned().unwrap_or(Value::Null),
        "role": row.get("role").cloned().unwrap_or(Value::Null),
        "content": row.get("content").cloned().unwrap_or(Value::Null),
        "payload": payload,
        "createdAt": row.get("created_at").cloned().unwrap_or(Value::Null),
    })
}

/// 列出会话工具结果（对齐 `ToolResultStore::list`：success/truncated 转 bool，键名 camelCase）。
fn list_tool_results(db: &Db, session_id: &str, limit: Option<i64>) -> anyhow::Result<Vec<Value>> {
    let rows = match limit {
        Some(l) => db.query_all_json(
            "SELECT id, session_id, call_id, tool_name, command, success, result_text, summary_text, \
                    payload_json, created_at, updated_at \
             FROM session_tool_results WHERE session_id = ?1 ORDER BY created_at DESC LIMIT ?2",
            &[json!(session_id), json!(l)],
        )?,
        None => db.query_all_json(
            "SELECT id, session_id, call_id, tool_name, command, success, result_text, summary_text, \
                    payload_json, created_at, updated_at \
             FROM session_tool_results WHERE session_id = ?1 ORDER BY created_at DESC",
            &[json!(session_id)],
        )?,
    };
    Ok(rows.into_iter().map(map_tool_result_row).collect())
}

fn map_tool_result_row(row: Value) -> Value {
    let payload = row
        .get("payload_json")
        .and_then(|v| v.as_str())
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null);
    let success = row
        .get("success")
        .and_then(|v| v.as_i64())
        .map(|n| n != 0)
        .unwrap_or(false);
    json!({
        "id": row.get("id").cloned().unwrap_or(Value::Null),
        "sessionId": row.get("session_id").cloned().unwrap_or(Value::Null),
        "callId": row.get("call_id").cloned().unwrap_or(Value::Null),
        "toolName": row.get("tool_name").cloned().unwrap_or(Value::Null),
        "command": row.get("command").cloned().unwrap_or(Value::Null),
        "success": success,
        "resultText": row.get("result_text").cloned().unwrap_or(Value::Null),
        "summaryText": row.get("summary_text").cloned().unwrap_or(Value::Null),
        "payload": payload,
        "createdAt": row.get("created_at").cloned().unwrap_or(Value::Null),
        "updatedAt": row.get("updated_at").cloned().unwrap_or(Value::Null),
    })
}

// ---------------------------------------------------------------------------
// MCP 配置（settings.mcp_servers_json）
// ---------------------------------------------------------------------------

/// 读取 settings.mcp_servers_json 为 JSON 数组（损坏/空 → 空数组）。
fn read_mcp_servers(db: &Db) -> anyhow::Result<Vec<Value>> {
    let row = db.query_one_json("SELECT mcp_servers_json FROM settings WHERE id = 1", &[])?;
    let raw = row
        .and_then(|r| {
            r.get("mcp_servers_json")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str::<Vec<Value>>(&raw).unwrap_or_default())
}

/// 规范化 MCP servers：仅保留对象元素（其余忽略，对齐前端容错）。
fn normalize_servers(value: &Value) -> Vec<Value> {
    value
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(Value::is_object)
        .collect()
}

/// MCP 未实现通道的结构化占位。
fn mcp_unimplemented(channel: &str, detail: &str) -> Value {
    json!({
        "success": false,
        "implemented": false,
        "todo": true,
        "channel": channel,
        "message": detail,
    })
}

// ---------------------------------------------------------------------------
// 通用助手
// ---------------------------------------------------------------------------

fn ensure_session_exists(db: &Db, session_id: &str, title: &str) -> anyhow::Result<()> {
    let exists = db.query_one_json(
        "SELECT id FROM chat_sessions WHERE id = ?1",
        &[json!(session_id)],
    )?;
    if exists.is_none() {
        let ts = now_ts();
        db.execute_json(
            "INSERT INTO chat_sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            &[json!(session_id), json!(title), json!(ts)],
        )?;
    }
    Ok(())
}

fn add_user_message(db: &Db, session_id: &str, content: &str) -> anyhow::Result<()> {
    let ts = now_ts();
    db.execute_json(
        "INSERT INTO chat_messages (id, session_id, role, content, timestamp) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        &[
            json!(format!("msg_{ts}")),
            json!(session_id),
            json!("user"),
            json!(content),
            json!(ts),
        ],
    )?;
    db.execute_json(
        "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
        &[json!(ts), json!(session_id)],
    )?;
    Ok(())
}

fn count_transcript(db: &Db, session_id: &str) -> anyhow::Result<i64> {
    count_sql(
        db,
        "SELECT COUNT(*) AS c FROM session_transcript_records WHERE session_id = ?1",
        session_id,
    )
}

fn count_owner_tasks(db: &Db, session_id: &str) -> anyhow::Result<i64> {
    count_sql(
        db,
        "SELECT COUNT(*) AS c FROM agent_tasks WHERE owner_session_id = ?1",
        session_id,
    )
}

fn count_sql(db: &Db, sql: &str, session_id: &str) -> anyhow::Result<i64> {
    let row = db.query_one_json(sql, &[json!(session_id)])?;
    Ok(row
        .and_then(|r| r.get("c").and_then(|v| v.as_i64()))
        .unwrap_or(0))
}

fn parse_metadata(raw: Option<&str>) -> Value {
    raw.and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(Value::is_object)
        .unwrap_or(json!({}))
}

fn opt_str<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(|v| v.as_str())
}

/// 取 sessionId / session_id，trim 后非空才返回。
fn session_id_of(payload: &Value) -> Option<String> {
    let raw = payload
        .get("sessionId")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("session_id").and_then(|v| v.as_str()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn opt_limit(payload: &Value) -> Option<i64> {
    payload
        .get("limit")
        .and_then(|v| v.as_i64())
        .filter(|l| *l > 0)
}

fn is_dry_run(payload: &Value) -> bool {
    payload
        .get("dryRun")
        .and_then(|v| v.as_bool())
        .or_else(|| payload.get("dry_run").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}

/// 单调递增计数器，配合时间戳生成唯一 id（对齐 Beav `${prefix}_${Date.now()}_${rand}`）。
static COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_id(prefix: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{}_{n}", now_ts())
}

/// 当前时间戳（毫秒）。
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    /// 转录 / 工具结果映射 + LIMIT 查询（in-memory Db）。
    #[test]
    fn transcript_and_tool_result_mapping() {
        let db = Db::open_in_memory().unwrap();
        db.execute_json(
            "INSERT INTO chat_sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            &[json!("s1"), json!("t"), json!(1000)],
        )
        .unwrap();
        db.execute_json(
            "INSERT INTO session_transcript_records \
             (id, session_id, record_type, role, content, payload_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            &[
                json!("tr1"),
                json!("s1"),
                json!("text"),
                json!("assistant"),
                json!("hi"),
                json!(r#"{"k":1}"#),
                json!(2000),
            ],
        )
        .unwrap();
        db.execute_json(
            "INSERT INTO session_tool_results \
             (id, session_id, call_id, tool_name, command, success, result_text, summary_text, \
              payload_json, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            &[
                json!("tool1"),
                json!("s1"),
                json!("call1"),
                json!("bash"),
                json!("ls"),
                json!(1),
                json!("out"),
                json!("ok"),
                json!(r#"{"n":2}"#),
                json!(3000),
            ],
        )
        .unwrap();

        let transcript = list_transcript(&db, "s1", None).unwrap();
        assert_eq!(transcript.len(), 1);
        assert_eq!(transcript[0]["recordType"], json!("text"));
        assert_eq!(transcript[0]["sessionId"], json!("s1"));
        assert_eq!(transcript[0]["payload"]["k"], json!(1));

        // LIMIT 生效
        assert_eq!(list_transcript(&db, "s1", Some(0)).unwrap().len(), 0);

        let tools = list_tool_results(&db, "s1", None).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["callId"], json!("call1"));
        assert_eq!(tools[0]["success"], json!(true));
        assert_eq!(tools[0]["payload"]["n"], json!(2));

        // 未知会话 → 空
        assert!(list_transcript(&db, "nope", None).unwrap().is_empty());
        assert_eq!(count_transcript(&db, "s1").unwrap(), 1);
    }

    /// MCP servers JSON 读写往返 + 规范化容错（in-memory Db，settings 单例行已存在）。
    #[test]
    fn mcp_servers_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert!(read_mcp_servers(&db).unwrap().is_empty());

        let servers = json!([{
            "id": "fs",
            "name": "FS",
            "enabled": true,
            "transport": "stdio",
            "command": "npx"
        }]);
        let normalized = normalize_servers(&servers);
        let payload_str = serde_json::to_string(&normalized).unwrap();
        db.execute_json(
            "UPDATE settings SET mcp_servers_json = ?1 WHERE id = 1",
            &[json!(payload_str)],
        )
        .unwrap();

        let again = read_mcp_servers(&db).unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0]["id"], json!("fs"));
        assert_eq!(again[0]["transport"], json!("stdio"));

        // 损坏 JSON → 空数组
        db.execute_json(
            "UPDATE settings SET mcp_servers_json = ?1 WHERE id = 1",
            &[json!("not-json")],
        )
        .unwrap();
        assert!(read_mcp_servers(&db).unwrap().is_empty());

        // 非数组 / 含非对象元素 → 规范化过滤
        assert!(normalize_servers(&json!("nope")).is_empty());
        assert_eq!(normalize_servers(&json!([1, "x", {"id": "ok"}])).len(), 1);
    }

    /// 分叉会话：复制消息 + 转录，新 id 唯一（in-memory Db）。
    #[test]
    fn fork_session_copies_messages_and_transcript() {
        let db = Db::open_in_memory().unwrap();
        db.execute_json(
            "INSERT INTO chat_sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            &[json!("src"), json!("src-title"), json!(1)],
        )
        .unwrap();
        db.execute_json(
            "INSERT INTO chat_messages (id, session_id, role, content, timestamp) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            &[
                json!("m1"),
                json!("src"),
                json!("user"),
                json!("hello"),
                json!(1),
            ],
        )
        .unwrap();
        db.execute_json(
            "INSERT INTO session_transcript_records \
             (id, session_id, record_type, role, content, payload_json, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            &[
                json!("t1"),
                json!("src"),
                json!("text"),
                json!("user"),
                json!("hello"),
                Value::Null,
                json!(1),
            ],
        )
        .unwrap();

        let forked = fork_session(&db, "src", Some("forked")).unwrap();
        assert_eq!(forked["transcriptCount"], json!(1));
        assert_eq!(forked["checkpointCount"], json!(0));
        let new_id = forked["id"].as_str().unwrap();
        assert_ne!(new_id, "src");

        // 新会话有 1 条消息 + 1 条转录；标题为 "forked"
        let msgs = db
            .query_all_json(
                "SELECT role, content FROM chat_messages WHERE session_id = ?1",
                &[json!(new_id)],
            )
            .unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["content"], json!("hello"));
        assert_eq!(list_transcript(&db, new_id, None).unwrap().len(), 1);

        let title_row = db
            .query_one_json(
                "SELECT title FROM chat_sessions WHERE id = ?1",
                &[json!(new_id)],
            )
            .unwrap()
            .unwrap();
        assert_eq!(title_row["title"], json!("forked"));

        // 源不存在 → 报错
        assert!(fork_session(&db, "missing", None).is_err());
    }

    /// payload 容错助手 + dryRun 识别。
    #[test]
    fn payload_helpers() {
        assert_eq!(
            session_id_of(&json!({"sessionId": "  x  "})),
            Some("x".into())
        );
        assert_eq!(session_id_of(&json!({"session_id": "y"})), Some("y".into()));
        assert_eq!(session_id_of(&json!({"sessionId": "   "})), None);
        assert_eq!(session_id_of(&json!({})), None);

        assert_eq!(opt_limit(&json!({"limit": 5})), Some(5));
        assert_eq!(opt_limit(&json!({"limit": 0})), None);
        assert_eq!(opt_limit(&json!({})), None);

        assert!(is_dry_run(&json!({"dryRun": true})));
        assert!(is_dry_run(&json!({"dry_run": true})));
        assert!(!is_dry_run(&json!({"dryRun": false})));
        assert!(!is_dry_run(&json!({})));

        // fork dry-run 不写库
        let db = Db::open_in_memory().unwrap();
        db.execute_json(
            "INSERT INTO chat_sessions (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            &[json!("src"), json!("t"), json!(1)],
        )
        .unwrap();
        let res = fork_channel(&json!({"sessionId": "src", "dryRun": true}), &db, false).unwrap();
        assert_eq!(res["success"], json!(true));
        assert_eq!(res["dryRun"], json!(true));
        // 仅 src 一个会话（dry-run 未创建分叉）
        let cnt = db
            .query_one_json("SELECT COUNT(*) AS c FROM chat_sessions", &[])
            .unwrap()
            .unwrap();
        assert_eq!(cnt["c"], json!(1));

        // 缺 sessionId → 结构化失败（非 Err）
        let missing = fork_channel(&json!({}), &db, false).unwrap();
        assert_eq!(missing["success"], json!(false));

        // bridge_event_payload 信封形状
        let p = bridge_event_payload("s1", &BridgeEvent::TextDelta("hi".into()));
        assert_eq!(p["eventType"], json!("runtime:text-delta"));
        assert_eq!(p["payload"]["content"], json!("hi"));
        assert_eq!(p["payload"]["stream"], json!("response"));
        let d = bridge_event_payload("s1", &BridgeEvent::Done);
        assert_eq!(d["eventType"], json!("runtime:done"));
        assert_eq!(d["payload"]["status"], json!("completed"));
    }

    /// session-bridge 摘要从 metadata 派生 contextType/runtimeMode + owner 任务计数。
    #[test]
    fn session_summary_derives_metadata() {
        let db = Db::open_in_memory().unwrap();
        db.execute_json(
            "INSERT INTO chat_sessions (id, title, created_at, updated_at, metadata) \
             VALUES (?1, ?2, ?3, ?3, ?4)",
            &[
                json!("s2"),
                json!("t"),
                json!(10),
                json!(r#"{"contextType":"manuscript","runtimeMode":"redclaw","isBackgroundSession":true}"#),
            ],
        )
        .unwrap();
        // 一个归属任务
        db.execute_json(
            "INSERT INTO agent_tasks (id, task_type, status, runtime_mode, owner_session_id, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            &[
                json!("task1"),
                json!("redclaw"),
                json!("running"),
                json!("redclaw"),
                json!("s2"),
                json!(20),
            ],
        )
        .unwrap();
        let row = db
            .query_one_json(
                "SELECT id, title, created_at, updated_at, metadata FROM chat_sessions WHERE id = ?1",
                &[json!("s2")],
            )
            .unwrap()
            .unwrap();
        let summary = session_summary(&row, &db).unwrap();
        assert_eq!(summary["contextType"], json!("manuscript"));
        assert_eq!(summary["runtimeMode"], json!("redclaw"));
        assert_eq!(summary["isBackgroundSession"], json!(true));
        assert_eq!(summary["ownerTaskCount"], json!(1));
    }
}
