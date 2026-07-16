//! 核心命名空间（P0）通道：db/settings、spaces、chat、goose。
//!
//! 由 [`super::dispatch_invoke`] / [`super::dispatch_send`] 按前缀路由到这里。

use std::{collections::HashMap, fs, path::Path, sync::Arc};

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use futures::StreamExt;
use goose::conversation::message::Message;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};

use super::AppState;
use crate::db::{ChatMessage, ChatSession};
use crate::goose_bridge::BridgeEvent;

#[derive(Debug, Clone)]
struct ChatRuntimeState {
    assistant_message_id: String,
    partial_response: String,
    tool_calls: Vec<Value>,
    is_processing: bool,
    status: String,
    updated_at: i64,
}

static CHAT_RUNTIME_STATES: Lazy<RwLock<HashMap<String, ChatRuntimeState>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// db / spaces / chat / goose 的双向通道。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        // ---- db:settings ----
        "db:get-settings" => Ok(state.db.settings().get()?),
        "db:save-settings" => {
            state.db.settings().save(&payload)?;
            let mut response = json!({ "ok": true });
            if payload_touches_goose_settings(&payload) {
                let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
                match state.goose.reload_from_settings(&settings).await {
                    Ok(true) => {
                        response["gooseReloaded"] = json!(true);
                    }
                    Ok(false) => {
                        response["gooseReloaded"] = json!(false);
                    }
                    Err(e) => {
                        eprintln!("[设置] Goose provider 热更新失败：{e}");
                        response["gooseReloaded"] = json!(false);
                        response["gooseReloadError"] = json!(e.to_string());
                    }
                }
                let db = state.db.clone();
                let goose = state.goose.clone();
                tokio::spawn(async move {
                    match crate::ipc::data::refresh_default_ai_source_model_metadata(&db).await {
                        Ok(result) => {
                            if result.get("updated").and_then(Value::as_bool) == Some(true) {
                                if let Ok(settings) = db.settings().get() {
                                    if let Err(error) = goose.reload_from_settings(&settings).await
                                    {
                                        eprintln!(
                                            "[模型] 元数据刷新后的 provider 热更新失败：{error}"
                                        );
                                    }
                                }
                            }
                        }
                        Err(error) => eprintln!("[模型] 后台刷新模型元数据失败：{error}"),
                    }
                });
                response["modelMetadataRefreshScheduled"] = json!(true);
            }
            Ok(response)
        }
        // ---- spaces ----
        "spaces:list" => {
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let active_space_id = settings
                .get("active_space_id")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("default");
            Ok(json!({
                "activeSpaceId": active_space_id,
                "spaces": state.db.spaces().list()?,
            }))
        }
        "spaces:create" => {
            let name = payload
                .as_str()
                .or_else(|| payload.get("name").and_then(|v| v.as_str()))
                .unwrap_or("新空间")
                .trim();
            if name.is_empty() {
                anyhow::bail!("空间名称不能为空");
            }
            if name.chars().count() > 80 {
                anyhow::bail!("空间名称不能超过 80 个字符");
            }
            let id = payload
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("space-{}", now_ts()));
            validate_space_id(&id)?;
            ensure_space_directories(state, &id)?;
            let space = state.db.spaces().create(&id, name)?;
            state.emitter.emit(
                "space:changed",
                json!({ "action": "created", "spaceId": id }),
            );
            Ok(json!({ "success": true, "space": space }))
        }
        "spaces:switch" => {
            let id = payload
                .as_str()
                .or_else(|| payload.get("id").and_then(|v| v.as_str()))
                .or_else(|| payload.get("spaceId").and_then(|v| v.as_str()))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow::anyhow!("缺少空间 ID"))?;
            validate_space_id(id)?;
            if !space_exists(state, id)? {
                anyhow::bail!("空间不存在：{id}");
            }
            ensure_space_directories(state, id)?;
            state
                .db
                .settings()
                .save(&json!({ "active_space_id": id }))?;
            state.emitter.emit(
                "space:changed",
                json!({ "action": "switched", "spaceId": id }),
            );
            Ok(json!({ "success": true, "activeSpaceId": id }))
        }
        "spaces:rename" => {
            let id = need_str(&payload, "id")?.trim();
            let name = need_str(&payload, "name")?.trim();
            validate_space_id(id)?;
            if name.is_empty() {
                anyhow::bail!("空间名称不能为空");
            }
            if name.chars().count() > 80 {
                anyhow::bail!("空间名称不能超过 80 个字符");
            }
            if !space_exists(state, id)? {
                anyhow::bail!("空间不存在：{id}");
            }
            state.db.spaces().rename(id, name)?;
            state.emitter.emit(
                "space:changed",
                json!({ "action": "renamed", "spaceId": id }),
            );
            Ok(json!({ "success": true }))
        }
        // ---- chat sessions / messages ----
        "chat:create-session" => {
            let id = payload
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| format!("chat-{}", now_ts()));
            let title = payload
                .as_str()
                .or_else(|| payload.get("title").and_then(|v| v.as_str()));
            let metadata = payload.get("metadata").and_then(|v| v.as_str());
            Ok(json!(state
                .db
                .chat()
                .create_session(&id, title, metadata)?))
        }
        "chat:list-sessions" | "chat:get-sessions" => Ok(json!(state.db.chat().list_sessions()?)),
        "chat:list-context-sessions" => {
            let context_id = payload
                .get("contextId")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            let context_type = payload
                .get("contextType")
                .and_then(|v| v.as_str())
                .unwrap_or("redclaw");
            let sessions = state.db.chat().list_sessions()?;
            let mut items = Vec::new();
            for session in sessions
                .iter()
                .filter(|s| session_matches_context(s, context_id, context_type))
            {
                items.push(context_session_item(state, session)?);
            }
            Ok(json!(items))
        }
        "chat:get-session" => {
            let id = need_payload_str(&payload, &["id", "sessionId"])?;
            Ok(json!(state.db.chat().get_session(id)?))
        }
        "chat:get-messages" => {
            let id = need_payload_str(&payload, &["sessionId", "id"])?;
            let messages = state
                .db
                .chat()
                .get_messages(id)?
                .into_iter()
                .filter_map(sanitize_chat_message_for_renderer)
                .collect::<Vec<_>>();
            Ok(json!(messages))
        }
        "chat:update-session-metadata" => {
            let id = need_str(&payload, "id")?;
            let metadata = need_str(&payload, "metadata")?;
            state.db.chat().update_session_metadata(id, metadata)?;
            Ok(json!({ "ok": true }))
        }
        "chat:delete-session" => {
            let id = need_payload_str(&payload, &["id", "sessionId"])?;
            let _ = state.goose.cancel_chat_session(id).await;
            CHAT_RUNTIME_STATES.write().remove(id);
            state.db.chat().delete_session(id)?;
            Ok(json!({ "ok": true }))
        }
        "chat:clear-messages" => {
            let id = need_payload_str(&payload, &["sessionId", "id"])?;
            let _ = state.goose.cancel_chat_session(id).await;
            CHAT_RUNTIME_STATES.write().remove(id);
            state.db.execute_json(
                "DELETE FROM chat_messages WHERE session_id = ?1",
                &[json!(id)],
            )?;
            Ok(json!({ "ok": true }))
        }
        // ---- chat:高级会话操作 ----
        "chat:getOrCreateContextSession" => {
            let context_id = payload
                .get("contextId")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            let context_type = payload
                .get("contextType")
                .and_then(|v| v.as_str())
                .unwrap_or("redclaw");
            let title = payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("运营工作台");
            let sessions = state.db.chat().list_sessions()?;
            if let Some(session) = sessions
                .into_iter()
                .find(|s| session_matches_context(s, context_id, context_type))
            {
                return Ok(json!(session));
            }

            Ok(json!(create_context_session(
                state,
                &payload,
                context_id,
                context_type,
                title,
            )?))
        }
        "chat:create-context-session" => {
            let context_id = payload
                .get("contextId")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            let context_type = payload
                .get("contextType")
                .and_then(|v| v.as_str())
                .unwrap_or("redclaw");
            let title = payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("运营工作台");
            Ok(json!(create_context_session(
                state,
                &payload,
                context_id,
                context_type,
                title,
            )?))
        }
        "chat:get-runtime-state" => {
            let session_id = need_payload_str(&payload, &["sessionId", "id"])?;
            let runtime = CHAT_RUNTIME_STATES.read().get(session_id).cloned();
            Ok(match runtime {
                Some(runtime) => json!({
                    "success": true,
                    "sessionId": session_id,
                    "isProcessing": runtime.is_processing,
                    "partialResponse": runtime.partial_response,
                    "updatedAt": runtime.updated_at,
                    "runtimeMode": "goose",
                    "status": runtime.status,
                    "runtimeEvents": [],
                }),
                None => json!({
                    "success": true,
                    "sessionId": session_id,
                    "isProcessing": false,
                    "partialResponse": "",
                    "updatedAt": 0,
                    "runtimeMode": "goose",
                    "status": "idle",
                    "runtimeEvents": [],
                }),
            })
        }
        "chat:get-context-usage" => {
            let session_id = payload
                .as_str()
                .or_else(|| payload.get("sessionId").and_then(Value::as_str))
                .unwrap_or("");
            let estimated_tokens = if session_id.is_empty() {
                0
            } else {
                state
                    .db
                    .chat()
                    .get_messages(session_id)?
                    .iter()
                    .map(estimate_message_tokens)
                    .sum::<usize>()
            };
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let context_limit = crate::goose_bridge::apply_settings_to_config(
                yunying_config::Config::default(),
                &settings,
            )
            .goose
            .context_limit
            .unwrap_or(80_000)
            .max(1);
            let compact_ratio = (estimated_tokens as f64 / context_limit as f64).clamp(0.0, 1.0);
            Ok(json!({
                "success": true,
                "estimatedTotalTokens": estimated_tokens,
                "estimatedEffectiveTokens": estimated_tokens,
                "compactThreshold": context_limit,
                "compactRatio": compact_ratio,
                "compactRounds": 0,
            }))
        }
        "chat:fork-from-message" => {
            let session_id = need_str(&payload, "sessionId")?;
            let title = payload
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Fork");
            let new_id = format!("fork-{session_id}-{}", now_ts());
            let session = state.db.chat().create_session(&new_id, Some(title), None)?;
            Ok(json!({ "success": true, "session": session }))
        }
        "chat:generate-title" => {
            // TODO: 接 AI 生成标题；v1 用截断的第一条消息。
            let msg = payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("新会话");
            let title: String = msg.chars().take(30).collect();
            Ok(json!(title))
        }
        "chat:pick-attachment" => Ok(json!({
            "success": false,
            "canceled": false,
            "reason": "native_picker_must_use_tauri_command",
            "error": "请通过 Tauri pick_files 选择附件",
        })),
        "chat:stage-attachment" | "chat:create-path-attachment" => {
            let source = need_payload_str(&payload, &["path", "sourcePath", "absolutePath"])?;
            stage_attachment_from_path(state, Path::new(source))
        }
        "chat:create-inline-attachment" => create_inline_attachment(state, &payload),
        // ---- goose ----
        "goose:status" => {
            Ok(json!({ "success": true, "status": { "running": true, "runtime": "embedded" } }))
        }
        "goose:start" | "goose:stop" => {
            Ok(json!({ "success": true, "status": { "running": true } }))
        }
        "goose:send-message" => {
            // 同 chat:send-message，经 goose 命名空间。
            let session_id = need_str(&payload, "sessionId")
                .or_else(|_| need_str(&payload, "id"))?
                .to_string();
            let text = need_str(&payload, "text")
                .or_else(|_| need_str(&payload, "message"))?
                .to_string();
            let ts = now_ts();
            let attachments = attachments_from_payload(&payload);
            state.db.chat().add_message(&crate::db::ChatMessage {
                id: format!("u-{ts}-{}", uuid::Uuid::new_v4().simple()),
                session_id: session_id.clone(),
                role: "user".into(),
                content: text.clone(),
                display_content: payload
                    .get("displayContent")
                    .or_else(|| payload.get("display_content"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                attachment: serialize_attachments(&attachments)?,
                tool_calls: None,
                tool_call_id: None,
                timestamp: ts,
            })?;
            let user_message = build_goose_user_message(&text, &attachments)?;
            spawn_goose_reply(state, session_id, user_message);
            Ok(json!({ "success": true }))
        }
        "goose:mcp-read-resource" => {
            let extension_name = payload
                .get("extensionName")
                .and_then(Value::as_str)
                .unwrap_or("openmontage");
            let uri = need_str(&payload, "uri")?;
            if extension_name != "openmontage" || uri != "ui://openmontage/short-drama" {
                anyhow::bail!("MCP App resource is not allowed: {extension_name} {uri}");
            }
            state.goose.read_mcp_resource(extension_name, uri).await
        }
        "goose:mcp-call-tool" => {
            let extension_name = payload
                .get("extensionName")
                .and_then(Value::as_str)
                .unwrap_or("openmontage");
            if extension_name != "openmontage" {
                anyhow::bail!("MCP App extension is not allowed: {extension_name}");
            }
            let requested_name = need_str(&payload, "name")?;
            let local_name = requested_name.rsplit("__").next().unwrap_or(requested_name);
            if !matches!(
                local_name,
                "drama_selection_commit" | "drama_stage_decide" | "drama_ui_refresh"
            ) {
                anyhow::bail!("MCP App tool is not allowed: {local_name}");
            }
            let arguments = payload
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            state
                .goose
                .call_mcp_tool(&format!("{extension_name}__{local_name}"), arguments)
                .await
        }
        "goose:start-video-agent-session" => {
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            match crate::plugins::mount_openmontage(&state.goose, &settings).await {
                Ok(plugin_root) => Ok(json!({
                    "success": true,
                    "sessionId": format!("video-{}", now_ts()),
                    "plugin": "openmontage",
                    "pluginRoot": plugin_root,
                    "runtime": "uv",
                })),
                Err(error) => Ok(json!({
                    "success": false,
                    "error": error.to_string(),
                    "plugin": "openmontage",
                })),
            }
        }
        other => Err(anyhow::anyhow!("核心通道未实现: {other}")),
    }
}

/// chat 的单向通道（fire-and-forget + 推事件）。
pub async fn send(channel: &str, payload: Value, state: Arc<AppState>) -> anyhow::Result<()> {
    match channel {
        "chat:send-message" => {
            let session_id = need_str(&payload, "sessionId")
                .or_else(|_| need_str(&payload, "id"))?
                .to_string();
            let text = need_str(&payload, "text")
                .or_else(|_| need_str(&payload, "message"))
                .or_else(|_| need_str(&payload, "content"))?
                .to_string();
            let ts = now_ts();
            let attachments = attachments_from_payload(&payload);
            // 1) 落库用户消息
            state.db.chat().add_message(&ChatMessage {
                id: format!("u-{ts}-{}", uuid::Uuid::new_v4().simple()),
                session_id: session_id.clone(),
                role: "user".into(),
                content: text.clone(),
                display_content: payload
                    .get("displayContent")
                    .or_else(|| payload.get("display_content"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                attachment: serialize_attachments(&attachments)?,
                tool_calls: None,
                tool_call_id: None,
                timestamp: ts,
            })?;
            // 2) 启动 Goose 回复流，事件经 emitter 推给前端
            let user_message = build_goose_user_message(&text, &attachments)?;
            spawn_goose_reply(&state, session_id, user_message);
            Ok(())
        }
        "chat:cancel" => {
            let session_id = need_payload_str(&payload, &["sessionId", "id"])?;
            let cancelled = state.goose.cancel_chat_session(session_id).await;
            let runtime_snapshot = {
                let mut runtimes = CHAT_RUNTIME_STATES.write();
                if let Some(runtime) = runtimes.get_mut(session_id) {
                    runtime.is_processing = false;
                    runtime.status = "cancelled".into();
                    runtime.updated_at = now_ts();
                    Some(runtime.clone())
                } else {
                    None
                }
            };
            if let Some(runtime) = runtime_snapshot {
                let _ = persist_assistant_draft(
                    &state.db,
                    session_id,
                    &runtime.assistant_message_id,
                    &runtime.partial_response,
                    &runtime.tool_calls,
                    runtime.updated_at,
                );
            }
            state.emitter.emit(
                "runtime:event",
                runtime_checkpoint_payload(session_id, "chat.cancelled", json!({})),
            );
            if !cancelled {
                eprintln!("[聊天] 会话 {session_id} 当前没有可取消的运行");
            }
            Ok(())
        }
        other => Err(anyhow::anyhow!("核心 send 通道未实现: {other}")),
    }
}

fn spawn_goose_reply(state: &AppState, session_id: String, user_message: Message) {
    let goose = state.goose.clone();
    let emitter = state.emitter.clone();
    let db = state.db.clone();
    tokio::spawn(async move {
        let started_at = now_ts();
        let assistant_message_id = format!("a-{started_at}-{}", uuid::Uuid::new_v4().simple());
        let mut assistant_content = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        let mut failed = false;
        let mut cancelled = false;

        CHAT_RUNTIME_STATES.write().insert(
            session_id.clone(),
            ChatRuntimeState {
                assistant_message_id: assistant_message_id.clone(),
                partial_response: String::new(),
                tool_calls: Vec::new(),
                is_processing: true,
                status: "running".into(),
                updated_at: started_at,
            },
        );
        if let Err(error) = persist_assistant_draft(
            &db,
            &session_id,
            &assistant_message_id,
            &assistant_content,
            &tool_calls,
            started_at,
        ) {
            eprintln!("[聊天] 创建流式回复草稿失败：{error}");
        }
        emitter.emit("runtime:event", runtime_stream_start_payload(&session_id));

        match goose
            .reply_message_for_session(&session_id, user_message)
            .await
        {
            Ok(mut stream) => {
                while let Some(event) = stream.next().await {
                    match &event {
                        BridgeEvent::TextDelta(text) => {
                            assistant_content.push_str(text);
                            update_chat_runtime(
                                &session_id,
                                &assistant_content,
                                &tool_calls,
                                true,
                                "running",
                            );
                            if let Err(error) = persist_assistant_draft(
                                &db,
                                &session_id,
                                &assistant_message_id,
                                &assistant_content,
                                &tool_calls,
                                now_ts(),
                            ) {
                                eprintln!("[聊天] 更新流式回复失败：{error}");
                            }
                            emitter
                                .emit("runtime:event", bridge_event_payload(&session_id, &event));
                        }
                        BridgeEvent::ToolStart {
                            call_id,
                            name,
                            input,
                        } => {
                            tool_calls.push(json!({
                                "callId": call_id,
                                "name": name,
                                "input": input,
                                "description": format!("正在调用 {name}"),
                                "status": "running",
                                "startedAt": now_ts(),
                            }));
                            update_chat_runtime(
                                &session_id,
                                &assistant_content,
                                &tool_calls,
                                true,
                                "running",
                            );
                            let _ = persist_assistant_draft(
                                &db,
                                &session_id,
                                &assistant_message_id,
                                &assistant_content,
                                &tool_calls,
                                now_ts(),
                            );
                            emitter
                                .emit("runtime:event", bridge_event_payload(&session_id, &event));
                        }
                        BridgeEvent::ToolEnd {
                            call_id,
                            name,
                            output,
                        } => {
                            let success = output
                                .get("success")
                                .and_then(Value::as_bool)
                                .unwrap_or(true);
                            if let Some(existing) = tool_calls.iter_mut().rev().find(|item| {
                                item.get("callId").and_then(Value::as_str) == Some(call_id.as_str())
                            }) {
                                existing["output"] = output.clone();
                                existing["status"] = json!(if success { "done" } else { "failed" });
                                existing["finishedAt"] = json!(now_ts());
                            } else {
                                tool_calls.push(json!({
                                    "callId": call_id,
                                    "name": name,
                                    "input": {},
                                    "output": output,
                                    "status": if success { "done" } else { "failed" },
                                    "startedAt": now_ts(),
                                    "finishedAt": now_ts(),
                                }));
                            }
                            update_chat_runtime(
                                &session_id,
                                &assistant_content,
                                &tool_calls,
                                true,
                                "running",
                            );
                            let _ = persist_assistant_draft(
                                &db,
                                &session_id,
                                &assistant_message_id,
                                &assistant_content,
                                &tool_calls,
                                now_ts(),
                            );
                            emitter
                                .emit("runtime:event", bridge_event_payload(&session_id, &event));
                        }
                        BridgeEvent::Error {
                            message,
                            detail,
                            category,
                        } => {
                            if failed || cancelled {
                                continue;
                            }
                            failed = true;
                            if assistant_content.trim().is_empty() {
                                assistant_content = message.clone();
                            }
                            update_chat_runtime(
                                &session_id,
                                &assistant_content,
                                &tool_calls,
                                false,
                                "failed",
                            );
                            let _ = persist_assistant_draft(
                                &db,
                                &session_id,
                                &assistant_message_id,
                                &assistant_content,
                                &tool_calls,
                                now_ts(),
                            );
                            emitter.emit(
                                "chat:error",
                                json!({
                                    "sessionId": &session_id,
                                    "title": message,
                                    "message": message,
                                    "detail": detail,
                                    "category": category,
                                    "hint": "本次请求未完成，请重试。",
                                    "retryable": true,
                                }),
                            );
                            emitter.emit(
                                "runtime:event",
                                runtime_done_payload(
                                    &session_id,
                                    &assistant_content,
                                    "failed",
                                    category,
                                ),
                            );
                        }
                        BridgeEvent::Cancelled => {
                            cancelled = true;
                            update_chat_runtime(
                                &session_id,
                                &assistant_content,
                                &tool_calls,
                                false,
                                "cancelled",
                            );
                            let _ = persist_assistant_draft(
                                &db,
                                &session_id,
                                &assistant_message_id,
                                &assistant_content,
                                &tool_calls,
                                now_ts(),
                            );
                            emitter.emit(
                                "runtime:event",
                                runtime_checkpoint_payload(
                                    &session_id,
                                    "chat.cancelled",
                                    json!({}),
                                ),
                            );
                        }
                        BridgeEvent::Done => {
                            if failed || cancelled {
                                continue;
                            }
                            update_chat_runtime(
                                &session_id,
                                &assistant_content,
                                &tool_calls,
                                false,
                                "completed",
                            );
                            if let Err(error) = persist_assistant_draft(
                                &db,
                                &session_id,
                                &assistant_message_id,
                                &assistant_content,
                                &tool_calls,
                                now_ts(),
                            ) {
                                eprintln!("[聊天] 保存 Goose 回复失败：{error}");
                            }
                            emitter.emit(
                                "runtime:event",
                                runtime_done_payload(
                                    &session_id,
                                    &assistant_content,
                                    "completed",
                                    "",
                                ),
                            );
                        }
                        BridgeEvent::ThoughtDelta(_) => {
                            emitter
                                .emit("runtime:event", bridge_event_payload(&session_id, &event));
                        }
                    }
                }
            }
            Err(error) => {
                let message =
                    format!("Goose 回复失败（请检查默认模型、Endpoint 与 API Key）：{error}");
                assistant_content = message.clone();
                update_chat_runtime(
                    &session_id,
                    &assistant_content,
                    &tool_calls,
                    false,
                    "failed",
                );
                let _ = persist_assistant_draft(
                    &db,
                    &session_id,
                    &assistant_message_id,
                    &assistant_content,
                    &tool_calls,
                    now_ts(),
                );
                emitter.emit(
                    "runtime:event",
                    runtime_done_payload(&session_id, &message, "failed", "provider_error"),
                );
                emitter.emit(
                    "chat:error",
                    json!({
                        "sessionId": session_id,
                        "message": message,
                        "raw": error.to_string(),
                        "hint": "请在设置页确认默认 AI 源后重试。",
                    }),
                );
            }
        }
    });
}

fn update_chat_runtime(
    session_id: &str,
    partial_response: &str,
    tool_calls: &[Value],
    is_processing: bool,
    status: &str,
) {
    if let Some(runtime) = CHAT_RUNTIME_STATES.write().get_mut(session_id) {
        runtime.partial_response = partial_response.to_string();
        runtime.tool_calls = tool_calls.to_vec();
        runtime.is_processing = is_processing;
        runtime.status = status.to_string();
        runtime.updated_at = now_ts();
    }
}

fn persist_assistant_draft(
    db: &crate::db::Db,
    session_id: &str,
    message_id: &str,
    content: &str,
    tool_calls: &[Value],
    timestamp: i64,
) -> anyhow::Result<()> {
    db.chat().upsert_message(&ChatMessage {
        id: message_id.to_string(),
        session_id: session_id.to_string(),
        role: "assistant".into(),
        content: content.to_string(),
        display_content: None,
        attachment: None,
        tool_calls: if tool_calls.is_empty() {
            None
        } else {
            Some(serde_json::to_string(tool_calls)?)
        },
        tool_call_id: None,
        timestamp,
    })
}

fn attachments_from_payload(payload: &Value) -> Vec<Value> {
    if let Some(attachments) = payload.get("attachments").and_then(Value::as_array) {
        return attachments
            .iter()
            .filter(|attachment| attachment.is_object())
            .cloned()
            .collect();
    }

    payload
        .get("attachment")
        .filter(|attachment| !attachment.is_null() && attachment.is_object())
        .cloned()
        .into_iter()
        .collect()
}

fn serialize_attachments(attachments: &[Value]) -> anyhow::Result<Option<String>> {
    if attachments.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::to_string(attachments)?))
    }
}

fn build_goose_user_message(text: &str, attachments: &[Value]) -> anyhow::Result<Message> {
    let mut runtime_text = text.trim().to_string();
    let mut image_payloads: Vec<(String, String)> = Vec::new();

    for (index, attachment) in attachments.iter().filter_map(Value::as_object).enumerate() {
        let absolute_path = attachment
            .get("absolutePath")
            .or_else(|| attachment.get("absolute_path"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let original_path = attachment
            .get("originalAbsolutePath")
            .or_else(|| attachment.get("original_absolute_path"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let file_name = attachment
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| absolute_path.and_then(|path| Path::new(path).file_name()?.to_str()))
            .unwrap_or("attachment");
        let kind = attachment
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("binary");
        let mime_type = attachment
            .get("mimeType")
            .or_else(|| attachment.get("mime_type"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| guess_attachment_mime_type(Path::new(file_name), kind));
        let summary = attachment
            .get("summary")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());

        let mut lines = vec![
            String::new(),
            if attachments.len() > 1 {
                format!("[用户上传附件 {}]", index + 1)
            } else {
                "[用户上传附件]".to_string()
            },
            format!("文件名: {file_name}"),
            format!("附件类型: {kind}"),
        ];
        if let Some(path) = absolute_path {
            lines.push(format!("工作暂存路径: {path}"));
        }
        if let Some(path) = original_path {
            if Some(path) != absolute_path {
                lines.push(format!("原始路径: {path}"));
            }
        }
        if let Some(summary) = summary {
            lines.push(format!("附件摘要: {summary}"));
        }
        lines.push(match kind {
            "image" => {
                "请把这张图片作为当前任务的参考图；需要生成图片或视频时，优先使用该暂存路径。"
                    .into()
            }
            "text" => "请先读取暂存文件原文，再基于文件内容回答。".into(),
            _ => "请先检查暂存文件，再决定转录、解析或其他处理方式。".into(),
        });
        runtime_text.push_str(&lines.join("\n"));

        if kind == "image" {
            if let Some(path) = absolute_path {
                if let Ok(bytes) = fs::read(path) {
                    image_payloads.push((BASE64_STANDARD.encode(bytes), mime_type));
                }
            }
        }
    }

    let mut message = Message::user().with_text(if runtime_text.is_empty() {
        "请处理这个附件".to_string()
    } else {
        runtime_text
    });
    for (image, mime_type) in image_payloads {
        message = message.with_image(image, mime_type);
    }
    Ok(message)
}

fn stage_attachment_from_path(state: &AppState, source: &Path) -> anyhow::Result<Value> {
    let source = source.canonicalize()?;
    if !source.is_file() {
        return Ok(json!({ "success": false, "error": "只能上传文件" }));
    }
    let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
    let uploads_dir = crate::workspace::resolve(&settings).redclaw.join("uploads");
    fs::create_dir_all(&uploads_dir)?;

    let original_name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("attachment.bin");
    let safe_name = sanitize_attachment_file_name(original_name);
    let target = uploads_dir.join(format!(
        "{}_{}_{}",
        now_ts(),
        &uuid::Uuid::new_v4().simple().to_string()[..8],
        safe_name
    ));
    fs::copy(&source, &target)?;
    attachment_result(&target, Some(&source), original_name)
}

fn create_inline_attachment(state: &AppState, payload: &Value) -> anyhow::Result<Value> {
    let raw = need_str(payload, "dataUrl")?.trim();
    let Some((header, body)) = raw
        .strip_prefix("data:")
        .and_then(|value| value.split_once(','))
    else {
        return Ok(json!({ "success": false, "error": "无效的粘贴文件数据" }));
    };
    let mut header_parts = header.split(';');
    let mime_type = header_parts
        .next()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("application/octet-stream")
        .trim()
        .to_lowercase();
    let is_base64 = header_parts.any(|value| value.eq_ignore_ascii_case("base64"));
    let bytes = if is_base64 {
        let normalized = body
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        BASE64_STANDARD.decode(normalized)?
    } else {
        urlencoding::decode(body)?.into_owned().into_bytes()
    };
    if bytes.is_empty() {
        return Ok(json!({ "success": false, "error": "粘贴文件为空" }));
    }

    let requested_name = payload
        .get("fileName")
        .or_else(|| payload.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("clipboard");
    let requested_path = Path::new(requested_name);
    let extension = requested_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_lowercase)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| extension_from_mime_type(&mime_type).to_string());
    let stem = requested_path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("clipboard");
    let display_name = if extension.is_empty() {
        sanitize_attachment_file_name(requested_name)
    } else {
        format!("{}.{}", sanitize_attachment_file_name(stem), extension)
    };

    let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
    let uploads_dir = crate::workspace::resolve(&settings).redclaw.join("uploads");
    fs::create_dir_all(&uploads_dir)?;
    let target = uploads_dir.join(format!(
        "{}_{}_{}",
        now_ts(),
        &uuid::Uuid::new_v4().simple().to_string()[..8],
        display_name
    ));
    fs::write(&target, bytes)?;
    attachment_result(&target, Some(&target), &display_name)
}

fn attachment_result(
    target: &Path,
    original: Option<&Path>,
    display_name: &str,
) -> anyhow::Result<Value> {
    let metadata = fs::metadata(target)?;
    let extension = target
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    let kind = infer_attachment_kind(&extension);
    let mime_type = guess_attachment_mime_type(target, kind);
    let summary = if kind == "text" {
        fs::read_to_string(target)
            .ok()
            .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
            .chars()
            .take(220)
            .collect::<String>()
    } else {
        String::new()
    };
    Ok(json!({
        "success": true,
        "canceled": false,
        "attachment": {
            "type": "uploaded-file",
            "name": display_name,
            "ext": extension,
            "size": metadata.len(),
            "absolutePath": target.to_string_lossy(),
            "originalAbsolutePath": original.unwrap_or(target).to_string_lossy(),
            "localUrl": target.to_string_lossy(),
            "kind": kind,
            "mimeType": mime_type,
            "storageMode": "staged",
            "directUploadEligible": kind == "image",
            "processingStrategy": if kind == "image" { "direct-image-or-staged" } else if kind == "text" { "staged-text" } else { "staged-file" },
            "requiresMultimodal": kind == "image",
            "summary": summary,
        }
    }))
}

fn sanitize_attachment_file_name(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric()
                || matches!(ch, '.' | '-' | '_')
                || ('\u{4e00}'..='\u{9fff}').contains(&ch)
            {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.trim_matches(['.', '_']).is_empty() {
        "attachment.bin".into()
    } else {
        sanitized
    }
}

fn infer_attachment_kind(extension: &str) -> &'static str {
    match extension.trim_start_matches('.').to_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "svg" | "avif" => "image",
        "mp4" | "mov" | "webm" | "m4v" | "avi" | "mkv" => "video",
        "mp3" | "wav" | "m4a" | "aac" | "flac" | "ogg" | "opus" => "audio",
        "txt" | "md" | "markdown" | "json" | "csv" | "tsv" | "xml" | "yaml" | "yml" | "html"
        | "htm" | "js" | "jsx" | "ts" | "tsx" | "py" | "rs" | "go" | "java" | "c" | "cpp" | "h"
        | "hpp" => "text",
        _ => "binary",
    }
}

fn guess_attachment_mime_type(path: &Path, kind: &str) -> String {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => {
            if kind == "audio" {
                "audio/webm"
            } else {
                "video/webm"
            }
        }
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "ogg" | "opus" => "audio/ogg",
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        _ if kind == "text" => "text/plain",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn extension_from_mime_type(mime_type: &str) -> &'static str {
    match mime_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
        .as_str()
    {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "audio/mp4" => "m4a",
        "audio/aac" => "aac",
        "audio/ogg" => "ogg",
        "text/plain" => "txt",
        "text/markdown" => "md",
        "application/json" => "json",
        _ => "bin",
    }
}

fn sanitize_chat_message_for_renderer(mut message: ChatMessage) -> Option<ChatMessage> {
    if message.role == "assistant" {
        message.content = sanitize_assistant_content_for_renderer(&message.content)?;
    }
    Some(message)
}

fn sanitize_assistant_content_for_renderer(content: &str) -> Option<String> {
    let text = content.trim();
    let Some(error_index) = text.find("Ran into this error:") else {
        if let Some(raw_index) = text.find("\nRaw response:") {
            return Some(text[..raw_index].trim().to_string());
        }
        return Some(content.to_string());
    };

    let error_text = &text[error_index..];
    if !error_text.contains("Failed to parse Responses stream event")
        && !error_text.contains("Stream decode error")
    {
        return Some(content.to_string());
    }

    None
}

fn need_payload_str<'a>(payload: &'a Value, keys: &[&str]) -> anyhow::Result<&'a str> {
    if let Some(s) = payload.as_str().filter(|s| !s.trim().is_empty()) {
        return Ok(s);
    }
    for key in keys {
        if let Some(s) = payload
            .get(*key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            return Ok(s);
        }
    }
    Err(anyhow::anyhow!("缺少字段 {}", keys.join("/")))
}

fn payload_touches_goose_settings(payload: &Value) -> bool {
    let Some(obj) = payload.as_object() else {
        return false;
    };
    [
        "api_endpoint",
        "api_key",
        "model_name",
        "ai_sources_json",
        "default_ai_source_id",
        "chat_max_tokens_default",
        "chat_max_tokens_deepseek",
        "image_provider",
        "image_endpoint",
        "image_api_key",
        "image_model",
        "image_size",
        "image_quality",
        "image_aspect_ratio",
        "video_endpoint",
        "video_api_key",
        "video_model",
    ]
    .iter()
    .any(|key| obj.contains_key(*key))
}

fn build_context_metadata(payload: &Value, context_id: &str, context_type: &str) -> Value {
    let mut metadata = payload
        .get("metadata")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    metadata.insert("contextId".into(), json!(context_id));
    metadata.insert("contextType".into(), json!(context_type));
    if let Some(initial_context) = payload.get("initialContext").and_then(|v| v.as_str()) {
        metadata.insert("initialContext".into(), json!(initial_context));
    }
    Value::Object(metadata)
}

fn create_context_session(
    state: &AppState,
    payload: &Value,
    context_id: &str,
    context_type: &str,
    title: &str,
) -> anyhow::Result<ChatSession> {
    let metadata = build_context_metadata(payload, context_id, context_type).to_string();
    let id = format!(
        "ctx-{context_type}-{context_id}-{}-{}",
        now_ts(),
        uuid::Uuid::new_v4().simple()
    );
    state
        .db
        .chat()
        .create_session(&id, Some(title), Some(&metadata))
}

fn session_matches_context(session: &ChatSession, context_id: &str, context_type: &str) -> bool {
    let Some(metadata) = session.metadata.as_deref() else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(metadata) else {
        return false;
    };
    value.get("contextId").and_then(|v| v.as_str()) == Some(context_id)
        && value.get("contextType").and_then(|v| v.as_str()) == Some(context_type)
}

fn context_session_item(state: &AppState, session: &ChatSession) -> anyhow::Result<Value> {
    let message_count = count_session_rows(
        state,
        "SELECT COUNT(*) AS count FROM chat_messages WHERE session_id = ?1",
        &session.id,
    )?;
    let transcript_count = count_session_rows(
        state,
        "SELECT COUNT(*) AS count FROM session_transcript_records WHERE session_id = ?1",
        &session.id,
    )?;
    let checkpoint_count = count_session_rows(
        state,
        "SELECT COUNT(*) AS count FROM agent_tasks WHERE owner_session_id = ?1",
        &session.id,
    )?;

    Ok(json!({
        "id": session.id.clone(),
        "messageCount": message_count,
        "summary": "",
        "transcriptCount": transcript_count,
        "checkpointCount": checkpoint_count,
        "context": null,
        "chatSession": session,
    }))
}

fn count_session_rows(state: &AppState, sql: &str, session_id: &str) -> anyhow::Result<i64> {
    let value = state
        .db
        .query_one_json(sql, &[json!(session_id)])?
        .and_then(|row| {
            row.get("count")
                .and_then(|v| v.as_i64())
                .or_else(|| row.get("COUNT(*)").and_then(|v| v.as_i64()))
        })
        .unwrap_or(0);
    Ok(value)
}

/// 把 [`BridgeEvent`] 规整为前端 `runtime:event` 信封。
pub(super) fn bridge_event_payload(session_id: &str, ev: &BridgeEvent) -> Value {
    match ev {
        BridgeEvent::TextDelta(t) => runtime_event(
            session_id,
            "runtime:text-delta",
            json!({ "content": t, "stream": "response" }),
        ),
        BridgeEvent::ThoughtDelta(t) => runtime_event(
            session_id,
            "runtime:text-delta",
            json!({ "content": t, "stream": "thought" }),
        ),
        BridgeEvent::ToolStart {
            call_id,
            name,
            input,
        } => runtime_event(
            session_id,
            "runtime:tool-start",
            json!({
                "callId": call_id,
                "name": name,
                "input": input,
                "description": format!("正在调用 {name}"),
            }),
        ),
        BridgeEvent::ToolEnd {
            call_id,
            name,
            output,
        } => runtime_event(
            session_id,
            "runtime:tool-end",
            json!({ "callId": call_id, "name": name, "output": output }),
        ),
        BridgeEvent::Error {
            detail, category, ..
        } => runtime_done_payload(session_id, detail, "failed", category),
        BridgeEvent::Cancelled => {
            runtime_checkpoint_payload(session_id, "chat.cancelled", json!({}))
        }
        BridgeEvent::Done => runtime_done_payload(session_id, "", "completed", ""),
    }
}

fn runtime_event(session_id: &str, event_type: &str, payload: Value) -> Value {
    json!({
        "eventType": event_type,
        "sessionId": session_id,
        "taskId": null,
        "runtimeId": null,
        "parentRuntimeId": null,
        "payload": payload,
        "timestamp": now_ts(),
    })
}

pub(super) fn runtime_stream_start_payload(session_id: &str) -> Value {
    runtime_event(
        session_id,
        "runtime:stream-start",
        json!({ "phase": "thinking", "runtimeMode": "goose" }),
    )
}

pub(super) fn runtime_done_payload(
    session_id: &str,
    content: &str,
    status: &str,
    reason: &str,
) -> Value {
    runtime_event(
        session_id,
        "runtime:done",
        json!({
            "status": status,
            "content": content,
            "runtimeMode": "goose",
            "reason": reason,
        }),
    )
}

fn runtime_checkpoint_payload(session_id: &str, checkpoint_type: &str, payload: Value) -> Value {
    runtime_event(
        session_id,
        "runtime:checkpoint",
        json!({
            "checkpointType": checkpoint_type,
            "payload": payload,
            "summary": "",
        }),
    )
}

pub(super) fn need_str<'a>(payload: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("缺少字段 {key}"))
}

fn validate_space_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty() || id.len() > 128 {
        anyhow::bail!("空间 ID 无效");
    }
    if !id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        anyhow::bail!("空间 ID 包含不支持的字符");
    }
    Ok(())
}

fn space_exists(state: &AppState, id: &str) -> anyhow::Result<bool> {
    Ok(state
        .db
        .spaces()
        .list()?
        .into_iter()
        .any(|space| space.id == id))
}

fn ensure_space_directories(state: &AppState, id: &str) -> anyhow::Result<()> {
    let mut settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
    if !settings.is_object() {
        settings = json!({});
    }
    settings["active_space_id"] = json!(id);
    let base = crate::workspace::resolve(&settings).base;
    fs::create_dir_all(&base)?;
    for directory in [
        "manuscripts",
        "media",
        "cover",
        "knowledge",
        "skills",
        "redclaw",
        "archives",
        "chatrooms",
        "advisors",
    ] {
        fs::create_dir_all(base.join(directory))?;
    }
    Ok(())
}

fn estimate_message_tokens(message: &ChatMessage) -> usize {
    4 + estimate_text_tokens(&message.content)
        + message
            .tool_calls
            .as_deref()
            .map(estimate_text_tokens)
            .unwrap_or(0)
}

fn estimate_text_tokens(text: &str) -> usize {
    let (ascii, non_ascii) = text.chars().fold((0usize, 0usize), |counts, character| {
        if character.is_ascii() {
            (counts.0 + 1, counts.1)
        } else {
            (counts.0, counts.1 + 1)
        }
    });
    ascii.div_ceil(4) + non_ascii
}

pub(super) fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::goose_bridge::GooseBridge;
    use crate::ipc::NoopEmitter;

    fn test_state() -> AppState {
        let db = Db::open_in_memory().unwrap();
        AppState {
            redclaw_scheduler: crate::ipc::redclaw_runner::RedClawScheduler::inactive(db.clone()),
            db,
            goose: GooseBridge::default(),
            emitter: Arc::new(NoopEmitter),
            login: Arc::new(crate::login::LoginService::new(Arc::new(
                crate::login::StubLoginDriver,
            ))),
        }
    }

    #[tokio::test]
    async fn space_mutations_match_renderer_contract_and_persist_switch() {
        let state = test_state();
        let workspace = tempfile::tempdir().unwrap();
        state
            .db
            .settings()
            .save(&json!({ "workspace_dir": workspace.path().to_string_lossy() }))
            .unwrap();

        let created = invoke(
            "spaces:create",
            json!({ "id": "space-contract-test", "name": "内容团队" }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(created["success"], json!(true));
        assert_eq!(created["space"]["id"], json!("space-contract-test"));
        assert_eq!(created["space"]["name"], json!("内容团队"));
        assert!(workspace
            .path()
            .join("spaces/space-contract-test/media")
            .is_dir());

        let renamed = invoke(
            "spaces:rename",
            json!({ "id": "space-contract-test", "name": "视频团队" }),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(renamed["success"], json!(true));

        let switched = invoke("spaces:switch", json!("space-contract-test"), &state)
            .await
            .unwrap();
        assert_eq!(switched["success"], json!(true));
        assert_eq!(switched["activeSpaceId"], json!("space-contract-test"));
        assert_eq!(
            state.db.settings().get().unwrap()["active_space_id"],
            json!("space-contract-test")
        );

        let listed = invoke("spaces:list", Value::Null, &state).await.unwrap();
        assert_eq!(listed["activeSpaceId"], json!("space-contract-test"));
        assert!(listed["spaces"].as_array().is_some_and(|spaces| spaces
            .iter()
            .any(|space| space["id"] == "space-contract-test" && space["name"] == "视频团队")));
    }

    #[tokio::test]
    async fn switching_to_unknown_space_is_rejected() {
        let state = test_state();
        let error = invoke("spaces:switch", json!("space-missing"), &state)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("空间不存在"));
        assert_eq!(
            state.db.settings().get().unwrap()["active_space_id"],
            json!("default")
        );
    }

    #[test]
    fn renderer_history_hides_raw_responses_payload() {
        let content = concat!(
            "准备调用图片生成工具。",
            "Ran into this error: Request failed: Stream decode error: ",
            "Failed to parse Responses stream event: missing field `id`: ",
            "{\"tools\":[{\"name\":\"private-tool\"}],\"usage\":{}}.\n\n",
            "Please retry if you think this is a transient or recoverable error."
        );

        assert_eq!(sanitize_assistant_content_for_renderer(content), None);
    }

    #[test]
    fn renderer_history_keeps_normal_long_content() {
        let content = "正常回答".repeat(4_000);
        assert_eq!(
            sanitize_assistant_content_for_renderer(&content),
            Some(content)
        );
    }

    #[test]
    fn estimates_mixed_language_message_tokens() {
        let message = ChatMessage {
            id: "m1".into(),
            session_id: "s1".into(),
            role: "user".into(),
            content: "hello world 你好".into(),
            display_content: None,
            attachment: None,
            tool_calls: Some("{\"ok\":true}".into()),
            tool_call_id: None,
            timestamp: 0,
        };
        assert_eq!(estimate_text_tokens("你好"), 2);
        assert!(estimate_message_tokens(&message) >= 10);
    }

    #[test]
    fn multiple_uploaded_images_are_preserved_in_the_model_message() {
        let workspace = tempfile::tempdir().unwrap();
        let first_path = workspace.path().join("first.png");
        let second_path = workspace.path().join("second.png");
        fs::write(&first_path, b"first-image").unwrap();
        fs::write(&second_path, b"second-image").unwrap();
        let attachments = vec![
            json!({
                "type": "uploaded-file",
                "name": "first.png",
                "kind": "image",
                "mimeType": "image/png",
                "absolutePath": first_path,
            }),
            json!({
                "type": "uploaded-file",
                "name": "second.png",
                "kind": "image",
                "mimeType": "image/png",
                "absolutePath": second_path,
            }),
        ];

        let message = build_goose_user_message("请比较两张图片", &attachments).unwrap();
        assert_eq!(
            message
                .content
                .iter()
                .filter(|content| matches!(
                    content,
                    goose::conversation::message::MessageContent::Image(_)
                ))
                .count(),
            2,
        );
        assert!(message.as_concat_text().contains("[用户上传附件 1]"));
        assert!(message.as_concat_text().contains("[用户上传附件 2]"));
        assert_eq!(
            attachments_from_payload(&json!({ "attachments": attachments })).len(),
            2,
        );
        let persisted = serialize_attachments(&attachments).unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&persisted)
                .unwrap()
                .as_array()
                .map(Vec::len),
            Some(2),
        );
    }

    #[tokio::test]
    async fn redclaw_session_init_contract_matches_renderer() {
        let state = test_state();

        let spaces = invoke("spaces:list", Value::Null, &state).await.unwrap();
        assert_eq!(spaces["activeSpaceId"], json!("default"));
        assert!(spaces["spaces"]
            .as_array()
            .is_some_and(|items| !items.is_empty()));
        assert!(spaces["spaces"][0].get("updatedAt").is_some());

        let context_payload = json!({
            "contextId": "redclaw:default",
            "contextType": "redclaw",
            "title": "商媒运营助手 · 默认空间",
            "initialContext": "当前空间: 默认空间 (default)",
            "metadata": {
                "agentRuntime": "goose",
                "runtimeBridge": "goose",
                "runtimeMode": "redclaw",
                "employeeClient": true
            }
        });

        let empty = invoke(
            "chat:list-context-sessions",
            context_payload.clone(),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(empty.as_array().unwrap().len(), 0);

        let session = invoke(
            "chat:getOrCreateContextSession",
            context_payload.clone(),
            &state,
        )
        .await
        .unwrap();
        let session_id = session["id"].as_str().unwrap();
        assert!(session_id.starts_with("ctx-redclaw-redclaw:default-"));
        assert!(session["updatedAt"].as_str().is_some());
        assert!(session.get("updated_at").is_none());

        let existing = invoke(
            "chat:getOrCreateContextSession",
            context_payload.clone(),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(existing["id"], session["id"]);

        let created = invoke(
            "chat:create-context-session",
            context_payload.clone(),
            &state,
        )
        .await
        .unwrap();
        let created_id = created["id"].as_str().unwrap();
        assert!(created_id.starts_with("ctx-redclaw-redclaw:default-"));
        assert_ne!(created["id"], session["id"]);
        assert!(created["updatedAt"].as_str().is_some());

        let list = invoke("chat:list-context-sessions", context_payload, &state)
            .await
            .unwrap();
        let items = list.as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().any(|item| item["id"] == json!(session_id)));
        assert!(items.iter().any(|item| item["id"] == json!(created_id)));
        assert_eq!(items[0]["messageCount"], json!(0));
        assert!(items.iter().all(|item| item["messageCount"] == json!(0)));
        assert!(items[0]["chatSession"]["updatedAt"].as_str().is_some());

        let messages = invoke("chat:get-messages", json!(session_id), &state)
            .await
            .unwrap();
        assert_eq!(messages.as_array().unwrap().len(), 0);
    }
}
