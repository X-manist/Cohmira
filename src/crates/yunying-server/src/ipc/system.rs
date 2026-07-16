//! `system` 命名空间 IPC 通道：应用版本/更新、剪贴板、文件、音频、日志、调试、通知、社交工具。
//!
//! 对应 Beav `desktop/electron/main.ts` 中前缀为 `app:` / `clipboard:` / `file:` / `audio:` /
//! `logs:` / `debug:` / `notifications:` / `socialTools:`（及其连字符别名 `social-tools:`）的
//! `ipcMain.handle`。由 [`super::dispatch_invoke`] 按前缀路由到这里。
//!
//! 设计取舍：
//! - 打开外部 URL/路径用 `webbrowser`（workspace 已有）；在 Finder/资源管理器中定位文件用
//!   平台命令（`open -R` / `explorer /select,` / `xdg-open`）。
//! - 剪贴板文本用 `arboard`（workspace 已有，`goose` 已用 `set_text`）。HTML 富文本格式为 TODO。
//! - 真实系统 API（音频采集、系统通知、自动更新、社交账号 check）当前返回结构完整的占位结果，
//!   通道名汇总在 [`super`] 的 `stub_channels` 说明里，待接入 `socialconnect` / `notify-rust` 等。
//! - 写/真实副作用默认尊重 `payload.dryRun`（=`true`）或 `payload.confirm`（=`false`）。
//! - DB 读写走 [`crate::db::Db`] 的通用 JSON 助手（`settings.social_tools_json`）。

use super::AppState;
use crate::db::Db;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use image::ImageOutputFormat;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::fs::OpenOptions;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 应用语义化版本号（对齐 `app.getVersion()`，Tauri 壳无打包版本时取此常量）。
const APP_VERSION: &str = "0.1.0";
static LOG_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// `system` 命名空间的双向通道。按通道全名 `match`；未知通道返回 `Err`。
///
/// 本命名空间无单向（`send`）通道，故不提供 `send`。所有触发类（打开浏览器/目录）也走
/// `invoke`，与 Beav 一致。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        // ---------------- app ----------------
        "app:get-version" => Ok(json!(APP_VERSION)),

        "app:check-update" => {
            // 真实自动更新需网络（GitHub Releases / electron-updater 对应物），下一步接入。
            // payload.force 仅为语义占位，不触发真实请求。
            let _force = payload
                .get("force")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(json!({
                "success": true,
                "hasUpdate": false,
                "checkedAt": now_ts(),
                "reason": "auto_update_not_configured",
            }))
        }

        "app:open-release-page" => {
            let url = release_url(&payload);
            if !is_http_url(&url) {
                return Ok(json!({ "success": false, "error": "Invalid release URL" }));
            }
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true, "url": url }));
            }
            match open_url(&url) {
                Ok(()) => Ok(json!({ "success": true })),
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }

        "app:open-path" => {
            let path = need_str(&payload, "path")?.trim().to_string();
            if path.is_empty() {
                return Ok(json!({ "success": false, "error": "path is required" }));
            }
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true, "path": path }));
            }
            // webbrowser::open 在 mac 上走 `open`，可打开文件/目录/应用（对齐 shell.openPath）。
            match open_url(&path) {
                Ok(()) => Ok(json!({ "success": true })),
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }

        // 启动迁移：Rust 壳不再需要从旧 markdown/Electron DB 迁移，恒返回 not-needed。
        "app:startup-migration-status" | "app:startup-migration-start" => Ok(json!({
            "status": "not-needed",
            "needsDbImport": false,
            "needsProjectUpgrade": false,
            "shouldShowModal": false,
            "progress": if channel.ends_with("start") { 1 } else { 0 },
            "legacyMarkdownCount": 0,
            "projectUpgradeCounts": Value::Null,
        })),

        // ---------------- clipboard ----------------
        "clipboard:read-text" => {
            let text = read_clipboard_text().unwrap_or_default();
            Ok(json!(text))
        }

        "clipboard:read-image" => match read_clipboard_image_data_url() {
            Ok((data_url, width, height)) => Ok(json!({
                "success": true,
                "dataUrl": data_url,
                "fileName": format!("clipboard-{}.png", now_ts()),
                "mimeType": "image/png",
                "width": width,
                "height": height,
            })),
            Err(error) => Ok(json!({
                "success": false,
                "reason": "clipboard_has_no_image",
                "error": error.to_string(),
            })),
        },

        "clipboard:write-html" => {
            let html = match payload.get("html").and_then(|v| v.as_str()) {
                Some(h) => h.trim().to_string(),
                None => return Ok(json!({ "success": false, "error": "html is required" })),
            };
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true }));
            }
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| strip_html(&html));
            match write_clipboard_text(&text) {
                Ok(()) => Ok(json!({ "success": true })),
                // TODO: 真正的 HTML 富文本格式（平台剪贴板 flavor），当前仅写纯文本回退。
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }

        // ---------------- file ----------------
        "file:show-in-folder" => {
            let source = need_str(&payload, "source")?.trim().to_string();
            if source.is_empty() {
                return Ok(json!({ "success": false, "error": "source is required" }));
            }
            // TODO: 对齐 Beav 的 allowed-roots 沙箱校验（isPathWithinRoots）。
            if !Path::new(&source).exists() {
                return Ok(json!({ "success": false, "error": "file not found" }));
            }
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true, "source": source }));
            }
            reveal_in_folder(&source).await;
            Ok(json!({ "success": true }))
        }

        "file:copy-image" => {
            let source = need_str(&payload, "source")?.trim().to_string();
            match copy_image_to_clipboard(&source) {
                Ok(()) => Ok(json!({ "success": true })),
                Err(error) => Ok(json!({
                    "success": false,
                    "reason": "copy_image_failed",
                    "error": error.to_string(),
                })),
            }
        }

        "file:save-as" => {
            // 需原生「另存为」对话框（Tauri dialog / rfd），下一步接入。占位返回。
            Ok(json!({
                "success": false,
                "reason": "native_dialog_required",
                "error": "save-as dialog not wired",
            }))
        }

        // ---------------- audio ----------------
        // 内置录音服务未启用；允许前端使用浏览器录音能力。系统采集 API 为下一步。
        "audio:get-capture-capability" => Ok(json!({
            "success": true,
            "available": false,
            "activeRecording": false,
            "platform": platform(),
            "reason": "host_unavailable",
            "message": "商媒运营助手内置录音服务未启用，已允许前端使用浏览器录音能力。",
        })),
        "audio:start-recording" | "audio:stop-recording" | "audio:cancel-recording" => Ok(json!({
            "success": false,
            "reason": "host_unavailable",
            "error": format!("商媒运营助手 audio action failed: host_unavailable"),
        })),

        // ---------------- logs ----------------
        "logs:get-status" => {
            let dir = logs_dir();
            let _ = std::fs::create_dir_all(&dir);
            Ok(json!({
                "enabled": true,
                "logDirectory": dir.to_string_lossy(),
                "reportDirectory": dir.join("reports").to_string_lossy(),
                "retentionDays": 7,
                "maxFileMb": 10,
                "recentPreviewLimit": 200,
                "uploadConfigured": false,
                "uploadEndpoint": null,
                "pendingCount": 0,
                "debugVerboseEnabled": false,
                "previousUncleanShutdown": false,
            }))
        }
        "logs:get-recent" => {
            let limit = payload_limit(&payload, 200);
            Ok(json!({ "lines": read_recent_lines_from(&logs_dir().join("yunying.log"), limit) }))
        }
        "logs:append-renderer" => match append_renderer_log_to(&logs_dir(), &payload) {
            Ok(path) => Ok(json!({
                "success": true,
                "path": path.to_string_lossy(),
            })),
            Err(error) => Ok(json!({
                "success": false,
                "error": error.to_string(),
            })),
        },
        "logs:list-pending-reports" => Ok(json!([])),
        "logs:set-upload-consent" | "logs:dismiss-report" => Ok(json!({
            "success": true,
        })),
        "logs:upload-report" => Ok(json!({
            "success": false,
            "error": "diagnostics upload is not configured",
        })),
        "logs:open-dir" | "debug:open-log-dir" => {
            if is_dry_run(&payload) {
                return Ok(
                    json!({ "success": true, "dryRun": true, "path": logs_dir().to_string_lossy() }),
                );
            }
            let _ = std::fs::create_dir_all(logs_dir());
            match open_url(&logs_dir().to_string_lossy()) {
                Ok(()) => Ok(json!({ "success": true, "path": logs_dir().to_string_lossy() })),
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }

        // ---------------- debug ----------------
        "debug:get-status" => {
            // Rust schema 暂未建 debug_log_enabled 列；恒报未启用，待 schema 补列后接 settings。
            Ok(json!({
                "enabled": false,
                "logDirectory": logs_dir().to_string_lossy(),
            }))
        }
        "debug:get-recent" => {
            let limit = payload_limit(&payload, 200);
            Ok(json!({ "lines": read_recent_lines_from(&logs_dir().join("debug.log"), limit) }))
        }
        "debug:get-runtime-summary" => Ok(json!({
            "generatedAt": now_ts(),
            "runtimeWarm": { "lastWarmedAt": 0, "entries": [] },
            "phase0": {
                "personaGeneration": { "count": 0, "byAdvisor": [], "recent": [] },
                "knowledgeIngest": { "count": 0, "byAdvisor": [], "recent": [] },
                "runtimeQueries": { "count": 0, "byAdvisor": [], "byMode": [], "recent": [] },
                "skillInvocations": { "count": 0, "bySkill": [], "recent": [] },
                "toolCalls": {
                    "count": 0, "successCount": 0, "successRate": 0,
                    "byAdvisor": [], "byTool": [], "recent": []
                }
            }
        })),

        // ---------------- notifications ----------------
        // 系统通知权限/展示待接 notify-rust + 平台权限 API。当前占位。
        "notifications:permission_state" => Ok(json!({
            "state": "unknown",
        })),
        "notifications:request_permission" => Ok(json!({
            "success": true,
            "state": "unknown",
            "granted": false,
            "reason": "system_notification_pending",
        })),
        "notifications:show_system" => {
            let _title = payload.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let _body = payload.get("body").and_then(|v| v.as_str()).unwrap_or("");
            Ok(json!({
                "success": false,
                "reason": "system_notification_pending",
                "error": "system notification not wired",
            }))
        }

        // ---------------- socialTools（兼容 social-tools: 连字符别名）----------------
        "socialTools:get-status" | "social-tools:get-status" => social_tools_get_status(&state.db),

        "socialTools:save-config" | "social-tools:save-config" => {
            let config = coerce_config(&payload);
            if is_dry_run(&payload) {
                return Ok(json!({ "success": true, "dryRun": true, "config": config }));
            }
            match write_social_config(&state.db, &config) {
                Ok(stored) => {
                    // 对齐 Beav broadcastSettingsUpdated() → 前端 listen('settings:updated')。
                    state
                        .emitter
                        .emit("settings:updated", json!({ "scope": "social_tools" }));
                    Ok(json!({ "success": true, "config": stored }))
                }
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }

        "socialTools:check-account" | "social-tools:check-account" => {
            // 真实账号 check 走 socialconnect crate（sau 子进程 + 网络），TODO。
            Ok(json!({
                "success": false,
                "reason": "socialconnect_pending",
                "error": "social account check not wired (socialconnect crate)",
                "accounts": []
            }))
        }

        "socialTools:open-social-cookies-dir" | "social-tools:open-social-cookies-dir" => {
            let roots = social_roots();
            if is_dry_run(&payload) {
                return Ok(json!({
                    "success": true,
                    "dryRun": true,
                    "path": roots.social_cookies_dir.to_string_lossy(),
                }));
            }
            if let Err(e) = std::fs::create_dir_all(&roots.social_cookies_dir) {
                return Ok(json!({
                    "success": false,
                    "path": roots.social_cookies_dir.to_string_lossy(),
                    "error": e.to_string(),
                }));
            }
            match open_url(&roots.social_cookies_dir.to_string_lossy()) {
                Ok(()) => Ok(json!({
                    "success": true,
                    "path": roots.social_cookies_dir.to_string_lossy(),
                })),
                Err(e) => Ok(json!({
                    "success": false,
                    "path": roots.social_cookies_dir.to_string_lossy(),
                    "error": e.to_string(),
                })),
            }
        }

        other => Err(anyhow::anyhow!("system 通道未实现: {other}")),
    }
}

// ---------------------------------------------------------------------------
// 剪贴板（arboard，文本与图片）
// ---------------------------------------------------------------------------

/// 读取系统剪贴板纯文本（对齐 `clipboard.readText()`）。失败返回 None。
fn read_clipboard_text() -> Option<String> {
    let mut cb = arboard::Clipboard::new().ok()?;
    cb.get_text().ok()
}

/// 写入系统剪贴板纯文本（对齐 `clipboard.writeText()`）。
fn write_clipboard_text(text: &str) -> anyhow::Result<()> {
    let mut cb = arboard::Clipboard::new().map_err(|e| anyhow::anyhow!("clipboard: {e}"))?;
    cb.set_text(text)
        .map_err(|e| anyhow::anyhow!("clipboard: {e}"))?;
    Ok(())
}

/// 读取系统剪贴板中的 RGBA 图片，并编码成可直接交给前端附件通道的 PNG data URL。
fn read_clipboard_image_data_url() -> anyhow::Result<(String, usize, usize)> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| anyhow::anyhow!("clipboard: {error}"))?;
    let image = clipboard
        .get_image()
        .map_err(|error| anyhow::anyhow!("clipboard image: {error}"))?;
    let width = image.width;
    let height = image.height;
    let rgba = image::RgbaImage::from_raw(width as u32, height as u32, image.bytes.into_owned())
        .ok_or_else(|| anyhow::anyhow!("invalid clipboard RGBA image"))?;
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut png, ImageOutputFormat::Png)
        .map_err(|error| anyhow::anyhow!("encode clipboard image: {error}"))?;
    Ok((
        format!(
            "data:image/png;base64,{}",
            BASE64_STANDARD.encode(png.into_inner())
        ),
        width,
        height,
    ))
}

/// 将本地图片或 data URL 解码为 RGBA 后写入系统剪贴板。
fn copy_image_to_clipboard(source: &str) -> anyhow::Result<()> {
    let bytes = read_image_source(source)?;
    let rgba = image::load_from_memory(&bytes)
        .map_err(|error| anyhow::anyhow!("decode image: {error}"))?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| anyhow::anyhow!("clipboard: {error}"))?;
    clipboard
        .set_image(arboard::ImageData {
            width: width as usize,
            height: height as usize,
            bytes: Cow::Owned(rgba.into_raw()),
        })
        .map_err(|error| anyhow::anyhow!("write clipboard image: {error}"))?;
    Ok(())
}

fn read_image_source(source: &str) -> anyhow::Result<Vec<u8>> {
    let trimmed = source.trim();
    if let Some(data) = trimmed.strip_prefix("data:") {
        let (metadata, encoded) = data
            .split_once(',')
            .ok_or_else(|| anyhow::anyhow!("invalid image data URL"))?;
        if !metadata.ends_with(";base64") {
            return Err(anyhow::anyhow!("only base64 image data URLs are supported"));
        }
        return BASE64_STANDARD
            .decode(
                encoded
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect::<String>(),
            )
            .map_err(|error| anyhow::anyhow!("decode image data URL: {error}"));
    }

    let path = local_path_from_source(trimmed)?;
    std::fs::read(&path).map_err(|error| anyhow::anyhow!("read image {}: {error}", path.display()))
}

fn local_path_from_source(source: &str) -> anyhow::Result<PathBuf> {
    let path = source
        .strip_prefix("file://")
        .or_else(|| source.strip_prefix("asset://localhost/"))
        .or_else(|| source.strip_prefix("http://asset.localhost/"))
        .or_else(|| source.strip_prefix("https://asset.localhost/"))
        .unwrap_or(source);
    let decoded = urlencoding::decode(path)
        .map_err(|error| anyhow::anyhow!("decode image path: {error}"))?
        .into_owned();
    let normalized = if source.starts_with("file://") || source.contains("asset.localhost/") {
        format!("/{}", decoded.trim_start_matches('/'))
    } else {
        decoded
    };
    let path = PathBuf::from(normalized);
    if !path.is_file() {
        return Err(anyhow::anyhow!("image file not found: {}", path.display()));
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// 浏览器 / Finder 打开
// ---------------------------------------------------------------------------

/// 用默认浏览器/应用打开 URL 或本地路径（对齐 `shell.openExternal` / `shell.openPath`）。
fn open_url(target: &str) -> anyhow::Result<()> {
    webbrowser::open(target).map_err(|e| anyhow::anyhow!("open failed: {e}"))?;
    Ok(())
}

/// 在文件管理器中定位文件（对齐 `shell.showItemInFolder`）。平台分支 + fire-and-forget。
async fn reveal_in_folder(path: &str) {
    use std::process::Command as StdCommand;
    let _ = match std::env::consts::OS {
        "macos" => StdCommand::new("open").args(["-R", path]).spawn(),
        "windows" => StdCommand::new("explorer.exe")
            .arg(format!("/select,{path}"))
            .spawn(),
        other => {
            let parent = Path::new(path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|| ".".to_string());
            // macoxide 之外的平台（linux/freebsd 等）尝试 xdg-open 父目录。
            let _ = other;
            StdCommand::new("xdg-open").arg(parent).spawn()
        }
    };
}

// ---------------------------------------------------------------------------
// 社交工具配置（settings.social_tools_json）
// ---------------------------------------------------------------------------

/// 读取并解析 `settings.social_tools_json`；为空或解析失败时返回默认配置。
fn read_social_config(db: &Db) -> anyhow::Result<Value> {
    let row = db.query_one_json(
        "SELECT social_tools_json AS cfg FROM settings WHERE id = 1",
        &[],
    )?;
    let raw = row
        .as_ref()
        .and_then(|r| r.get("cfg"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if raw.trim().is_empty() {
        return Ok(default_social_config());
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(v) if v.is_object() => Ok(normalize_social_config(&v)),
        _ => Ok(default_social_config()),
    }
}

/// 把配置对象序列化写入 `settings.social_tools_json`，返回（规整过 version 的）已存对象。
fn write_social_config(db: &Db, config: &Value) -> anyhow::Result<Value> {
    let stored = normalize_social_config(config);
    let s = serde_json::to_string(&stored)?;
    db.execute_json(
        "UPDATE settings SET social_tools_json = ?1 WHERE id = 1",
        &[Value::String(s)],
    )?;
    Ok(stored)
}

/// 从 payload 取配置对象：优先 `payload.config`，否则 payload 自身（须为对象）；否则默认值。
fn coerce_config(payload: &Value) -> Value {
    let raw = payload
        .get("config")
        .filter(|v| v.is_object())
        .unwrap_or(payload);
    if raw.is_object() {
        normalize_social_config(raw)
    } else {
        default_social_config()
    }
}

/// 深度合并默认配置，兼容旧版本只保存部分字段的 social_tools_json。
fn normalize_social_config(raw: &Value) -> Value {
    let mut normalized = default_social_config();
    merge_json_object(&mut normalized, raw);
    normalized["version"] = json!(1);
    normalized
}

fn merge_json_object(target: &mut Value, source: &Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) else {
        *target = source.clone();
        return;
    };
    for (key, value) in source {
        match (target.get_mut(key), value) {
            (Some(existing), Value::Object(_)) if existing.is_object() => {
                merge_json_object(existing, value);
            }
            _ => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

/// 构造 `socialTools:get-status` 的完整结构（config + roots + fs 存在性；API 健康度占位）。
fn social_tools_get_status(db: &Db) -> anyhow::Result<Value> {
    let cfg = read_social_config(db)?;
    let roots = social_roots();
    let api_url = cfg
        .get("mediaCrawler")
        .and_then(|m| m.get("apiUrl"))
        .cloned()
        .unwrap_or(json!("http://127.0.0.1:8080"));
    Ok(json!({
        "success": true,
        "config": cfg,
        "roots": {
            "repoRoot": roots.repo_root.to_string_lossy(),
            "mediaCrawlerRoot": roots.media_crawler_root.to_string_lossy(),
            "socialConnectionRoot": roots.social_connection_root.to_string_lossy(),
            "socialCookiesDir": roots.social_cookies_dir.to_string_lossy(),
            "mediaCrawlerBrowserDataDir": roots.media_crawler_browser_data_dir.to_string_lossy(),
        },
        "mediaCrawler": {
            "rootExists": roots.media_crawler_root.exists(),
            "browserDataExists": roots.media_crawler_browser_data_dir.exists(),
            "apiUrl": api_url,
            "apiHealthy": false,
            "apiError": "health_check_not_implemented",
        },
        "socialConnection": {
            "rootExists": roots.social_connection_root.exists(),
            "runtimeMode": "rust-cdp",
            "browserExecutable": null,
            "browserAvailable": false,
            "cookiesDirExists": roots.social_cookies_dir.exists(),
            "accounts": [],
            "discoveredAccounts": [],
        }
    }))
}

/// 社交工具默认配置（对齐 `normalizeSocialToolsConfig` 的关键默认值；其余字段前端可补）。
fn default_social_config() -> Value {
    let browser = std::env::var("SOCIAL_CONNECTION_BROWSER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_default();
    let data_dir = std::env::var("SOCIAL_CONNECTION_DATA_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_default();
    json!({
        "version": 1,
        "mediaCrawler": {
            "enabled": true,
            "apiUrl": "http://127.0.0.1:8080",
            "apiTimeoutMs": 30000,
            "defaultPlatform": "xhs",
            "defaultLoginType": "qrcode",
            "saveOption": "jsonl",
            "maxNotesCount": 20,
            "maxCommentsCount": 20,
            "proxyUrl": "",
            "cookies": { "xhs": "", "douyin": "", "bili": "" }
        },
        "socialConnection": {
            "enabled": true,
            "runtimeMode": "rust-cdp",
            "browserExecutable": browser,
            "dataDir": data_dir,
            "headless": false,
            "proxyUrl": "",
            "accounts": {
                "xiaohongshu": "default",
                "douyin": "default",
                "kuaishou": "default",
                "bilibili": "default",
                "tencent": "default",
                "youtube": "default"
            }
        },
        "goose": { "inlineMediaCrawler": false, "inlineSocialConnection": false }
    })
}

/// 社交工具文件系统根（对齐 `getSocialToolsRoots`，env 优先，回退到仓库默认布局）。
struct SocialRoots {
    repo_root: PathBuf,
    media_crawler_root: PathBuf,
    social_connection_root: PathBuf,
    social_cookies_dir: PathBuf,
    media_crawler_browser_data_dir: PathBuf,
}

fn social_roots() -> SocialRoots {
    let repo_root = env_path("YUNYING_REPO_ROOT").unwrap_or_else(|| PathBuf::from("."));
    let media_crawler_root =
        env_path("MEDIACRAWLER_ROOT").unwrap_or_else(|| repo_root.join("MediaCrawler"));
    let social_connection_root =
        env_path("SOCIAL_CONNECTION_ROOT").unwrap_or_else(|| repo_root.join("social-connection"));
    let social_data_root =
        env_path("SOCIAL_CONNECTION_DATA_DIR").unwrap_or_else(|| social_connection_root.clone());
    let social_cookies_dir = social_data_root.join("cookies");
    let media_crawler_browser_data_dir = env_path("MEDIACRAWLER_BROWSER_DATA_DIR")
        .or_else(|| env_path("MEDIACRAWLER_QRCODE_DIR"))
        .unwrap_or_else(|| media_crawler_root.join("browser_data"));
    SocialRoots {
        repo_root,
        media_crawler_root,
        social_connection_root,
        social_cookies_dir,
        media_crawler_browser_data_dir,
    }
}

// ---------------------------------------------------------------------------
// 日志读取（占位：读单个日志文件尾部 N 行）
// ---------------------------------------------------------------------------

/// 日志目录：env `YUNYING_LOG_DIR` > `YUNYING_DATA_DIR/logs` > `./logs`。
fn logs_dir() -> PathBuf {
    if let Ok(d) = std::env::var("YUNYING_LOG_DIR") {
        return PathBuf::from(d);
    }
    if let Ok(d) = std::env::var("YUNYING_DATA_DIR") {
        return PathBuf::from(d).join("logs");
    }
    PathBuf::from("logs")
}

fn append_renderer_log_to(dir: &Path, payload: &Value) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join("yunying.log");
    let level = payload
        .get("level")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| matches!(*value, "trace" | "debug" | "info" | "warn" | "error"))
        .unwrap_or("info");
    let category = bounded_log_text(payload.get("category").and_then(Value::as_str), 160);
    let event = bounded_log_text(payload.get("event").and_then(Value::as_str), 160);
    let message = bounded_log_text(payload.get("message").and_then(Value::as_str), 16_384);
    let mut record = json!({
        "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "source": "renderer",
        "level": level,
        "category": category,
        "event": event,
        "message": message,
        "fields": payload.get("fields").cloned().unwrap_or(Value::Null),
    });
    let mut line = serde_json::to_string(&record)?;
    if line.len() > 64 * 1024 {
        record["fields"] = json!({ "truncated": true });
        line = serde_json::to_string(&record)?;
    }
    line.push('\n');

    let _guard = LOG_WRITE_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("renderer log lock poisoned"))?;
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all(line.as_bytes())?;
    file.flush()?;
    Ok(path)
}

fn bounded_log_text(value: Option<&str>, max_chars: usize) -> String {
    value.unwrap_or("").trim().chars().take(max_chars).collect()
}

/// 读取文件最后 `limit`（1..=1000）行；文件缺失返回空。供 logs/debug:get-recent。
fn read_recent_lines_from(path: &Path, limit: usize) -> Vec<String> {
    let limit = limit.clamp(1, 1000);
    let s = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let lines: Vec<String> = s.lines().map(String::from).collect();
    let len = lines.len();
    if len <= limit {
        lines
    } else {
        lines[len - limit..].to_vec()
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 当前毫秒时间戳（`std::time::SystemTime`）。
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 操作系统标识（对齐 `process.platform`：macos/windows/linux…）。
fn platform() -> &'static str {
    std::env::consts::OS
}

/// 取必需字符串字段。
fn need_str<'a>(payload: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("缺少字段 {key}"))
}

/// payload 的 limit（默认 `def`，夹到 1..=1000）。
fn payload_limit(payload: &Value, def: usize) -> usize {
    let raw = payload
        .get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(def as i64);
    (raw.max(1) as usize).min(1000)
}

/// 是否 dry-run：`dryRun===true` 或 `confirm===false`。
fn is_dry_run(payload: &Value) -> bool {
    payload
        .get("dryRun")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || payload
            .get("confirm")
            .map(|v| v.as_bool() == Some(false))
            .unwrap_or(false)
}

/// 是否 http(s) URL（对齐 `isHttpUrl`）。
fn is_http_url(s: &str) -> bool {
    s.starts_with("http://") || s.starts_with("https://")
}

/// 发布页 URL：payload.url > env `YUNYING_RELEASES_URL` > 占位。
fn release_url(payload: &Value) -> String {
    payload
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("YUNYING_RELEASES_URL").ok())
        .unwrap_or_else(|| "https://github.com/".to_string())
}

/// 粗略去 HTML 标签（对齐 `html.replace(/<[^>]+>/g, ' ')`）。
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 读 env 为 `PathBuf`。
fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    /// DB：social_tools_json 写读往返（`Db::open_in_memory`）。
    #[test]
    fn social_config_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        // 空配置 → 默认值。
        let first = read_social_config(&db).unwrap();
        assert_eq!(first["version"], 1);
        assert_eq!(first["mediaCrawler"]["apiUrl"], "http://127.0.0.1:8080");

        // 写入自定义配置。
        let cfg = json!({ "mediaCrawler": { "enabled": false, "apiUrl": "http://host:9" } });
        let stored = write_social_config(&db, &cfg).unwrap();
        assert_eq!(stored["version"], 1, "写入应补 version=1");

        // 读回应保留写入值。
        let back = read_social_config(&db).unwrap();
        assert_eq!(back["mediaCrawler"]["enabled"], false);
        assert_eq!(back["mediaCrawler"]["apiUrl"], "http://host:9");
    }

    /// fs：日志尾部 N 行读取（临时目录）。
    #[test]
    fn read_recent_log_tail() {
        let path = std::env::temp_dir().join(format!("yunying_system_test_{}.log", now_ts()));
        std::fs::write(&path, "a\nb\nc\nd\n").unwrap();
        assert_eq!(
            read_recent_lines_from(&path, 2),
            vec!["c".to_string(), "d".to_string()]
        );
        // limit 超过行数时返回全部。
        assert_eq!(
            read_recent_lines_from(&path, 100),
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string()
            ]
        );
        // 文件不存在返回空。
        let _ = std::fs::remove_file(&path);
        assert!(read_recent_lines_from(&path, 10).is_empty());
    }

    #[test]
    fn append_renderer_log_writes_json_line() {
        let dir = std::env::temp_dir().join(format!("yunying_renderer_log_{}", now_ts()));
        let path = append_renderer_log_to(
            &dir,
            &json!({
                "level": "error",
                "category": "plugin.bridge",
                "event": "window.error",
                "message": "boom",
                "fields": { "line": 42 }
            }),
        )
        .unwrap();
        let lines = read_recent_lines_from(&path, 10);
        assert_eq!(lines.len(), 1);
        let record: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(record["source"], "renderer");
        assert_eq!(record["level"], "error");
        assert_eq!(record["message"], "boom");
        assert_eq!(record["fields"]["line"], 42);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 纯函数：URL 校验 / dry-run 判定 / HTML 去标签。
    #[test]
    fn helpers_pure() {
        assert!(is_http_url("https://x.com"));
        assert!(is_http_url("http://x.com"));
        assert!(!is_http_url("file:///x"));
        assert!(!is_http_url("javascript:alert(1)"));

        assert!(is_dry_run(&json!({ "dryRun": true })));
        assert!(is_dry_run(&json!({ "confirm": false })));
        assert!(!is_dry_run(&json!({ "dryRun": false, "confirm": true })));

        assert_eq!(strip_html("<b>hi</b> <i>there</i>"), "hi there");
        assert_eq!(strip_html("plain"), "plain");

        assert_eq!(payload_limit(&json!({ "limit": 5000 }), 200), 1000);
        assert_eq!(payload_limit(&json!({ "limit": 0 }), 200), 1);
        assert_eq!(payload_limit(&json!({}), 200), 200);
    }

    #[test]
    fn image_data_url_source_decodes_for_clipboard_copy() {
        let bytes = read_image_source("data:image/png;base64,aGVsbG8=").unwrap();
        assert_eq!(bytes, b"hello");
    }

    /// coerce_config：优先 config 子对象，回退 payload 自身，再回退默认值。
    #[test]
    fn coerce_config_picks_object() {
        let inner = json!({ "mediaCrawler": { "enabled": true } });
        let a = coerce_config(&json!({ "config": inner }));
        assert_eq!(a["mediaCrawler"]["enabled"], true);

        let b = coerce_config(&json!({ "version": 1, "goose": {} }));
        assert_eq!(b["version"], 1);

        // 非对象 → 默认配置。
        let c = coerce_config(&json!("oops"));
        assert_eq!(c["version"], 1);
        assert!(c["mediaCrawler"].is_object());
    }

    /// 子进程/外部应用：打开浏览器（CI/无头环境跳过）。
    #[test]
    #[ignore = "opens external browser/application"]
    fn open_url_invokes_webbrowser() {
        // 仅断言合法 URL 不 panic；真实打开行为由人工验证。
        let _ = open_url("https://example.com");
    }

    /// 子进程：reveal_in_folder 启动 Finder/Explorer（CI 跳过）。
    #[tokio::test]
    #[ignore = "spawns platform file manager"]
    async fn reveal_in_folder_smoke() {
        let path = std::env::temp_dir().join(format!("yunying_reveal_{}.txt", now_ts()));
        std::fs::write(&path, "x").unwrap();
        reveal_in_folder(&path.to_string_lossy()).await;
        let _ = std::fs::remove_file(&path);
    }
}
