//! 运行时组装：config → Goose 桥 + 工具。
//!
//! 在进程启动时构造 [`Runtime`]，持有 [`GooseBridge`] 与配置，
//! 供 [`crate::http`] 的 axum 路由共享。

use std::sync::Arc;

use crate::goose_bridge::GooseBridge;
use yunying_config::Config;

/// 桌面后端运行时。`Arc` 共享给 axum handler。
pub struct Runtime {
    pub config: Config,
    pub goose: GooseBridge,
}

impl Runtime {
    /// 从 [`Config`] 组装：加载配置 → 构造 Goose 桥（建 agent + session + provider）。
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let goose = GooseBridge::new(&config).await?;
        Ok(Self { config, goose })
    }

    /// 包成 `Arc` 供 axum state 共享。
    pub fn shared(self) -> Arc<Self> {
        Arc::new(self)
    }
}
