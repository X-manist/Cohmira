//! IPC 通道分发层（Tauri 壳的 `ipc_invoke` / `ipc_send` 路由到这里）。
//!
//! 前端（Beav renderer）已是 Tauri 形态：`invoke('ipc_invoke', {channel, payload})`
//! 与 `invoke('ipc_send', {channel, payload})`（见 `src/compat/tauri-core.ts`）。
//! 本模块按通道名前缀（`namespace:action`）路由到各命名空间子模块，替代 Electron 的 344 个 ipcMain。
//!
//! - [`dispatch_invoke`]：双向调用，返回 JSON。
//! - [`dispatch_send`]：单向触发，异步执行并通过 [`EventEmitter`] 推事件。
//!
//! 命名空间子模块：[`core`]（P0：db/spaces/chat/goose）、[`system`]（system 命名空间：
//! app/clipboard/file/audio/logs/debug/notifications/socialTools）；P1-P3 由各命名空间模块
//! （tasks/memory/...）补充。

// 各命名空间在这里统一路由；具体能力状态由对应模块返回，避免前端收到静默占位结果。
pub mod accounts;
pub mod advisors;
pub mod authsys;
pub mod boss_sync;
pub mod content;
pub mod core;
pub mod data;
pub mod devtools;
pub mod generation;
pub mod knowledge;
pub mod redclaw;
pub mod redclaw_runner;
pub mod runtime;
pub mod social;
pub mod system;
pub mod tasks;

use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::db::Db;
use crate::goose_bridge::GooseBridge;

/// 应用共享状态。Tauri `State` / axum `State` 持有 `Arc<AppState>`。
pub struct AppState {
    pub db: Db,
    pub goose: GooseBridge,
    pub emitter: Arc<dyn EventEmitter>,
    /// 账号绑定 · 扫码登录服务（设置页绑定 mediacrawler/socialconnect 账号）。
    pub login: Arc<crate::login::LoginService>,
    /// RedClaw 的 Rust/Tokio 持久化调度服务。
    pub redclaw_scheduler: redclaw_runner::RedClawScheduler,
}

/// 事件发射器（对齐前端 `listen(channel, cb)`）。Tauri 实现走 `app_handle.emit`。
pub trait EventEmitter: Send + Sync {
    fn emit(&self, channel: &str, payload: Value);
}

/// 空实现（测试/无 UI 时用）。
pub struct NoopEmitter;
impl EventEmitter for NoopEmitter {
    fn emit(&self, _channel: &str, _payload: Value) {}
}

/// 记录事件到 `Arc<Mutex<Vec>>` 的实现（测试用）。
pub struct RecordingEmitter {
    pub events: Mutex<Vec<(String, Value)>>,
}
impl Default for RecordingEmitter {
    fn default() -> Self {
        Self::new()
    }
}
impl RecordingEmitter {
    pub fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }
}
impl EventEmitter for RecordingEmitter {
    fn emit(&self, channel: &str, payload: Value) {
        let mut e = self.events.blocking_lock();
        e.push((channel.to_string(), payload));
    }
}

/// 取通道的命名空间前缀（`chat:send-message` → `chat`）。
fn namespace(channel: &str) -> &str {
    channel.split(':').next().unwrap_or("")
}

/// 双向调用分发。按命名空间前缀路由；未实现的命名空间返回明确错误。
pub async fn dispatch_invoke(
    channel: &str,
    payload: Value,
    state: &AppState,
) -> anyhow::Result<Value> {
    match namespace(channel) {
        "db" | "settings" | "spaces" | "chat" | "goose" => {
            core::invoke(channel, payload, state).await
        }
        "tasks" | "work" | "subjects" | "background-tasks" | "background-workers" => {
            tasks::invoke(channel, payload, state).await
        }
        "memory" | "archives" | "manuscripts" => content::invoke(channel, payload, state).await,
        "knowledge" => knowledge::invoke(channel, payload, state).await,
        "generation" | "image-gen" | "video-gen" | "media" | "cover" => {
            generation::invoke(channel, payload, state).await
        }
        "sessions" | "runtime" | "session-bridge" | "mcp" => {
            runtime::invoke(channel, payload, state).await
        }
        "advisors" => advisors::invoke(channel, payload, state).await,
        "redclaw" => redclaw::invoke(channel, payload, state).await,
        "chatrooms" | "wander" | "assistant" | "wechat-official" => {
            social::invoke(channel, payload, state).await
        }
        "skills" | "tools" | "plugin" | "plugins" | "cli-runtime" => {
            devtools::invoke(channel, payload, state).await
        }
        "embedding" | "similarity" | "indexing" | "youtube" | "ai" => {
            data::invoke(channel, payload, state).await
        }
        "auth" | "redbox-auth" | "videoEditorV2" => authsys::invoke(channel, payload, state).await,
        "boss-sync" => boss_sync::invoke(channel, payload, state).await,
        // 账号绑定（扫码登录 + 账号池）：mediacrawler/socialconnect 账号在设置页绑定。
        "social-tools" | "socialTools" => accounts::invoke(channel, payload, state).await,
        "app" | "clipboard" | "file" | "audio" | "logs" | "debug" | "notifications" => {
            system::invoke(channel, payload, state).await
        }
        other => Err(anyhow::anyhow!(
            "IPC 通道未实现（命名空间 {other}）：{channel}"
        )),
    }
}

/// 单向调用分发（fire-and-forget，异步执行 + 推事件）。
pub async fn dispatch_send(
    channel: &str,
    payload: Value,
    state: Arc<AppState>,
) -> anyhow::Result<()> {
    match namespace(channel) {
        "chat" | "goose" => core::send(channel, payload, state).await,
        other => Err(anyhow::anyhow!(
            "IPC send 通道未实现（命名空间 {other}）：{channel}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_emitter_records() {
        let e = RecordingEmitter::new();
        e.emit("runtime:event", serde_json::json!({"type": "done"}));
        let evs = e.events.blocking_lock();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].0, "runtime:event");
    }

    #[test]
    fn namespace_prefix() {
        assert_eq!(namespace("chat:send-message"), "chat");
        assert_eq!(namespace("db:get-settings"), "db");
        assert_eq!(namespace("plain"), "plain");
    }

    #[test]
    fn bridge_event_payload_shape() {
        use crate::goose_bridge::BridgeEvent;
        let p = core::bridge_event_payload("s1", &BridgeEvent::TextDelta("hi".into()));
        assert_eq!(p["eventType"], serde_json::json!("runtime:text-delta"));
        assert_eq!(p["payload"]["content"], serde_json::json!("hi"));
        assert_eq!(p["payload"]["stream"], serde_json::json!("response"));
    }
}
