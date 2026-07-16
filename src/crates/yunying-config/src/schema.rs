//! 强类型配置 schema，覆盖原 `.env` / `.env.operations.example` 的全部配置项。
//!
//! 加载优先级：默认值 ← `config.json` ← 环境变量（兼容旧 `.env`，平滑迁移）。
//! 敏感字段经 [`Config::redact`] 脱敏后再写 work-package / 日志。

use serde::{Deserialize, Serialize};

/// 配置根。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Goose / agent 运行时（OpenAI-compatible）。
    pub goose: GooseConfig,
    /// MediaCrawler 采集。
    pub mediacrawler: MediaCrawlerConfig,
    /// Social Connection / sau 发布。
    pub social: SocialConfig,
    /// OpenMontage 视频分析与渲染。
    pub openmontage: OpenMontageConfig,
    /// 图像生成（GPT image 2 / Seedream）。
    pub image: ImageConfig,
    /// 视频生成（Seedance / Ark Plan）。
    pub video: VideoConfig,
    /// 真实执行开关与人工确认口令（见 docs/17 §6）。
    pub safety: SafetyConfig,
    /// 本地服务端口。
    pub server: ServerConfig,
    /// 资产根目录（OPERATIONS_ASSET_ROOT）。
    pub asset_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GooseConfig {
    pub provider: String,
    pub model: String,
    pub operations_agent_model: String,
    pub base_url: String,
    pub api_key: String,
    /// Total model context window. `None` lets Goose resolve it from canonical metadata.
    pub context_limit: Option<usize>,
    /// Maximum completion/output tokens. `None` lets Goose resolve the model default.
    pub max_tokens: Option<i32>,
    pub mode: String,
    pub chat_timeout_ms: u64,
    pub media_chat_timeout_ms: u64,
    pub history_message_limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaCrawlerConfig {
    pub api_timeout_ms: u64,
    pub default_login_type: String,
    pub save_option: String,
    pub max_notes_count: usize,
    pub max_comments_count: usize,
    pub xhs_cookies: String,
    pub douyin_cookies: String,
    pub bili_cookies: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SocialConfig {
    pub sau_bin: String,
    pub accounts: std::collections::HashMap<String, String>,
    pub headless: bool,
    pub yt_proxy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenMontageConfig {
    pub vision_analyzer_provider: String,
    pub vision_analyzer_model: String,
    pub mcp_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageConfig {
    pub provider: String,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub size: String,
    pub quality: String,
    pub aspect_ratio: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub aspect_ratio: String,
    pub resolution: String,
    pub duration_seconds: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    pub real_confirm: String,
    pub run_real_crawler: bool,
    pub run_crawler_readback: bool,
    pub run_real_publish: bool,
    pub run_real_image: bool,
    pub run_real_video: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bridge_url: String,
    /// `0` 表示由操作系统分配临时端口；桌面主程序始终使用此安全模式。
    pub bridge_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            goose: GooseConfig::default(),
            mediacrawler: MediaCrawlerConfig::default(),
            social: SocialConfig::default(),
            openmontage: OpenMontageConfig::default(),
            image: ImageConfig::default(),
            video: VideoConfig::default(),
            safety: SafetyConfig::default(),
            server: ServerConfig::default(),
            asset_root: "media/generated".into(),
        }
    }
}

impl Default for GooseConfig {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: "gpt-5.5".into(),
            operations_agent_model: "gpt-5.5".into(),
            base_url: String::new(),
            api_key: String::new(),
            context_limit: None,
            max_tokens: None,
            mode: "auto".into(),
            chat_timeout_ms: 180_000,
            media_chat_timeout_ms: 1_800_000,
            history_message_limit: 40,
        }
    }
}

impl Default for MediaCrawlerConfig {
    fn default() -> Self {
        Self {
            api_timeout_ms: 30_000,
            default_login_type: "qrcode".into(),
            save_option: "json".into(),
            max_notes_count: 50,
            max_comments_count: 20,
            xhs_cookies: String::new(),
            douyin_cookies: String::new(),
            bili_cookies: String::new(),
        }
    }
}

impl Default for SocialConfig {
    fn default() -> Self {
        Self {
            sau_bin: String::new(),
            accounts: Default::default(),
            headless: true,
            yt_proxy: String::new(),
        }
    }
}

impl Default for OpenMontageConfig {
    fn default() -> Self {
        Self {
            vision_analyzer_provider: "openai".into(),
            vision_analyzer_model: "gpt-5.5".into(),
            mcp_timeout_ms: 1_800_000,
        }
    }
}

impl Default for ImageConfig {
    fn default() -> Self {
        Self {
            provider: "openai-compatible".into(),
            endpoint: String::new(),
            api_key: String::new(),
            model: "gpt-image-2".into(),
            size: "1024x1536".into(),
            quality: "high".into(),
            aspect_ratio: "3:4".into(),
        }
    }
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            endpoint: "https://ark.cn-beijing.volces.com/api/plan/v3".into(),
            api_key: String::new(),
            model: "doubao-seedance-1.5-pro".into(),
            aspect_ratio: "9:16".into(),
            resolution: "720p".into(),
            duration_seconds: 8,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bridge_url: "http://127.0.0.1".into(),
            bridge_port: 0,
        }
    }
}

impl Config {
    /// 从 JSON 文本加载。
    pub fn from_json(raw: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(raw)?)
    }

    /// 返回一份敏感字段脱敏后的副本（cookie/api_key 显示为 `[redacted:N]`）。
    pub fn redact(&self) -> Self {
        let mut c = self.clone();
        c.goose.api_key = redact(&c.goose.api_key);
        c.image.api_key = redact(&c.image.api_key);
        c.video.api_key = redact(&c.video.api_key);
        c.mediacrawler.xhs_cookies = redact(&c.mediacrawler.xhs_cookies);
        c.mediacrawler.douyin_cookies = redact(&c.mediacrawler.douyin_cookies);
        c.mediacrawler.bili_cookies = redact(&c.mediacrawler.bili_cookies);
        c
    }
}

fn redact(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    format!("[redacted:{} chars]", s.len())
}
