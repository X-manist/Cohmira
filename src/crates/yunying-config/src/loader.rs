//! 配置加载器：默认值 ← `config.json` ← 环境变量。
//!
//! 兼容旧 `.env`：若 `config.json` 不存在，则从环境变量构造（key 与 `.env.operations.example` 对齐），
//! 使桌面应用可在迁移期直接复用现有 `.env`，平滑切换到 `config.json`。

use crate::Config;
use std::path::{Path, PathBuf};

/// 加载配置。
///
/// - `config_path`：显式指定 `config.json` 路径；为 `None` 时按
///   `YUNYING_CONFIG_PATH` → `./config.json` → 应用数据目录查找。
/// - 失败兜底：找不到文件时返回带环境变量覆盖的默认值（不 panic）。
pub fn load(config_path: Option<&Path>) -> anyhow::Result<Config> {
    let mut cfg = match resolve_path(config_path) {
        Some(p) if p.exists() => Config::from_json(&std::fs::read_to_string(&p)?)?,
        _ => Config::default(),
    };
    apply_env_overrides(&mut cfg);
    Ok(cfg)
}

/// 解析配置文件路径：显式 → `YUNYING_CONFIG_PATH` → 当前目录 → 应用数据目录。
pub fn resolve_path(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    if let Some(path) = std::env::var_os("YUNYING_CONFIG_PATH") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    let cwd = PathBuf::from("config.json");
    if cwd.exists() {
        return Some(cwd);
    }
    dirs::config_dir().map(|d| d.join("yunyingagent").join("config.json"))
}

/// 用环境变量覆盖关键配置项（兼容旧 `.env`）。缺失项保持默认/文件值。
pub fn apply_env_overrides(cfg: &mut Config) {
    if let Ok(v) = std::env::var("OPENAI_BASE_URL") {
        cfg.goose.base_url = v;
    }
    if let Ok(v) = std::env::var("OPENAI_API_KEY") {
        cfg.goose.api_key = v;
    }
    if let Ok(v) = std::env::var("GOOSE_PROVIDER") {
        cfg.goose.provider = v;
    }
    if let Ok(v) = std::env::var("GOOSE_MODEL") {
        cfg.goose.model = v;
    }
    if let Ok(v) = std::env::var("OPERATIONS_AGENT_MODEL") {
        cfg.goose.operations_agent_model = v;
    }

    if let Ok(v) = std::env::var("MEDIACRAWLER_XHS_COOKIES") {
        cfg.mediacrawler.xhs_cookies = v;
    }
    if let Ok(v) = std::env::var("MEDIACRAWLER_DOUYIN_COOKIES") {
        cfg.mediacrawler.douyin_cookies = v;
    }
    if let Ok(v) = std::env::var("MEDIACRAWLER_BILI_COOKIES") {
        cfg.mediacrawler.bili_cookies = v;
    }

    if let Ok(v) = std::env::var("BEAV_VIDEO_ENDPOINT") {
        cfg.video.endpoint = v;
    }
    if let Ok(v) = std::env::var("BEAV_VIDEO_API_KEY") {
        cfg.video.api_key = v;
    }
    if let Ok(v) = std::env::var("BEAV_VIDEO_MODEL") {
        cfg.video.model = v;
    }

    if let Ok(v) = std::env::var("BEAV_IMAGE_ENDPOINT") {
        cfg.image.endpoint = v;
    }
    if let Ok(v) = std::env::var("BEAV_IMAGE_API_KEY") {
        cfg.image.api_key = v;
    }
    if let Ok(v) = std::env::var("BEAV_IMAGE_MODEL") {
        cfg.image.model = v;
    }

    cfg.safety.run_real_crawler = flag("RUN_REAL_CRAWLER", cfg.safety.run_real_crawler);
    cfg.safety.run_real_publish = flag("RUN_REAL_PUBLISH", cfg.safety.run_real_publish);
    cfg.safety.run_real_image = flag("RUN_REAL_IMAGE", cfg.safety.run_real_image);
    cfg.safety.run_real_video = flag("RUN_REAL_VIDEO", cfg.safety.run_real_video);
}

fn flag(key: &str, default: bool) -> bool {
    match std::env::var(key) {
        Ok(v) => matches!(v.trim(), "1" | "true" | "TRUE" | "yes"),
        Err(_) => default,
    }
}
