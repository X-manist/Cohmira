//! 嵌入式 Goose 会话驱动（替代 goosed sidecar + stdio 桥）。
//!
//! 直接把 Goose 作为库链接进进程，对前端保持与原 goosed 一致的会话/事件契约（docs/14）。
//! 嵌入模板取自 `crates/goose/examples/agent.rs`：
//!   `create_with_named_model` → `Agent::new()` → `session_manager.create_session`
//!   → `agent.update_provider` → `agent.reply` 流式。
//!
//! 控制面保留一个 agent；聊天面按前端会话维护独立的 agent、Goose session 与取消令牌，
//! 从而允许多个聊天会话在后台并行继续运行。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::{self, BoxStream, StreamExt};
use futures::Stream;
use goose::agents::extension::Envs;
use goose::agents::{
    Agent, AgentConfig, AgentEvent, ExtensionConfig, GoosePlatform, SessionConfig, ToolCallContext,
};
use goose::config::permission::{PermissionLevel, PermissionManager};
use goose::config::{GooseMode, DEFAULT_EXTENSION_DESCRIPTION};
use goose::conversation::message::{Message, MessageContent};
use goose::model::ModelConfig;
use goose::providers::base::Provider;
use goose::providers::create;
use goose::session::{SessionManager, SessionType};
use rmcp::model::CallToolRequestParams;
use serde_json::Value;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use yunying_config::Config;

/// 嵌入式 Goose 运行时。控制面与聊天会话运行时均通过 `Arc` 共享，支持廉价 `Clone`。
#[derive(Clone)]
pub struct GooseBridge {
    agent: Arc<Agent>,
    session: Arc<Mutex<Option<SessionInfo>>>,
    chat_sessions: Arc<Mutex<HashMap<String, ChatSessionRuntime>>>,
    chat_cancel_tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
    provider: Arc<RwLock<Option<Arc<dyn Provider>>>>,
    extension_configs: Arc<Mutex<Vec<ExtensionConfig>>>,
}

#[derive(Default)]
pub(crate) struct StdioMcpEnvironment {
    pub(crate) values: HashMap<String, String>,
    pub(crate) inherited_keys: Vec<String>,
}

/// 测试用 Default：构造 agent 但不建会话/不装 provider（reply 会因无会话报错，
/// 但 AppState 的 DB 类测试不需要 reply）。真实运行用 [`GooseBridge::new`]。
impl Default for GooseBridge {
    fn default() -> Self {
        Self {
            agent: Arc::new(desktop_agent()),
            session: Arc::new(Mutex::new(None)),
            chat_sessions: Arc::new(Mutex::new(HashMap::new())),
            chat_cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
            provider: Arc::new(RwLock::new(None)),
            extension_configs: Arc::new(Mutex::new(vec![document_extension_config()])),
        }
    }
}

struct SessionInfo {
    session_id: String,
}

#[derive(Clone)]
struct ChatSessionRuntime {
    goose_session_id: String,
    agent: Arc<Agent>,
    permission_manager: Option<Arc<PermissionManager>>,
}

fn desktop_agent() -> Agent {
    Agent::with_config(AgentConfig::new(
        Arc::new(SessionManager::instance()),
        PermissionManager::instance(),
        None,
        GooseMode::Auto,
        false,
        GoosePlatform::GooseDesktop,
    ))
}

fn document_extension_config() -> ExtensionConfig {
    ExtensionConfig::Platform {
        name: "office".to_string(),
        description: "Read common local documents with bundled, read-only Rust parsers".to_string(),
        display_name: Some("Document Reader".to_string()),
        bundled: Some(true),
        available_tools: vec!["document_read".to_string()],
    }
}

fn restricted_background_agent(permission_manager: Arc<PermissionManager>) -> Agent {
    Agent::with_config(AgentConfig::new(
        Arc::new(SessionManager::instance()),
        permission_manager,
        None,
        GooseMode::Approve,
        false,
        GoosePlatform::GooseDesktop,
    ))
}

/// 一条已规整化的前端事件（对齐原 `chatAdapter` 的 runtime:* 信封）。
#[derive(Debug, Clone)]
pub enum BridgeEvent {
    /// 文本增量（agent 输出的 `Text` chunk，前端自行累积）。
    TextDelta(String),
    /// 模型思考增量。
    ThoughtDelta(String),
    /// 工具调用开始。
    ToolStart {
        call_id: String,
        name: String,
        input: Value,
    },
    /// 工具调用结束。
    ToolEnd {
        call_id: String,
        name: String,
        output: Value,
    },
    /// Goose/provider 错误。必须走独立错误通道，不能混入 assistant 正文。
    Error {
        message: String,
        detail: String,
        category: String,
    },
    /// 当前前端会话被用户主动取消。
    Cancelled,
    /// 一轮结束（Goose 事件流自然结束）。
    Done,
}

impl GooseBridge {
    /// 从 [`Config`] 构造运行时：注入 Goose/OpenAI 配置 → 建 agent → 建 session → 装 provider。
    pub async fn new(cfg: &Config) -> anyhow::Result<Self> {
        apply_goose_env(cfg);

        let provider = create_configured_provider(cfg).await?;
        let agent = Arc::new(desktop_agent());

        let session = agent
            .config
            .session_manager
            .create_session(
                PathBuf::default(),
                "yunying".to_string(),
                SessionType::Hidden,
                GooseMode::Auto,
            )
            .await?;
        agent
            .update_provider(Arc::clone(&provider), &session.id)
            .await?;
        let document_extension = document_extension_config();
        agent
            .add_extension(document_extension.clone(), &session.id)
            .await?;

        Ok(Self {
            agent,
            session: Arc::new(Mutex::new(Some(SessionInfo {
                session_id: session.id,
            }))),
            chat_sessions: Arc::new(Mutex::new(HashMap::new())),
            chat_cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
            provider: Arc::new(RwLock::new(Some(provider))),
            extension_configs: Arc::new(Mutex::new(vec![document_extension])),
        })
    }

    /// 根据设置页持久化的默认 AI 源热更新 Goose provider。
    ///
    /// 返回 `Ok(false)` 表示 settings 中还没有足够的模型配置；保存 settings 本身不应因此失败。
    pub async fn reload_from_settings(&self, settings: &Value) -> anyhow::Result<bool> {
        let has_goose_config = config_from_settings(settings).is_some();
        let cfg = apply_settings_to_config(Config::default(), settings);
        if !has_goose_config {
            apply_goose_env(&cfg);
            return Ok(false);
        }
        self.reload_from_config(&cfg).await?;
        Ok(true)
    }

    /// 重新创建 provider 并挂到当前 Goose session；若当前 bridge 是降级态，则先创建 session。
    pub async fn reload_from_config(&self, cfg: &Config) -> anyhow::Result<()> {
        apply_goose_env(cfg);
        let provider = create_configured_provider(cfg).await?;
        let session_id = self.ensure_session().await?;
        self.agent
            .update_provider(Arc::clone(&provider), &session_id)
            .await?;
        *self.provider.write().await = Some(Arc::clone(&provider));

        let chat_sessions = self
            .chat_sessions
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for runtime in chat_sessions {
            runtime
                .agent
                .update_provider(Arc::clone(&provider), &runtime.goose_session_id)
                .await?;
        }
        Ok(())
    }

    /// 发送一条用户消息，返回规整化事件流（末尾发 [`BridgeEvent::Done`]）。
    ///
    /// 把 Goose `AgentEvent::Message` 上的 `MessageContent` 段映射为 [`BridgeEvent`]，
    /// 对齐前端 `runtime:text-delta` / `runtime:done` 信封。
    pub async fn reply(&self, text: &str) -> anyhow::Result<impl Stream<Item = BridgeEvent> + '_> {
        let session_id = self
            .session_id()
            .await
            .ok_or_else(|| anyhow::anyhow!("无活跃 Goose 会话"))?;

        let session_config = SessionConfig {
            id: session_id,
            schedule_id: None,
            max_turns: None,
            retry_config: None,
        };
        let user_message = Message::user().with_text(text);
        let raw = self.agent.reply(user_message, session_config, None).await?;

        let mapped = raw
            .flat_map(|result| {
                let events: Vec<BridgeEvent> = match result {
                    Ok(AgentEvent::Message(msg)) => {
                        let is_assistant = msg.role == Message::assistant().role;
                        msg.content
                            .into_iter()
                            .filter_map(|content| content_to_event(content, is_assistant))
                            .collect()
                    }
                    Ok(_) => Vec::new(),
                    Err(error) => vec![BridgeEvent::Error {
                        message: "Goose 运行失败，请稍后重试".into(),
                        detail: sanitize_provider_error_detail(&error.to_string()),
                        category: "goose_stream".into(),
                    }],
                };
                stream::iter(events)
            })
            .scan(HashMap::<String, String>::new(), |tool_names, mut event| {
                normalize_tool_event_name(tool_names, &mut event);
                futures::future::ready(Some(event))
            })
            .chain(stream::once(async { BridgeEvent::Done }));
        Ok(mapped)
    }

    /// 在独立的前端聊天会话中发送纯文本。不同 `frontend_session_id` 使用不同
    /// Goose session 与 Agent，因此切换或新建会话不会中断旧会话。
    pub async fn reply_for_session(
        &self,
        frontend_session_id: &str,
        text: &str,
    ) -> anyhow::Result<BoxStream<'static, BridgeEvent>> {
        self.reply_message_for_session(
            frontend_session_id,
            Message::user().with_text(text.to_string()),
        )
        .await
    }

    /// Starts a background turn whose effective tool set is deny-by-default.
    /// Every mounted tool not named in `allowed_tools` receives `NeverAllow`
    /// before the model can issue its first call.
    pub async fn reply_for_session_with_allowed_tools(
        &self,
        frontend_session_id: &str,
        text: &str,
        allowed_tools: &[String],
    ) -> anyhow::Result<BoxStream<'static, BridgeEvent>> {
        let frontend_session_id = frontend_session_id.trim();
        if frontend_session_id.is_empty() {
            anyhow::bail!("后台会话 ID 不能为空");
        }
        if allowed_tools.is_empty() {
            anyhow::bail!("后台任务必须声明非空 allowedTools；默认拒绝全部工具");
        }
        let runtime = self
            .ensure_restricted_chat_session(frontend_session_id, allowed_tools)
            .await?;
        self.start_reply_for_runtime(
            frontend_session_id,
            Message::user().with_text(text.to_string()),
            runtime,
        )
        .await
    }

    /// 在独立的前端聊天会话中发送完整 Goose message（可包含图片）。
    pub async fn reply_message_for_session(
        &self,
        frontend_session_id: &str,
        user_message: Message,
    ) -> anyhow::Result<BoxStream<'static, BridgeEvent>> {
        let frontend_session_id = frontend_session_id.trim();
        if frontend_session_id.is_empty() {
            anyhow::bail!("前端会话 ID 不能为空");
        }
        let runtime = self.ensure_chat_session(frontend_session_id).await?;
        self.start_reply_for_runtime(frontend_session_id, user_message, runtime)
            .await
    }

    async fn start_reply_for_runtime(
        &self,
        frontend_session_id: &str,
        user_message: Message,
        runtime: ChatSessionRuntime,
    ) -> anyhow::Result<BoxStream<'static, BridgeEvent>> {
        let cancel_token = CancellationToken::new();
        {
            let mut active = self.chat_cancel_tokens.lock().await;
            if active.contains_key(frontend_session_id) {
                anyhow::bail!("当前会话仍在执行，请等待完成或先取消");
            }
            active.insert(frontend_session_id.to_string(), cancel_token.clone());
        }

        let bridge = self.clone();
        let frontend_session_id = frontend_session_id.to_string();
        let goose_session_id = runtime.goose_session_id.clone();
        let agent = Arc::clone(&runtime.agent);
        let stream = async_stream::stream! {
            let session_config = SessionConfig {
                id: goose_session_id,
                schedule_id: None,
                max_turns: None,
                retry_config: None,
            };
            let mut tool_names = HashMap::<String, String>::new();
            match agent
                .reply(user_message, session_config, Some(cancel_token.clone()))
                .await
            {
                Ok(mut raw) => {
                    while let Some(result) = raw.next().await {
                        let events: Vec<BridgeEvent> = match result {
                            Ok(AgentEvent::Message(msg)) => {
                                let is_assistant = msg.role == Message::assistant().role;
                                msg.content
                                    .into_iter()
                                    .filter_map(|content| content_to_event(content, is_assistant))
                                    .collect()
                            }
                            Ok(_) => Vec::new(),
                            Err(error) => vec![BridgeEvent::Error {
                                message: "Goose 运行失败，请稍后重试".into(),
                                detail: sanitize_provider_error_detail(&error.to_string()),
                                category: "goose_stream".into(),
                            }],
                        };
                        for mut event in events {
                            normalize_tool_event_name(&mut tool_names, &mut event);
                            yield event;
                        }
                    }
                }
                Err(error) => {
                    yield BridgeEvent::Error {
                        message: "Goose 回复启动失败，请稍后重试".into(),
                        detail: sanitize_provider_error_detail(&error.to_string()),
                        category: "goose_start".into(),
                    };
                }
            }

            bridge
                .chat_cancel_tokens
                .lock()
                .await
                .remove(&frontend_session_id);
            if cancel_token.is_cancelled() {
                yield BridgeEvent::Cancelled;
            } else {
                yield BridgeEvent::Done;
            }
        };
        Ok(Box::pin(stream))
    }

    /// 取消指定前端会话，不影响其他会话。
    pub async fn cancel_chat_session(&self, frontend_session_id: &str) -> bool {
        let token = self
            .chat_cancel_tokens
            .lock()
            .await
            .get(frontend_session_id)
            .cloned();
        if let Some(token) = token {
            token.cancel();
            true
        } else {
            false
        }
    }

    pub async fn is_chat_session_busy(&self, frontend_session_id: &str) -> bool {
        self.chat_cancel_tokens
            .lock()
            .await
            .contains_key(frontend_session_id)
    }

    /// Removes one hidden runtime and its persisted Goose session after a
    /// scheduler execution. This also clears tokens when a stream was dropped
    /// before its async cleanup tail ran.
    pub async fn close_chat_session(&self, frontend_session_id: &str) -> anyhow::Result<()> {
        if let Some(token) = self
            .chat_cancel_tokens
            .lock()
            .await
            .remove(frontend_session_id)
        {
            token.cancel();
        }
        let runtime = self.chat_sessions.lock().await.remove(frontend_session_id);
        if let Some(runtime) = runtime {
            SessionManager::instance()
                .delete_session(&runtime.goose_session_id)
                .await?;
        }
        Ok(())
    }

    /// A restricted background execution gets one call per allowed tool. Once
    /// the first result is observed, revoke that exact tool before the next
    /// model turn can issue another paid or side-effecting call.
    pub async fn revoke_chat_session_tool(
        &self,
        frontend_session_id: &str,
        tool_name: &str,
    ) -> anyhow::Result<()> {
        let runtime = self
            .chat_sessions
            .lock()
            .await
            .get(frontend_session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("后台会话不存在"))?;
        let manager = runtime
            .permission_manager
            .ok_or_else(|| anyhow::anyhow!("会话不是受限后台会话"))?;
        manager.update_user_permission(tool_name, PermissionLevel::NeverAllow);
        Ok(())
    }

    async fn ensure_chat_session(
        &self,
        frontend_session_id: &str,
    ) -> anyhow::Result<ChatSessionRuntime> {
        if let Some(existing) = self
            .chat_sessions
            .lock()
            .await
            .get(frontend_session_id)
            .cloned()
        {
            return Ok(existing);
        }

        let session_manager = SessionManager::instance();
        let session_name = format!("yunying-chat:{frontend_session_id}");
        let session = match session_manager
            .list_sessions()
            .await?
            .into_iter()
            .find(|session| session.name == session_name)
        {
            Some(session) => session,
            None => {
                session_manager
                    .create_session(
                        PathBuf::default(),
                        session_name,
                        SessionType::Hidden,
                        GooseMode::Auto,
                    )
                    .await?
            }
        };

        let agent = Arc::new(desktop_agent());
        if let Some(provider) = self.provider.read().await.clone() {
            agent.update_provider(provider, &session.id).await?;
        } else {
            let _ = agent.restore_provider_from_session(&session).await;
        }

        let _ = agent.load_extensions_from_session(&session).await;
        let mut loaded = agent
            .list_extensions()
            .await
            .into_iter()
            .collect::<HashSet<_>>();
        let extension_configs = self.extension_configs.lock().await.clone();
        for extension in extension_configs {
            let name = extension.name();
            if loaded.contains(&name) {
                continue;
            }
            agent.add_extension(extension, &session.id).await?;
            loaded.insert(name);
        }

        let runtime = ChatSessionRuntime {
            goose_session_id: session.id,
            agent,
            permission_manager: None,
        };
        let mut sessions = self.chat_sessions.lock().await;
        Ok(sessions
            .entry(frontend_session_id.to_string())
            .or_insert_with(|| runtime.clone())
            .clone())
    }

    async fn ensure_restricted_chat_session(
        &self,
        frontend_session_id: &str,
        allowed_tools: &[String],
    ) -> anyhow::Result<ChatSessionRuntime> {
        if self
            .chat_sessions
            .lock()
            .await
            .contains_key(frontend_session_id)
        {
            anyhow::bail!("后台会话 ID 已存在，拒绝复用不同工具授权快照");
        }

        let session_manager = SessionManager::instance();
        let session = session_manager
            .create_session(
                PathBuf::default(),
                format!("yunying-scheduled:{frontend_session_id}"),
                SessionType::Hidden,
                GooseMode::Approve,
            )
            .await?;
        let permission_manager = Arc::new(PermissionManager::new_ephemeral());
        let agent = Arc::new(restricted_background_agent(Arc::clone(&permission_manager)));
        if let Some(provider) = self.provider.read().await.clone() {
            agent.update_provider(provider, &session.id).await?;
        } else {
            let _ = session_manager.delete_session(&session.id).await;
            anyhow::bail!("后台 Agent provider 尚未就绪");
        }

        let _ = agent.load_extensions_from_session(&session).await;
        let mut loaded = agent
            .list_extensions()
            .await
            .into_iter()
            .collect::<HashSet<_>>();
        let extension_configs = self.extension_configs.lock().await.clone();
        for extension in extension_configs {
            let name = extension.name();
            if loaded.contains(&name) {
                continue;
            }
            if let Err(error) = agent.add_extension(extension, &session.id).await {
                let _ = session_manager.delete_session(&session.id).await;
                return Err(error.into());
            }
            loaded.insert(name);
        }

        let tools = agent.list_tools(&session.id, None).await;
        let missing: Vec<&str> = allowed_tools
            .iter()
            .filter(|allowed| {
                !tools
                    .iter()
                    .any(|tool| scheduled_tool_name_matches(&tool.name, allowed))
            })
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            let _ = session_manager.delete_session(&session.id).await;
            anyhow::bail!("后台任务授权的工具尚未就绪：{}", missing.join(", "));
        }
        let permissions = tools
            .iter()
            .map(|tool| {
                let allowed = allowed_tools
                    .iter()
                    .any(|name| scheduled_tool_name_matches(&tool.name, name));
                (
                    tool.name.to_string(),
                    if allowed {
                        PermissionLevel::AlwaysAllow
                    } else {
                        PermissionLevel::NeverAllow
                    },
                )
            })
            .collect::<Vec<_>>();
        permission_manager.replace_user_permissions(&permissions);

        let runtime = ChatSessionRuntime {
            goose_session_id: session.id,
            agent,
            permission_manager: Some(permission_manager),
        };
        self.chat_sessions
            .lock()
            .await
            .insert(frontend_session_id.to_string(), runtime.clone());
        Ok(runtime)
    }

    async fn session_id(&self) -> Option<String> {
        self.session
            .lock()
            .await
            .as_ref()
            .map(|s| s.session_id.clone())
    }

    async fn ensure_session(&self) -> anyhow::Result<String> {
        if let Some(session_id) = self.session_id().await {
            return Ok(session_id);
        }

        let session = self
            .agent
            .config
            .session_manager
            .create_session(
                PathBuf::default(),
                "yunying".to_string(),
                SessionType::Hidden,
                GooseMode::Auto,
            )
            .await?;
        let session_id = session.id;
        *self.session.lock().await = Some(SessionInfo {
            session_id: session_id.clone(),
        });
        Ok(session_id)
    }

    /// 暴露底层 agent（供注册 MCP 扩展 / skills 等）。
    pub fn agent(&self) -> &Arc<Agent> {
        &self.agent
    }

    /// 注册 `yunying-ops-mcp` 作为 Goose 的 stdio MCP 扩展。
    ///
    /// 注册后模型即可发现并自主调用 operations 工具（list_capabilities / start_task /
    /// generate_image / upload_video / social_check_account / create_note），产出结构化
    /// workParams 而非纯文本建议。`bin_path` 为 yunying-ops-mcp 可执行文件路径
    /// （dev：target/debug/yunying-ops-mcp；打包后：bundled 路径）。
    pub async fn register_operations_mcp(&self, bin_path: &str) -> anyhow::Result<()> {
        let timeout_seconds = std::env::var("YUNYING_OPERATIONS_MCP_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(20 * 60)
            .clamp(60, 2 * 60 * 60);
        let env_keys = [
            "YUNYING_CONFIG_PATH",
            crate::plugins::bridge::APP_BRIDGE_URL_ENV,
            crate::plugins::bridge::APP_BRIDGE_TOKEN_ENV,
        ]
        .into_iter()
        .filter(|key| std::env::var(key).is_ok_and(|value| !value.trim().is_empty()))
        .map(str::to_string)
        .collect();
        self.register_stdio_mcp(
            "yunying-ops",
            bin_path,
            Vec::new(),
            StdioMcpEnvironment {
                values: HashMap::new(),
                inherited_keys: env_keys,
            },
            DEFAULT_EXTENSION_DESCRIPTION,
            timeout_seconds,
        )
        .await
    }

    /// 注册任意 stdio MCP。插件运行时用它把 `uv run .../mcp/server.py` 挂到 Goose，
    /// 无需把 Python 工具重写成 Rust，也不要求桌面包携带 Node.js。
    pub(crate) async fn register_stdio_mcp(
        &self,
        name: &str,
        command: &str,
        args: Vec<String>,
        environment: StdioMcpEnvironment,
        description: &str,
        timeout_seconds: u64,
    ) -> anyhow::Result<()> {
        let session_id = self.ensure_session().await?;
        let config = ExtensionConfig::Stdio {
            name: name.to_string(),
            description: description.to_string(),
            cmd: command.to_string(),
            args,
            envs: Envs::new(environment.values),
            env_keys: environment.inherited_keys,
            timeout: Some(timeout_seconds),
            bundled: Some(true),
            available_tools: Vec::new(),
        };
        // `Agent::add_extension` 会在配置完全相同时跳过重启；若上次运行持久化了
        // 旧端口/旧 env 形态，则会原地替换并重新持久化为当前 env_keys 配置。
        self.agent
            .add_extension(config.clone(), &session_id)
            .await?;
        {
            let mut configs = self.extension_configs.lock().await;
            configs.retain(|item| item.name() != name);
            configs.push(config.clone());
        }

        let chat_sessions = self
            .chat_sessions
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for runtime in chat_sessions {
            if let Err(error) = runtime
                .agent
                .add_extension(config.clone(), &runtime.goose_session_id)
                .await
            {
                eprintln!(
                    "[Goose] 会话 {} 挂载扩展 {} 失败：{}",
                    runtime.goose_session_id, name, error
                );
            }
        }
        Ok(())
    }

    /// Read one MCP resource through the already-mounted extension connection.
    /// Used by the renderer-side MCP App host; no plugin-specific UI logic lives here.
    pub async fn read_mcp_resource(
        &self,
        extension_name: &str,
        uri: &str,
    ) -> anyhow::Result<Value> {
        let session_id = self
            .session_id()
            .await
            .ok_or_else(|| anyhow::anyhow!("无活跃 Goose 会话"))?;
        let result = self
            .agent
            .extension_manager
            .read_resource(&session_id, uri, extension_name, CancellationToken::new())
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(serde_json::to_value(result)?)
    }

    /// Call one MCP tool through the mounted extension. The IPC layer restricts
    /// which app-visible tools may reach this generic bridge.
    pub async fn call_mcp_tool(&self, name: &str, arguments: Value) -> anyhow::Result<Value> {
        let session_id = self
            .session_id()
            .await
            .ok_or_else(|| anyhow::anyhow!("无活跃 Goose 会话"))?;
        let arguments = arguments.as_object().cloned().unwrap_or_default();
        let request = CallToolRequestParams::new(name.to_string()).with_arguments(arguments);
        let context = ToolCallContext::new(session_id, None, None);
        let dispatched = self
            .agent
            .extension_manager
            .dispatch_tool_call(&context, request, CancellationToken::new())
            .await?;
        let result = dispatched
            .result
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        Ok(serde_json::to_value(result)?)
    }

    pub async fn tool_names(&self) -> anyhow::Result<Vec<String>> {
        let session_id = self
            .session_id()
            .await
            .ok_or_else(|| anyhow::anyhow!("无活跃 Goose 会话"))?;
        Ok(self
            .agent
            .list_tools(&session_id, None)
            .await
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect())
    }
}

async fn create_configured_provider(cfg: &Config) -> anyhow::Result<Arc<dyn Provider>> {
    let mut model = ModelConfig::new(&cfg.goose.model)?.with_canonical_limits(&cfg.goose.provider);
    if let Some(context_limit) = cfg.goose.context_limit.filter(|value| *value >= 4 * 1024) {
        model.context_limit = Some(context_limit);
    }
    if let Some(requested_max) = cfg.goose.max_tokens.filter(|value| *value > 0) {
        model.max_tokens = Some(match model.max_tokens {
            Some(model_max) if model_max > 0 => requested_max.min(model_max),
            _ => requested_max,
        });
    }

    let provider = create(&cfg.goose.provider, model, Vec::new()).await?;
    let resolved = provider.get_model_config();
    eprintln!(
        "[模型] provider={} model={} context_limit={} max_tokens={}",
        cfg.goose.provider,
        resolved.model_name,
        resolved.context_limit(),
        resolved
            .max_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "provider-default".into())
    );
    Ok(provider)
}

/// 单个 `MessageContent` 段 → [`BridgeEvent`]。
///
/// v1 只映射 `Text`（聊天文本）。`ToolRequest`/`ToolResponse`/`ToolConfirmationRequest`
/// 的结构化映射待后续接入 yunying-ops 工具确认时补全。
fn content_to_event(c: MessageContent, is_assistant: bool) -> Option<BridgeEvent> {
    match c {
        MessageContent::Text(t) if is_assistant => classify_goose_error_text(&t.text)
            .or_else(|| Some(BridgeEvent::TextDelta(t.text.clone()))),
        MessageContent::Thinking(t) if is_assistant => Some(BridgeEvent::ThoughtDelta(t.thinking)),
        MessageContent::ToolRequest(request) => match request.tool_call {
            Ok(call) => Some(BridgeEvent::ToolStart {
                call_id: request.id,
                name: call.name.to_string(),
                input: Value::Object(call.arguments.unwrap_or_default()),
            }),
            Err(error) => Some(BridgeEvent::ToolEnd {
                call_id: request.id,
                name: "tool".into(),
                output: serde_json::json!({
                    "success": false,
                    "content": error.to_string(),
                }),
            }),
        },
        MessageContent::ToolResponse(response) => {
            let output = match response.tool_result {
                Ok(result) => {
                    let serialized =
                        serde_json::to_value(&result).unwrap_or_else(|_| serde_json::json!({}));
                    let text = result
                        .content
                        .iter()
                        .filter_map(|content| {
                            let value = serde_json::to_value(content).ok()?;
                            value
                                .get("text")
                                .or_else(|| value.pointer("/raw/text"))
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mut output = serde_json::json!({
                        "success": !result.is_error.unwrap_or(false),
                        "content": if text.is_empty() {
                            serde_json::to_string(&result.content).unwrap_or_default()
                        } else {
                            text
                        },
                        "blocks": result.content,
                        "isError": result.is_error.unwrap_or(false),
                    });
                    if let (Some(target), Some(source)) =
                        (output.as_object_mut(), serialized.as_object())
                    {
                        if let Some(value) = source.get("structuredContent") {
                            target.insert("structuredContent".into(), value.clone());
                        }
                        if let Some(value) = source.get("_meta") {
                            target.insert("_meta".into(), value.clone());
                        }
                    }
                    output
                }
                Err(error) => serde_json::json!({
                    "success": false,
                    "content": error.to_string(),
                }),
            };
            Some(BridgeEvent::ToolEnd {
                call_id: response.id,
                name: "tool".into(),
                output,
            })
        }
        _ => None,
    }
}

fn normalize_tool_event_name(tool_names: &mut HashMap<String, String>, event: &mut BridgeEvent) {
    match event {
        BridgeEvent::ToolStart { call_id, name, .. } => {
            tool_names.insert(call_id.clone(), name.clone());
        }
        BridgeEvent::ToolEnd { call_id, name, .. } => {
            if name == "tool" {
                if let Some(start_name) = tool_names.get(call_id) {
                    *name = start_name.clone();
                }
            }
            tool_names.remove(call_id);
        }
        _ => {}
    }
}

fn scheduled_tool_name_matches(actual: &str, allowed: &str) -> bool {
    actual == allowed
        || (!allowed.contains("__")
            && actual
                .rsplit_once("__")
                .map(|(_, basename)| basename == allowed)
                .unwrap_or(false))
}

#[cfg(test)]
mod session_isolation_tests {
    use super::*;

    #[test]
    fn desktop_document_extension_exposes_only_the_read_tool() {
        match document_extension_config() {
            ExtensionConfig::Platform {
                name,
                available_tools,
                ..
            } => {
                assert_eq!(name, "office");
                assert_eq!(available_tools, vec!["document_read"]);
            }
            other => panic!("unexpected document extension config: {other:?}"),
        }
    }

    #[tokio::test]
    async fn new_employee_chat_session_mounts_only_document_read_from_office() {
        let bridge = GooseBridge::default();
        let runtime = bridge
            .ensure_chat_session("document-tool-contract")
            .await
            .unwrap();
        let tool_names = runtime
            .agent
            .list_tools(&runtime.goose_session_id, None)
            .await
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect::<Vec<_>>();

        assert!(tool_names.iter().any(|name| name == "document_read"));
        assert!(!tool_names.iter().any(|name| name == "ppt_create"));
        assert!(!tool_names.iter().any(|name| name == "developer__shell"));

        let spreadsheet = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../goose-mcp/src/computercontroller/tests/data/FinancialSample.xlsx");
        goose::document::allow_document_path(&spreadsheet).unwrap();
        let request = CallToolRequestParams::new("document_read".to_string()).with_arguments(
            serde_json::Map::from_iter([(
                "path".to_string(),
                serde_json::json!(spreadsheet.to_string_lossy()),
            )]),
        );
        let context = ToolCallContext::new(runtime.goose_session_id.clone(), None, None);
        let dispatched = runtime
            .agent
            .extension_manager
            .dispatch_tool_call(&context, request, CancellationToken::new())
            .await
            .unwrap();
        let result = dispatched.result.await.unwrap();
        let serialized = serde_json::to_string(&result).unwrap();
        assert!(serialized.contains("Government"));
        assert!(serialized.contains("spreadsheet"));
    }

    #[tokio::test]
    async fn different_frontend_sessions_can_be_active_and_cancelled_independently() {
        let bridge = GooseBridge::default();
        let first = bridge
            .reply_for_session("frontend-session-a", "hello")
            .await
            .unwrap();
        let second = bridge
            .reply_for_session("frontend-session-b", "hello")
            .await
            .unwrap();

        assert!(bridge.is_chat_session_busy("frontend-session-a").await);
        assert!(bridge.is_chat_session_busy("frontend-session-b").await);
        assert!(bridge.cancel_chat_session("frontend-session-a").await);
        assert!(bridge.is_chat_session_busy("frontend-session-b").await);

        drop(first);
        drop(second);
    }

    #[tokio::test]
    async fn the_same_frontend_session_rejects_overlapping_turns() {
        let bridge = GooseBridge::default();
        let first = bridge
            .reply_for_session("frontend-session", "first")
            .await
            .unwrap();
        let second = bridge.reply_for_session("frontend-session", "second").await;

        match second {
            Err(error) => assert!(error.to_string().contains("当前会话仍在执行")),
            Ok(_) => panic!("同一前端会话不应允许重叠执行"),
        }
        drop(first);
    }
}

const GOOSE_GENERIC_ERROR_SUFFIX: &str =
    ".\n\nPlease retry if you think this is a transient or recoverable error.";
const GOOSE_NETWORK_ERROR_SUFFIX: &str = "\n\nPlease resend your message to try again.";

fn classify_goose_error_text(text: &str) -> Option<BridgeEvent> {
    let raw = if let Some(value) = text
        .strip_prefix("Ran into this error: ")
        .and_then(|value| value.strip_suffix(GOOSE_GENERIC_ERROR_SUFFIX))
    {
        value
    } else if let Some(value) = text.strip_suffix(GOOSE_NETWORK_ERROR_SUFFIX) {
        value
    } else {
        return None;
    };

    let lower = raw.to_lowercase();
    let (message, category) = if lower.contains("failed to parse responses stream event")
        || lower.contains("stream decode error")
    {
        ("模型响应格式暂不兼容，本次操作未完成", "response_decode")
    } else if lower.contains("authentication")
        || lower.contains("unauthorized")
        || lower.contains("api key")
    {
        ("AI 服务认证失败，请检查 API Key", "authentication")
    } else if lower.contains("context length") || lower.contains("too many tokens") {
        ("当前会话上下文超过模型限制", "context_length")
    } else if lower.contains("network") || lower.contains("connection") || lower.contains("timeout")
    {
        ("无法连接 AI 服务，请稍后重试", "network")
    } else {
        ("AI 服务请求失败，请稍后重试", "provider_error")
    };

    Some(BridgeEvent::Error {
        message: message.into(),
        detail: sanitize_provider_error_detail(raw),
        category: category.into(),
    })
}

fn sanitize_provider_error_detail(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("failed to parse responses stream event")
        || lower.contains("stream decode error")
    {
        return "模型返回了应用暂不支持的 Responses API 事件格式。".into();
    }

    let first_line = raw.lines().next().unwrap_or(raw).trim();
    let without_payload = first_line
        .split("Raw response:")
        .next()
        .unwrap_or(first_line)
        .split("data: {")
        .next()
        .unwrap_or(first_line)
        .trim();
    without_payload.chars().take(500).collect()
}

/// 用 settings 表中的默认 AI 源覆盖 config.json 的 Goose 配置。
pub fn apply_settings_to_config(mut cfg: Config, settings: &Value) -> Config {
    if let Some(settings_cfg) = config_from_settings(settings) {
        cfg.goose.provider = settings_cfg.goose.provider;
        cfg.goose.model = settings_cfg.goose.model;
        cfg.goose.operations_agent_model = settings_cfg.goose.operations_agent_model;
        cfg.goose.base_url = settings_cfg.goose.base_url;
        cfg.goose.api_key = settings_cfg.goose.api_key;
        cfg.goose.context_limit = settings_cfg.goose.context_limit;
        cfg.goose.max_tokens = settings_cfg.goose.max_tokens;
    }
    if let Some(value) = setting_str(settings, "image_provider") {
        cfg.image.provider = value;
    }
    if let Some(value) = setting_str(settings, "image_endpoint") {
        cfg.image.endpoint = value;
    }
    if let Some(value) = setting_str(settings, "image_api_key") {
        cfg.image.api_key = value;
    }
    if let Some(value) = setting_str(settings, "image_model") {
        cfg.image.model = value;
    }
    if let Some(value) = setting_str(settings, "image_size") {
        cfg.image.size = value;
    }
    if let Some(value) = setting_str(settings, "image_quality") {
        cfg.image.quality = value;
    }
    if let Some(value) = setting_str(settings, "image_aspect_ratio") {
        cfg.image.aspect_ratio = value;
    }
    if let Some(value) = setting_str(settings, "video_endpoint") {
        cfg.video.endpoint = value;
    }
    if let Some(value) = setting_str(settings, "video_api_key") {
        cfg.video.api_key = value;
    }
    if let Some(value) = setting_str(settings, "video_model") {
        cfg.video.model = value;
    }
    cfg
}

fn config_from_settings(settings: &Value) -> Option<Config> {
    let selected = selected_ai_source(settings);
    let endpoint = setting_str(settings, "api_endpoint")
        .or_else(|| source_str(selected.as_ref(), "baseURL"))
        .or_else(|| source_str(selected.as_ref(), "base_url"))
        .unwrap_or_default();
    let api_key = setting_str(settings, "api_key")
        .or_else(|| source_str(selected.as_ref(), "apiKey"))
        .or_else(|| source_str(selected.as_ref(), "api_key"))
        .unwrap_or_default();
    let model = setting_str(settings, "model_name")
        .or_else(|| source_str(selected.as_ref(), "model"))
        .or_else(|| first_source_model(selected.as_ref()))
        .unwrap_or_default();

    if endpoint.is_empty() && api_key.is_empty() && model.is_empty() {
        return None;
    }
    if model.is_empty() {
        return None;
    }

    let protocol = source_str(selected.as_ref(), "protocol")
        .unwrap_or_else(|| infer_protocol_from_endpoint(&endpoint));
    let provider = goose_provider_for_protocol(&protocol);

    let mut cfg = Config::default();
    cfg.goose.provider = provider;
    cfg.goose.model = model.clone();
    cfg.goose.operations_agent_model = model.clone();
    cfg.goose.base_url = endpoint;
    cfg.goose.api_key = api_key;
    if let Some(metadata) = selected_model_metadata(selected.as_ref(), &model) {
        cfg.goose.context_limit = model_metadata_usize(
            metadata,
            &[
                "contextLimit",
                "context_limit",
                "contextLength",
                "context_length",
                "contextWindow",
                "context_window",
            ],
        );
        cfg.goose.max_tokens = model_metadata_usize(
            metadata,
            &[
                "maxOutputTokens",
                "max_output_tokens",
                "outputTokenLimit",
                "output_token_limit",
                "maxCompletionTokens",
                "max_completion_tokens",
            ],
        )
        .and_then(|value| i32::try_from(value).ok());
    }
    let output_limit_key = if cfg.goose.model.to_lowercase().contains("deepseek") {
        "chat_max_tokens_deepseek"
    } else {
        "chat_max_tokens_default"
    };
    if let Some(requested) = setting_positive_i32(settings, output_limit_key) {
        cfg.goose.max_tokens = Some(match cfg.goose.max_tokens {
            Some(model_limit) => requested.min(model_limit),
            None => requested,
        });
    }
    Some(cfg)
}

fn selected_model_metadata<'a>(source: Option<&'a Value>, model: &str) -> Option<&'a Value> {
    source?
        .get("modelsMeta")
        .or_else(|| source?.get("models_meta"))?
        .as_array()?
        .iter()
        .find(|item| {
            item.get("id")
                .and_then(|value| value.as_str())
                .map(str::trim)
                == Some(model.trim())
        })
}

fn model_metadata_usize(metadata: &Value, keys: &[&str]) -> Option<usize> {
    for key in keys {
        let Some(value) = metadata.get(*key) else {
            continue;
        };
        let parsed = value.as_u64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        });
        if let Some(parsed) = parsed.filter(|value| *value > 0) {
            if let Ok(parsed) = usize::try_from(parsed) {
                return Some(parsed);
            }
        }
    }
    None
}

fn setting_positive_i32(settings: &Value, key: &str) -> Option<i32> {
    settings
        .get(key)
        .and_then(|value| {
            value.as_i64().or_else(|| {
                value
                    .as_str()
                    .and_then(|text| text.trim().parse::<i64>().ok())
            })
        })
        .filter(|value| *value > 0)
        .and_then(|value| i32::try_from(value).ok())
}

fn selected_ai_source(settings: &Value) -> Option<Value> {
    let raw = settings
        .get("ai_sources_json")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return None;
    }
    let value = serde_json::from_str::<Value>(raw).ok()?;
    let sources = value.as_array()?;
    let default_id = settings
        .get("default_ai_source_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    sources
        .iter()
        .find(|source| {
            !default_id.is_empty()
                && source.get("id").and_then(|v| v.as_str()).map(str::trim) == Some(default_id)
        })
        .or_else(|| sources.first())
        .cloned()
}

fn setting_str(settings: &Value, key: &str) -> Option<String> {
    settings
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn source_str(source: Option<&Value>, key: &str) -> Option<String> {
    source?
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_source_model(source: Option<&Value>) -> Option<String> {
    source?
        .get("models")
        .and_then(|v| v.as_array())
        .and_then(|items| items.iter().find_map(|item| item.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn infer_protocol_from_endpoint(endpoint: &str) -> String {
    let endpoint = endpoint.to_lowercase();
    if endpoint.contains("anthropic.com") || endpoint.contains("/anthropic") {
        "anthropic".into()
    } else if endpoint.contains("generativelanguage.googleapis.com")
        || endpoint.contains("aiplatform.googleapis.com")
        || (endpoint.contains("googleapis.com")
            && !endpoint.contains("/openai")
            && !endpoint.contains("/compatible-mode"))
    {
        "gemini".into()
    } else {
        "openai".into()
    }
}

fn goose_provider_for_protocol(protocol: &str) -> String {
    match protocol.trim().to_lowercase().as_str() {
        "anthropic" => "anthropic".into(),
        "gemini" => "google".into(),
        _ => "openai".into(),
    }
}

/// 把 [`Config`] 的 Goose/OpenAI 配置注入环境变量（兼容 Goose openai provider 读取方式）。
///
/// Goose 的 openai provider 读取 `OPENAI_BASE_URL`/`OPENAI_API_KEY`。把 config.json 的值
/// 注入进程环境，使嵌入路径与 `examples/agent.rs` 的 env-driven 模式一致。
fn apply_goose_env(cfg: &Config) {
    set_or_remove_env("GOOSE_PROVIDER", &cfg.goose.provider);
    set_or_remove_env("GOOSE_MODEL", &cfg.goose.model);
    for key in [
        "OPENAI_BASE_URL",
        "OPENAI_HOST",
        "OPENAI_API_KEY",
        "ANTHROPIC_HOST",
        "ANTHROPIC_API_KEY",
        "GOOGLE_HOST",
        "GOOGLE_API_KEY",
    ] {
        std::env::remove_var(key);
    }

    match cfg.goose.provider.as_str() {
        "anthropic" => {
            set_or_remove_env(
                "ANTHROPIC_HOST",
                &normalize_provider_host(&cfg.goose.base_url, "anthropic"),
            );
            set_or_remove_env("ANTHROPIC_API_KEY", &cfg.goose.api_key);
        }
        "google" | "gemini" => {
            set_or_remove_env(
                "GOOGLE_HOST",
                &normalize_provider_host(&cfg.goose.base_url, "gemini"),
            );
            set_or_remove_env("GOOGLE_API_KEY", &cfg.goose.api_key);
        }
        _ => {
            set_or_remove_env("OPENAI_BASE_URL", &cfg.goose.base_url);
            set_or_remove_env("OPENAI_API_KEY", &cfg.goose.api_key);
        }
    }
    set_or_remove_env("BEAV_IMAGE_ENDPOINT", &cfg.image.endpoint);
    set_or_remove_env("BEAV_IMAGE_API_KEY", &cfg.image.api_key);
    set_or_remove_env("BEAV_IMAGE_MODEL", &cfg.image.model);
    set_or_remove_env("BEAV_VIDEO_ENDPOINT", &cfg.video.endpoint);
    set_or_remove_env("BEAV_VIDEO_API_KEY", &cfg.video.api_key);
    set_or_remove_env("BEAV_VIDEO_MODEL", &cfg.video.model);
}

fn set_or_remove_env(key: &str, value: &str) {
    if value.trim().is_empty() {
        std::env::remove_var(key);
    } else {
        std::env::set_var(key, value.trim());
    }
}

fn normalize_provider_host(base_url: &str, protocol: &str) -> String {
    let mut value = base_url.trim().trim_end_matches('/').to_string();
    if value.is_empty() {
        return value;
    }
    let suffixes: &[&str] = match protocol {
        "anthropic" => &["/v1/messages", "/v1/models", "/v1"],
        "gemini" => &["/v1beta/models", "/v1/models", "/v1beta", "/v1"],
        _ => &[],
    };
    let lower = value.to_lowercase();
    for suffix in suffixes {
        if lower.ends_with(suffix) {
            let keep = value.len().saturating_sub(suffix.len());
            value.truncate(keep);
            break;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_overlay_uses_default_ai_source() {
        let settings = json!({
            "api_endpoint": "https://api.example.com/v1",
            "api_key": "k",
            "model_name": "model-a",
            "default_ai_source_id": "source-a",
            "ai_sources_json": serde_json::to_string(&json!([
                { "id": "source-b", "protocol": "anthropic", "model": "claude" },
                { "id": "source-a", "protocol": "openai", "model": "model-a" }
            ])).unwrap()
        });
        let cfg = apply_settings_to_config(Config::default(), &settings);
        assert_eq!(cfg.goose.provider, "openai");
        assert_eq!(cfg.goose.model, "model-a");
        assert_eq!(cfg.goose.base_url, "https://api.example.com/v1");
        assert_eq!(cfg.goose.api_key, "k");
    }

    #[test]
    fn settings_overlay_maps_gemini_to_google_provider() {
        let settings = json!({
            "default_ai_source_id": "google",
            "ai_sources_json": serde_json::to_string(&json!([
                {
                    "id": "google",
                    "protocol": "gemini",
                    "baseURL": "https://generativelanguage.googleapis.com/v1beta",
                    "apiKey": "k",
                    "model": "gemini-2.5-pro"
                }
            ])).unwrap()
        });
        let cfg = apply_settings_to_config(Config::default(), &settings);
        assert_eq!(cfg.goose.provider, "google");
        assert_eq!(
            cfg.goose.base_url,
            "https://generativelanguage.googleapis.com/v1beta"
        );
        assert_eq!(
            normalize_provider_host(&cfg.goose.base_url, "gemini"),
            "https://generativelanguage.googleapis.com"
        );
    }

    #[test]
    fn settings_overlay_reads_model_limits_and_clamps_output() {
        let settings = json!({
            "default_ai_source_id": "source-a",
            "chat_max_tokens_default": 262144,
            "ai_sources_json": serde_json::to_string(&json!([{
                "id": "source-a",
                "protocol": "openai",
                "baseURL": "https://api.example.com/v1",
                "apiKey": "k",
                "model": "gpt-5.5",
                "modelsMeta": [{
                    "id": "gpt-5.5",
                    "contextLimit": 1050000,
                    "maxOutputTokens": 128000
                }]
            }])).unwrap()
        });
        let cfg = apply_settings_to_config(Config::default(), &settings);
        assert_eq!(cfg.goose.context_limit, Some(1_050_000));
        assert_eq!(cfg.goose.max_tokens, Some(128_000));
    }

    #[test]
    fn provider_error_text_becomes_structured_bridge_error() {
        let raw = concat!(
            "Ran into this error: Request failed: Stream decode error: ",
            "Failed to parse Responses stream event type response.completed: missing field `id` ",
            "(payload 28106 bytes).\n\n",
            "Please retry if you think this is a transient or recoverable error."
        );

        let event = classify_goose_error_text(raw).expect("error should be classified");
        match event {
            BridgeEvent::Error {
                message,
                detail,
                category,
            } => {
                assert_eq!(category, "response_decode");
                assert!(message.contains("响应格式"));
                assert!(!detail.contains("28106"));
                assert!(!detail.contains("response.completed"));
            }
            _ => panic!("expected structured bridge error"),
        }
    }

    #[test]
    fn ordinary_assistant_text_is_not_classified_as_error() {
        assert!(classify_goose_error_text("正常回答").is_none());
    }

    #[test]
    fn tool_end_uses_name_from_matching_start_event() {
        let mut names = HashMap::new();
        let mut start = BridgeEvent::ToolStart {
            call_id: "call-1".into(),
            name: "yunying-ops__list_capabilities".into(),
            input: json!({}),
        };
        normalize_tool_event_name(&mut names, &mut start);

        let mut end = BridgeEvent::ToolEnd {
            call_id: "call-1".into(),
            name: "tool".into(),
            output: json!({ "success": true }),
        };
        normalize_tool_event_name(&mut names, &mut end);

        assert!(matches!(
            end,
            BridgeEvent::ToolEnd { ref name, .. }
                if name == "yunying-ops__list_capabilities"
        ));
        assert!(names.is_empty());
    }

    #[test]
    fn tool_response_preserves_mcp_app_payload() {
        let mut result = rmcp::model::CallToolResult::success(vec![rmcp::model::Content::text(
            "打开短剧工作台",
        )]);
        result.structured_content = Some(json!({
            "project": { "projectId": "demo", "revision": 3 }
        }));
        result.meta = Some(
            serde_json::from_value(json!({
                "ui": { "resourceUri": "ui://openmontage/short-drama" }
            }))
            .unwrap(),
        );
        let mut message = Message::user().with_tool_response("call-app", Ok(result));
        let event = content_to_event(message.content.remove(0), false).unwrap();

        let BridgeEvent::ToolEnd { output, .. } = event else {
            panic!("expected tool-end event");
        };
        assert_eq!(output["content"], json!("打开短剧工作台"));
        assert_eq!(
            output["structuredContent"]["project"]["projectId"],
            json!("demo")
        );
        assert_eq!(
            output["_meta"]["ui"]["resourceUri"],
            json!("ui://openmontage/short-drama")
        );
        assert!(output["blocks"].is_array());
    }

    #[tokio::test]
    #[ignore = "requires target/release/yunying-ops-mcp"]
    async fn packaged_operations_mcp_exposes_tools() {
        let goose_root = tempfile::tempdir().unwrap();
        std::env::set_var("GOOSE_PATH_ROOT", goose_root.path());
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/release/yunying-ops-mcp");
        assert!(path.is_file(), "missing {}", path.display());
        let bridge = GooseBridge::default();
        bridge
            .register_operations_mcp(&path.to_string_lossy())
            .await
            .unwrap();
        let tools = bridge.tool_names().await.unwrap();
        assert!(tools.iter().any(|name| name.ends_with("list_capabilities")));
        assert!(tools.iter().any(|name| name.ends_with("generate_image")));
        assert!(tools.iter().any(|name| name.ends_with("upload_video")));
    }
}
