//! 生成/媒体/封面命名空间：`generation` / `image-gen` / `video-gen` / `media` / `cover`。
//!
//! - `generation:*`：内存任务池（`static Mutex<HashMap<job_id, Job>>`）。submit 建 job、emit
//!   `generation:job-updated`；list/get/cancel/retry/await 操作池。真实生图/视频调 config 的
//!   image/video endpoint（v1 dry-run 计划，真实调用标 TODO）。
//! - `media:*`：素材库，文件系统（asset_root 从 settings 取，默认 media/generated）。
//! - `cover:*`：封面占位（同 media 目录，templates 用内存态）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{AppState, EventEmitter};

static JOBS: Mutex<Option<HashMap<String, Value>>> = Mutex::new(None);
static MEDIA_THUMBNAIL_BACKFILL_RUNNING: AtomicBool = AtomicBool::new(false);

fn jobs() -> std::sync::MutexGuard<'static, Option<HashMap<String, Value>>> {
    let mut g = JOBS.lock().unwrap();
    if g.is_none() {
        *g = Some(HashMap::new());
    }
    g
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_job_id() -> String {
    format!("job-{}-{}", now_ts(), uuid::Uuid::new_v4().simple())
}

#[derive(Clone)]
struct GenerationRuntime {
    media_root: PathBuf,
    config: yunying_config::Config,
    emitter: Arc<dyn EventEmitter>,
}

#[derive(Clone, Copy)]
enum RealGenerationKind {
    Image,
    Video,
}

impl RealGenerationKind {
    fn safety_enabled(self, config: &yunying_config::Config) -> bool {
        match self {
            Self::Image => config.safety.run_real_image,
            Self::Video => config.safety.run_real_video,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Image => "图片生成",
            Self::Video => "视频生成",
        }
    }

    fn config_key(self) -> &'static str {
        match self {
            Self::Image => "safety.run_real_image",
            Self::Video => "safety.run_real_video",
        }
    }
}

fn ensure_real_generation_enabled(
    runtime: &GenerationRuntime,
    kind: RealGenerationKind,
) -> anyhow::Result<()> {
    if kind.safety_enabled(&runtime.config) {
        return Ok(());
    }
    anyhow::bail!(
        "安全策略已阻止真实{}：config.json 中 {}=false",
        kind.label(),
        kind.config_key()
    )
}

fn blocked_generation_response(error: anyhow::Error) -> Value {
    json!({
        "success": false,
        "blocked": true,
        "reason": "safety_disabled",
        "error": error.to_string(),
        "assets": [],
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MediaAsset {
    id: String,
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    aspect_ratio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requested_size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound_manuscript_path: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MediaCatalog {
    version: u8,
    assets: Vec<MediaAsset>,
}

const MEDIA_CATALOG_VERSION: u8 = 2;

impl Default for MediaCatalog {
    fn default() -> Self {
        Self {
            version: MEDIA_CATALOG_VERSION,
            assets: Vec::new(),
        }
    }
}

fn media_root(state: &AppState) -> PathBuf {
    let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
    crate::workspace::resolve(&settings).media
}

fn media_catalog_path(root: &Path) -> PathBuf {
    root.join("catalog.json")
}

fn ensure_media_dirs(root: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(root.join("generated"))?;
    fs::create_dir_all(root.join("imported"))?;
    fs::create_dir_all(root.join("external"))?;
    fs::create_dir_all(root.join(".thumbnails"))?;
    Ok(())
}

fn now_iso() -> String {
    Utc::now().to_rfc3339()
}

fn read_media_catalog(root: &Path) -> anyhow::Result<MediaCatalog> {
    ensure_media_dirs(root)?;
    let path = media_catalog_path(root);
    let mut catalog = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<MediaCatalog>(&raw).ok())
        .unwrap_or_default();

    let mut changed = false;
    if catalog.version < MEDIA_CATALOG_VERSION {
        for asset in &mut catalog.assets {
            reconcile_image_asset_dimensions(root, asset);
        }
        catalog.version = MEDIA_CATALOG_VERSION;
        changed = true;
    }
    for absolute_path in discover_media_files(root) {
        let Ok(relative) = absolute_path.strip_prefix(root) else {
            continue;
        };
        let relative_path = normalize_store_path(relative);
        if catalog
            .assets
            .iter()
            .any(|asset| asset.relative_path.as_deref() == Some(relative_path.as_str()))
        {
            continue;
        }
        let metadata = fs::metadata(&absolute_path).ok();
        let timestamp = metadata
            .as_ref()
            .and_then(|meta| meta.modified().ok())
            .map(system_time_iso)
            .unwrap_or_else(now_iso);
        let source = if relative_path.starts_with("generated/") {
            "generated"
        } else if relative_path.starts_with("external/") {
            "external"
        } else {
            "imported"
        };
        catalog.assets.push(MediaAsset {
            id: format!("media_{}", uuid::Uuid::new_v4().simple()),
            source: source.into(),
            project_id: None,
            delivery_role: None,
            title: absolute_path
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            prompt: None,
            provider: None,
            provider_template: None,
            model: None,
            aspect_ratio: None,
            size: None,
            requested_size: None,
            quality: None,
            mime_type: Some(guess_mime_type(&absolute_path)),
            relative_path: Some(relative_path),
            bound_manuscript_path: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        });
        changed = true;
    }
    if changed || !path.exists() {
        write_media_catalog(root, &catalog)?;
    }
    Ok(catalog)
}

fn reconcile_image_asset_dimensions(root: &Path, asset: &mut MediaAsset) -> bool {
    let Some(relative_path) = asset.relative_path.as_deref() else {
        return false;
    };
    let mime_type = asset
        .mime_type
        .clone()
        .unwrap_or_else(|| guess_mime_type(Path::new(relative_path)));
    if !mime_type.starts_with("image/") || mime_type == "image/svg+xml" {
        return false;
    }
    let Ok((width, height)) = image::image_dimensions(root.join(relative_path)) else {
        return false;
    };
    let physical_size = format!("{width}x{height}");
    if asset.size.as_deref() == Some(physical_size.as_str()) {
        return false;
    }
    if asset.requested_size.is_none() {
        asset.requested_size = asset.size.take();
    }
    asset.size = Some(physical_size);
    asset.updated_at = now_iso();
    true
}

fn write_media_catalog(root: &Path, catalog: &MediaCatalog) -> anyhow::Result<()> {
    ensure_media_dirs(root)?;
    fs::write(
        media_catalog_path(root),
        serde_json::to_string_pretty(catalog)?,
    )?;
    Ok(())
}

fn discover_media_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();
            if path == media_catalog_path(root)
                || file_name == "catalog.json.lock"
                || file_name.starts_with("catalog.json.")
            {
                continue;
            }
            match entry.file_type() {
                Ok(kind) if kind.is_dir() && file_name != ".thumbnails" => pending.push(path),
                Ok(kind)
                    if kind.is_file() && guess_mime_type(&path) != "application/octet-stream" =>
                {
                    out.push(path)
                }
                _ => {}
            }
        }
    }
    out
}

fn system_time_iso(value: std::time::SystemTime) -> String {
    let value: DateTime<Utc> = value.into();
    value.to_rfc3339()
}

fn normalize_store_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn normalize_relative_path(value: &str) -> anyhow::Result<String> {
    let normalized = value.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized == "."
        || normalized == ".."
        || normalized.starts_with('/')
        || normalized.starts_with("../")
        || normalized.contains("/../")
    {
        return Err(anyhow::anyhow!("Invalid relative path"));
    }
    Ok(normalized.trim_start_matches("./").to_string())
}

fn guess_mime_type(path: &Path) -> String {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "m4v" => "video/x-m4v",
        "avi" => "video/x-msvideo",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "opus" => "audio/opus",
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        _ => "application/octet-stream",
    }
    .into()
}

fn enrich_media_asset(root: &Path, asset: &MediaAsset) -> Value {
    let mut value = serde_json::to_value(asset).unwrap_or_else(|_| json!({}));
    let Some(relative_path) = asset.relative_path.as_deref() else {
        value["exists"] = json!(false);
        return value;
    };
    let absolute_path = root.join(relative_path);
    let exists = absolute_path.is_file();
    value["absolutePath"] = json!(absolute_path.to_string_lossy());
    value["previewUrl"] = json!(absolute_path.to_string_lossy());
    value["exists"] = json!(exists);
    if exists && is_video_media_asset(asset) {
        let thumbnail = video_thumbnail_path(root, asset);
        if thumbnail.is_file() {
            value["thumbnailUrl"] = json!(thumbnail.to_string_lossy());
        }
    }
    value
}

fn is_video_media_asset(asset: &MediaAsset) -> bool {
    asset
        .mime_type
        .as_deref()
        .map(|value| value.starts_with("video/"))
        .unwrap_or(false)
        || asset
            .relative_path
            .as_deref()
            .map(|value| {
                matches!(
                    Path::new(value)
                        .extension()
                        .and_then(|extension| extension.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .as_str(),
                    "mp4" | "mov" | "webm" | "m4v" | "avi" | "mkv"
                )
            })
            .unwrap_or(false)
}

fn video_thumbnail_path(root: &Path, asset: &MediaAsset) -> PathBuf {
    root.join(".thumbnails").join(format!("{}.jpg", asset.id))
}

fn ensure_video_thumbnail(root: &Path, asset: &MediaAsset, source: &Path) -> Option<PathBuf> {
    if fs::metadata(source).ok()?.len() < 32 {
        return None;
    }
    let target = video_thumbnail_path(root, asset);
    if target.is_file() {
        return Some(target);
    }
    fs::create_dir_all(target.parent()?).ok()?;
    let ffmpeg = std::env::var_os("FFMPEG_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ffmpeg"));
    let status = Command::new(ffmpeg)
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-ss",
            "0.25",
            "-i",
        ])
        .arg(source)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale=640:-2:force_original_aspect_ratio=decrease",
            "-q:v",
            "3",
        ])
        .arg(&target)
        .status()
        .ok()?;
    status.success().then_some(target)
}

fn backfill_video_thumbnails(root: &Path) -> usize {
    let Ok(catalog) = read_media_catalog(root) else {
        return 0;
    };
    catalog
        .assets
        .iter()
        .filter(|asset| is_video_media_asset(asset))
        .filter_map(|asset| {
            let relative_path = asset.relative_path.as_deref()?;
            let source = root.join(relative_path);
            let target = video_thumbnail_path(root, asset);
            if target.is_file() {
                return None;
            }
            ensure_video_thumbnail(root, asset, &source)
        })
        .count()
}

fn schedule_media_thumbnail_backfill(root: PathBuf, emitter: Arc<dyn EventEmitter>) {
    if MEDIA_THUMBNAIL_BACKFILL_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    tokio::spawn(async move {
        let generated = tokio::task::spawn_blocking(move || backfill_video_thumbnails(&root))
            .await
            .unwrap_or(0);
        MEDIA_THUMBNAIL_BACKFILL_RUNNING.store(false, Ordering::Release);
        if generated > 0 {
            let payload =
                json!({ "scope": "media", "action": "thumbnail-backfill", "generated": generated });
            emitter.emit("data:changed", payload.clone());
            emitter.emit("renderer:data-changed", payload);
        }
    });
}

fn list_media_assets(root: &Path, limit: usize) -> anyhow::Result<Vec<Value>> {
    let mut catalog = read_media_catalog(root)?;
    catalog
        .assets
        .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    Ok(catalog
        .assets
        .iter()
        .take(limit.max(1))
        .map(|asset| enrich_media_asset(root, asset))
        .collect())
}

fn sanitize_imported_file_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("asset");
    let mut out = String::new();
    for ch in stem.chars() {
        if ch.is_alphanumeric()
            || matches!(ch, '-' | '_' | '.')
            || ('\u{4e00}'..='\u{9fff}').contains(&ch)
        {
            out.push(ch);
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "asset".into()
    } else {
        trimmed.to_string()
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn normalize_prompt(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn import_media_files(
    root: &Path,
    paths: &[PathBuf],
    requested_source: &str,
) -> anyhow::Result<Vec<Value>> {
    let mut catalog = read_media_catalog(root)?;
    let mut imported = Vec::new();
    let source_kind = if requested_source == "external" {
        "external"
    } else {
        "imported"
    };
    let date_folder = Utc::now().format("%Y-%m-%d").to_string();
    let target_dir = root.join(source_kind).join(&date_folder);
    fs::create_dir_all(&target_dir)?;
    for source in paths {
        if !source.is_file() {
            continue;
        }
        let id = format!("media_{}", uuid::Uuid::new_v4().simple());
        let extension = source
            .extension()
            .and_then(|ext| ext.to_str())
            .filter(|ext| !ext.is_empty())
            .map(|ext| format!(".{ext}"))
            .unwrap_or_default();
        let target_name = format!("{id}_{}{}", sanitize_imported_file_name(source), extension);
        let relative_path = format!("{source_kind}/{date_folder}/{target_name}");
        let target = root.join(&relative_path);
        fs::copy(source, &target)?;
        let timestamp = now_iso();
        let asset = MediaAsset {
            id,
            source: source_kind.into(),
            project_id: None,
            delivery_role: None,
            title: source
                .file_stem()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            prompt: None,
            provider: None,
            provider_template: None,
            model: None,
            aspect_ratio: None,
            size: None,
            requested_size: None,
            quality: None,
            mime_type: Some(guess_mime_type(source)),
            relative_path: Some(relative_path),
            bound_manuscript_path: None,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        if is_video_media_asset(&asset) {
            let _ = ensure_video_thumbnail(root, &asset, &target);
        }
        imported.push(enrich_media_asset(root, &asset));
        catalog.assets.push(asset);
    }
    if !imported.is_empty() {
        write_media_catalog(root, &catalog)?;
    }
    Ok(imported)
}

fn open_path(path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let mut command = std::process::Command::new("xdg-open");

    let status = command.arg(path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("打开路径失败: {}", path.display()))
    }
}

fn generation_runtime(state: &AppState) -> GenerationRuntime {
    let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
    let config = crate::goose_bridge::apply_settings_to_config(
        yunying_config::load(None).unwrap_or_default(),
        &settings,
    );
    GenerationRuntime {
        media_root: crate::workspace::resolve(&settings).media,
        config,
        emitter: state.emitter.clone(),
    }
}

fn payload_string(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn payload_first_string(payload: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| payload_string(payload, key))
}

fn payload_delivery_role(payload: &Value) -> Option<String> {
    payload_first_string(
        payload,
        &["deliveryRole", "delivery_role", "assetRole", "asset_role"],
    )
}

fn sanitize_media_project_folder(value: &str) -> String {
    let mut output = String::new();
    for character in value.trim().chars() {
        if character.is_alphanumeric() {
            output.extend(character.to_lowercase());
        } else if !output.is_empty() && !output.ends_with('-') {
            output.push('-');
        }
        if output.chars().count() >= 80 {
            break;
        }
    }
    let output = output.trim_matches('-');
    if output.is_empty() {
        "untitled-project".into()
    } else {
        output.into()
    }
}

fn generated_asset_category(payload: &Value, mime_type: &str) -> &'static str {
    let role = payload_delivery_role(payload)
        .unwrap_or_default()
        .to_lowercase();
    if role.contains("final") || role.contains("output") {
        return "output";
    }
    if mime_type.starts_with("image/") {
        "images"
    } else if mime_type.starts_with("video/") {
        "clips"
    } else if mime_type.starts_with("audio/") {
        "audio"
    } else {
        "files"
    }
}

fn generated_asset_relative_path(payload: &Value, id: &str, mime_type: &str) -> String {
    let filename = format!("{id}.{}", mime_extension(mime_type));
    let Some(project_id) = payload_first_string(payload, &["projectId", "project_id"]) else {
        return format!("generated/{filename}");
    };
    format!(
        "generated/{}/{}/{}",
        sanitize_media_project_folder(&project_id),
        generated_asset_category(payload, mime_type),
        filename
    )
}

fn media_asset_category(asset: &MediaAsset) -> &'static str {
    let mime_type = asset
        .mime_type
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    let title = asset.title.as_deref().unwrap_or_default().to_lowercase();
    let role = asset
        .delivery_role
        .as_deref()
        .unwrap_or_default()
        .to_lowercase();
    let relative_path = asset
        .relative_path
        .as_deref()
        .unwrap_or_default()
        .replace('\\', "/")
        .to_lowercase();
    if mime_type.starts_with("video/")
        && (role.contains("final")
            || role.contains("output")
            || relative_path.contains("/output/")
            || title.starts_with("video_stitch:")
            || title.contains("最终")
            || title.contains("成片"))
    {
        return "output";
    }
    if mime_type.starts_with("image/") {
        "images"
    } else if mime_type.starts_with("video/") {
        "clips"
    } else if mime_type.starts_with("audio/") {
        "audio"
    } else {
        "files"
    }
}

fn move_generated_asset_to_project(
    root: &Path,
    asset: &mut MediaAsset,
    project_id: Option<String>,
) -> anyhow::Result<()> {
    if asset.source != "generated" {
        asset.project_id = project_id;
        return Ok(());
    }
    let Some(current_relative_path) = asset.relative_path.clone() else {
        asset.project_id = project_id;
        return Ok(());
    };
    let filename = Path::new(&current_relative_path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!(
                "{}.{}",
                asset.id,
                mime_extension(
                    asset
                        .mime_type
                        .as_deref()
                        .unwrap_or("application/octet-stream")
                )
            )
        });
    let next_relative_path = match project_id.as_deref() {
        Some(value) => format!(
            "generated/{}/{}/{}",
            sanitize_media_project_folder(value),
            media_asset_category(asset),
            filename
        ),
        None => format!("generated/{filename}"),
    };
    if current_relative_path != next_relative_path {
        let current_path = root.join(&current_relative_path);
        let next_path = root.join(&next_relative_path);
        if current_path.is_file() {
            if let Some(parent) = next_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if next_path.exists() {
                return Err(anyhow::anyhow!(
                    "目标素材文件已存在: {}",
                    next_path.display()
                ));
            }
            fs::rename(&current_path, &next_path).or_else(|_| -> std::io::Result<()> {
                fs::copy(&current_path, &next_path)?;
                fs::remove_file(&current_path)
            })?;
        }
        asset.relative_path = Some(next_relative_path);
    }
    asset.project_id = project_id;
    Ok(())
}

fn truncate_response_body(value: &str) -> String {
    let value = value.trim();
    if value.chars().count() <= 500 {
        return value.to_string();
    }
    let mut out: String = value.chars().take(500).collect();
    out.push_str("...");
    out
}

fn mime_extension(mime_type: &str) -> &'static str {
    match mime_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
        .as_str()
    {
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "video/quicktime" => "mov",
        "video/webm" => "webm",
        _ if mime_type.to_lowercase().starts_with("video/") => "mp4",
        _ => "png",
    }
}

fn decode_data_url(value: &str) -> anyhow::Result<(Vec<u8>, String)> {
    let Some((header, data)) = value.split_once(',') else {
        return Err(anyhow::anyhow!("Invalid data URL"));
    };
    if !header.contains(";base64") {
        return Err(anyhow::anyhow!("Only base64 data URLs are supported"));
    }
    let mime_type = header
        .strip_prefix("data:")
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = BASE64_STANDARD
        .decode(data.trim())
        .map_err(|error| anyhow::anyhow!("Invalid base64 asset: {error}"))?;
    Ok((bytes, mime_type))
}

fn normalize_video_reference(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.starts_with("https://") || value.starts_with("http://") || value.starts_with("data:") {
        return Ok(value.to_string());
    }

    let path_value = value
        .strip_prefix("file://")
        .or_else(|| value.strip_prefix("asset://localhost/"))
        .or_else(|| value.strip_prefix("redbox-asset://asset/"))
        .unwrap_or(value);
    let decoded = urlencoding::decode(path_value)
        .map_err(|error| anyhow::anyhow!("参考图路径解码失败: {error}"))?;
    let path = Path::new(decoded.as_ref());
    if !path.is_file() {
        return Err(anyhow::anyhow!(
            "参考图不存在或不是文件: {}",
            path.display()
        ));
    }
    let mime_type = guess_mime_type(path);
    if !mime_type.starts_with("image/") {
        return Err(anyhow::anyhow!("参考图格式不受支持: {mime_type}"));
    }
    let bytes = fs::read(path)
        .map_err(|error| anyhow::anyhow!("读取参考图失败（{}）: {error}", path.display()))?;
    Ok(format!(
        "data:{mime_type};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

fn extract_video_last_frame_reference(
    runtime: &GenerationRuntime,
    value: &str,
) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(anyhow::anyhow!("视频续写模式缺少上一段视频"));
    }

    let source = if value.starts_with("https://")
        || value.starts_with("http://")
        || value.starts_with("data:")
    {
        value.to_string()
    } else {
        let path_value = value
            .strip_prefix("file://")
            .or_else(|| value.strip_prefix("asset://localhost/"))
            .or_else(|| value.strip_prefix("redbox-asset://asset/"))
            .unwrap_or(value);
        let decoded = urlencoding::decode(path_value)
            .map_err(|error| anyhow::anyhow!("上一段视频路径解码失败: {error}"))?;
        let path = Path::new(decoded.as_ref());
        if !path.is_file() {
            return Err(anyhow::anyhow!(
                "上一段视频不存在或不是文件: {}",
                path.display()
            ));
        }
        let mime_type = guess_mime_type(path);
        if !mime_type.starts_with("video/") {
            return Err(anyhow::anyhow!("上一段视频格式不受支持: {mime_type}"));
        }
        path.to_string_lossy().into_owned()
    };

    let runtime_dir = runtime.media_root.join(".runtime").join("continuity");
    fs::create_dir_all(&runtime_dir)?;
    let output_path = runtime_dir.join(format!("last-frame-{}.png", uuid::Uuid::new_v4().simple()));
    let ffmpeg = std::env::var_os("FFMPEG_BIN").unwrap_or_else(|| "ffmpeg".into());
    let result = (|| -> anyhow::Result<String> {
        let output = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-nostdin",
                "-y",
                "-sseof",
                "-0.25",
                "-i",
            ])
            .arg(&source)
            .args(["-map", "0:v:0", "-frames:v", "1"])
            .arg(&output_path)
            .output()
            .map_err(|error| anyhow::anyhow!("提取上一段视频尾帧失败: {error}"))?;
        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "提取上一段视频尾帧失败: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let bytes = fs::read(&output_path).map_err(|error| {
            anyhow::anyhow!(
                "读取上一段视频尾帧失败（{}）: {error}",
                output_path.display()
            )
        })?;
        if bytes.is_empty() {
            return Err(anyhow::anyhow!("上一段视频尾帧为空"));
        }
        Ok(format!(
            "data:image/png;base64,{}",
            BASE64_STANDARD.encode(bytes)
        ))
    })();
    let _ = fs::remove_file(&output_path);
    result
}

async fn download_generated_asset(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    default_mime: &str,
) -> anyhow::Result<(Vec<u8>, String)> {
    if url.starts_with("data:") {
        return decode_data_url(url);
    }

    let mut last_error = String::new();
    for auth_mode in 0..3 {
        let mut request = client.get(url);
        if auth_mode == 1 && !api_key.is_empty() {
            request = request.bearer_auth(api_key);
        } else if auth_mode == 2 && !api_key.is_empty() {
            request = request.header("x-goog-api-key", api_key);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                let mime_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
                    .unwrap_or_else(|| default_mime.to_string());
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|error| anyhow::anyhow!("读取生成产物失败: {error}"))?;
                if bytes.is_empty() {
                    last_error = "生成产物内容为空".into();
                    continue;
                }
                return Ok((bytes.to_vec(), mime_type));
            }
            Ok(response) => {
                last_error = format!("HTTP {}", response.status());
            }
            Err(error) => {
                last_error = error.to_string();
            }
        }
    }
    Err(anyhow::anyhow!("下载生成产物失败: {last_error}"))
}

#[allow(clippy::too_many_arguments)]
fn persist_generated_asset(
    runtime: &GenerationRuntime,
    payload: &Value,
    bytes: &[u8],
    mime_type: &str,
    provider: &str,
    model: &str,
    aspect_ratio: Option<String>,
    requested_size: Option<String>,
    quality: Option<String>,
) -> anyhow::Result<Value> {
    if bytes.is_empty() {
        return Err(anyhow::anyhow!("生成产物内容为空"));
    }
    let mut catalog = read_media_catalog(&runtime.media_root)?;
    let id = format!("media_{}_{}", now_ts(), uuid::Uuid::new_v4().simple());
    let mime_type = mime_type
        .split(';')
        .next()
        .unwrap_or(mime_type)
        .trim()
        .to_lowercase();
    let relative_path = generated_asset_relative_path(payload, &id, &mime_type);
    let target = runtime.media_root.join(&relative_path);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&target, bytes)?;

    // `size` is part of the renderer/catalog contract and must describe the bytes that were
    // actually persisted. Providers may return a different resolution, and aspect correction
    // can crop it again, so retaining the requested value in `size` makes the catalog lie.
    let (size, requested_size) = if mime_type.starts_with("image/") && mime_type != "image/svg+xml"
    {
        let image = image::load_from_memory(bytes)
            .map_err(|error| anyhow::anyhow!("读取生成图片物理尺寸失败: {error}"))?;
        (
            Some(format!("{}x{}", image.width(), image.height())),
            requested_size,
        )
    } else {
        (requested_size, None)
    };

    let timestamp = now_iso();
    let prompt = payload_string(payload, "prompt");
    let title = payload_string(payload, "title").or_else(|| {
        prompt
            .as_ref()
            .map(|value| value.chars().take(40).collect())
    });
    let asset = MediaAsset {
        id,
        source: "generated".into(),
        project_id: payload_first_string(payload, &["projectId", "project_id"]),
        delivery_role: payload_delivery_role(payload),
        title,
        prompt,
        provider: Some(provider.to_string()),
        provider_template: payload_string(payload, "providerTemplate"),
        model: Some(model.to_string()),
        aspect_ratio,
        size,
        requested_size,
        quality,
        mime_type: Some(mime_type),
        relative_path: Some(relative_path),
        bound_manuscript_path: None,
        created_at: timestamp.clone(),
        updated_at: timestamp,
    };
    if is_video_media_asset(&asset) {
        let _ = ensure_video_thumbnail(&runtime.media_root, &asset, &target);
    }
    let enriched = enrich_media_asset(&runtime.media_root, &asset);
    catalog.assets.push(asset);
    write_media_catalog(&runtime.media_root, &catalog)?;
    Ok(enriched)
}

fn image_generation_urls(endpoint: &str) -> Vec<String> {
    let mut base = endpoint.trim().trim_end_matches('/').to_string();
    let lower = base.to_lowercase();
    for suffix in ["/chat/completions", "/responses", "/models"] {
        if lower.ends_with(suffix) {
            base.truncate(base.len().saturating_sub(suffix.len()));
            break;
        }
    }
    let lower = base.to_lowercase();
    if lower.ends_with("/images/generations") {
        return vec![base];
    }
    if lower.ends_with("/v1") || lower.ends_with("/openai") {
        return vec![format!("{base}/images/generations")];
    }

    // Standard OpenAI-compatible gateways usually expose their API below /v1,
    // while a few older proxies use a versionless route. Prefer /v1 so a web UI
    // mounted at the origin cannot swallow the generation request.
    vec![
        format!("{base}/v1/images/generations"),
        format!("{base}/images/generations"),
    ]
}

fn image_request_timeout() -> Duration {
    std::env::var("YUNYING_IMAGE_REQUEST_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(20 * 60))
}

fn parse_aspect_ratio(value: &str) -> Option<(u32, u32)> {
    let (width, height) = value.trim().split_once(':')?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

fn normalize_generated_image_aspect(
    bytes: Vec<u8>,
    mime_type: String,
    aspect_ratio: Option<&str>,
) -> anyhow::Result<(Vec<u8>, String)> {
    let Some((ratio_width, ratio_height)) = aspect_ratio.and_then(parse_aspect_ratio) else {
        return Ok((bytes, mime_type));
    };
    let image = image::load_from_memory(&bytes)
        .map_err(|error| anyhow::anyhow!("读取生成图片尺寸失败: {error}"))?;
    let width = image.width();
    let height = image.height();
    let current_scaled_width = u64::from(width) * u64::from(ratio_height);
    let target_scaled_width = u64::from(height) * u64::from(ratio_width);
    let difference = current_scaled_width.abs_diff(target_scaled_width);
    if difference <= current_scaled_width.max(target_scaled_width) / 100 {
        return Ok((bytes, mime_type));
    }

    let cropped = if current_scaled_width > target_scaled_width {
        let crop_width = ((u64::from(height) * u64::from(ratio_width)) / u64::from(ratio_height))
            .clamp(1, u64::from(width)) as u32;
        image.crop_imm((width - crop_width) / 2, 0, crop_width, height)
    } else {
        let crop_height = ((u64::from(width) * u64::from(ratio_height)) / u64::from(ratio_width))
            .clamp(1, u64::from(height)) as u32;
        image.crop_imm(0, (height - crop_height) / 2, width, crop_height)
    };
    let mut output = std::io::Cursor::new(Vec::new());
    cropped
        .write_to(&mut output, image::ImageOutputFormat::Png)
        .map_err(|error| anyhow::anyhow!("校正生成图片比例失败: {error}"))?;
    Ok((output.into_inner(), "image/png".into()))
}

async fn generate_images_to_media_library(
    runtime: &GenerationRuntime,
    payload: &Value,
) -> anyhow::Result<Vec<Value>> {
    // Defense in depth: all current and future callers must pass the safety gate before any
    // provider validation or HTTP request can happen.
    ensure_real_generation_enabled(runtime, RealGenerationKind::Image)?;
    let prompt =
        payload_string(payload, "prompt").ok_or_else(|| anyhow::anyhow!("Prompt is required"))?;
    let reference_count = payload
        .get("referenceImages")
        .and_then(|value| value.as_array())
        .map(Vec::len)
        .unwrap_or(0);
    if reference_count > 0 {
        return Err(anyhow::anyhow!(
            "当前 Rust 图片后端先支持文生图；参考图模式尚未迁移"
        ));
    }

    let endpoint = payload_string(payload, "endpoint")
        .or_else(|| non_empty_string(&runtime.config.image.endpoint))
        .or_else(|| non_empty_string(&runtime.config.goose.base_url))
        .ok_or_else(|| anyhow::anyhow!("Image endpoint is missing"))?;
    let api_key = payload_string(payload, "apiKey")
        .or_else(|| non_empty_string(&runtime.config.image.api_key))
        .or_else(|| non_empty_string(&runtime.config.goose.api_key))
        .ok_or_else(|| anyhow::anyhow!("Image API key is missing"))?;
    let model = payload_string(payload, "model")
        .or_else(|| non_empty_string(&runtime.config.image.model))
        .unwrap_or_else(|| "gpt-image-2".into());
    let provider = payload_string(payload, "provider")
        .or_else(|| non_empty_string(&runtime.config.image.provider))
        .unwrap_or_else(|| "openai-compatible".into());
    let count = payload
        .get("count")
        .and_then(|value| value.as_u64())
        .unwrap_or(1)
        .clamp(1, 4) as usize;
    let size =
        payload_string(payload, "size").or_else(|| non_empty_string(&runtime.config.image.size));
    let quality = payload_string(payload, "quality")
        .or_else(|| non_empty_string(&runtime.config.image.quality));
    let aspect_ratio = payload_string(payload, "aspectRatio")
        .or_else(|| non_empty_string(&runtime.config.image.aspect_ratio));

    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "n": count,
        "response_format": "url",
    });
    if let Some(value) = size.as_ref() {
        body["size"] = json!(value);
    }
    if let Some(value) = quality.as_ref() {
        body["quality"] = json!(value);
    }

    let client = reqwest::Client::builder()
        .timeout(image_request_timeout())
        .build()?;
    let mut last_error = String::new();
    let mut response_json = None;
    for url in image_generation_urls(&endpoint) {
        let response = match client
            .post(&url)
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                // A timed-out generation can still be running upstream; do not
                // submit the same paid task to another fallback endpoint.
                return Err(anyhow::anyhow!("图片生成请求失败（{url}）: {error}"));
            }
        };
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_lowercase();
        let response_bytes = response
            .bytes()
            .await
            .map_err(|error| anyhow::anyhow!("读取图片生成响应失败（{url}）: {error}"))?;
        let response_text = String::from_utf8_lossy(&response_bytes);
        if status.is_success() && content_type.contains("json") {
            match serde_json::from_slice::<Value>(&response_bytes) {
                Ok(value) => {
                    response_json = Some(value);
                    break;
                }
                Err(error) => {
                    last_error = format!("{url} 返回无效 JSON: {error}");
                }
            }
        } else {
            last_error = format!(
                "{url} HTTP {status}: {}",
                truncate_response_body(&response_text)
            );
        }
    }
    let response_json =
        response_json.ok_or_else(|| anyhow::anyhow!("图片生成接口不可用: {last_error}"))?;
    let outputs = response_json
        .get("data")
        .or_else(|| response_json.get("images"))
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("图片生成响应缺少 data/images"))?;

    let mut assets = Vec::new();
    for output in outputs.iter().take(count) {
        let (bytes, mime_type) = if let Some(encoded) = output
            .get("b64_json")
            .or_else(|| output.get("b64"))
            .and_then(|value| value.as_str())
        {
            (
                BASE64_STANDARD
                    .decode(encoded)
                    .map_err(|error| anyhow::anyhow!("图片 base64 解码失败: {error}"))?,
                "image/png".to_string(),
            )
        } else if let Some(url) = output
            .get("url")
            .or_else(|| output.get("image_url"))
            .and_then(|value| value.as_str())
        {
            download_generated_asset(&client, url, &api_key, "image/png").await?
        } else {
            continue;
        };
        let (bytes, mime_type) =
            normalize_generated_image_aspect(bytes, mime_type, aspect_ratio.as_deref())?;
        assets.push(persist_generated_asset(
            runtime,
            payload,
            &bytes,
            &mime_type,
            &provider,
            &model,
            aspect_ratio.clone(),
            size.clone(),
            quality.clone(),
        )?);
    }
    if assets.is_empty() {
        return Err(anyhow::anyhow!("图片生成没有返回可保存的图片"));
    }
    Ok(assets)
}

fn ark_video_tasks_url(endpoint: &str, task_id: Option<&str>) -> String {
    let normalized = endpoint.trim().trim_end_matches('/');
    let lower = normalized.to_lowercase();
    let base = lower
        .find("/contents/generations/tasks")
        .map(|index| &normalized[..index])
        .unwrap_or(normalized);
    match task_id {
        Some(task_id) => format!("{base}/contents/generations/tasks/{}", task_id.trim()),
        None => format!("{base}/contents/generations/tasks"),
    }
}

fn video_task_id(payload: &Value) -> Option<String> {
    for value in [
        payload.get("id"),
        payload.get("task_id"),
        payload.get("taskId"),
    ] {
        if let Some(value) = value.and_then(|value| value.as_str()) {
            if !value.trim().is_empty() {
                return Some(value.trim().to_string());
            }
        }
    }
    payload.get("data").and_then(video_task_id)
}

fn video_task_status(payload: &Value) -> String {
    for key in ["task_status", "taskStatus", "status"] {
        if let Some(value) = payload.get(key).and_then(|value| value.as_str()) {
            if !value.trim().is_empty() {
                return value.trim().to_uppercase();
            }
        }
    }
    for key in ["data", "output", "result"] {
        if let Some(value) = payload.get(key) {
            let nested = video_task_status(value);
            if !nested.is_empty() {
                return nested;
            }
        }
    }
    String::new()
}

fn video_status_succeeded(status: &str) -> bool {
    ["SUCCEEDED", "SUCCESS", "COMPLETED", "DONE", "FINISHED"]
        .iter()
        .any(|value| status.contains(value))
}

fn video_status_failed(status: &str) -> bool {
    ["FAIL", "ERROR", "CANCEL"]
        .iter()
        .any(|value| status.contains(value))
}

fn collect_video_urls(payload: &Value, urls: &mut Vec<String>) {
    match payload {
        Value::Object(map) => {
            for key in [
                "video_url",
                "videoUrl",
                "file_url",
                "fileUrl",
                "url",
                "video",
            ] {
                if let Some(value) = map.get(key).and_then(|value| value.as_str()) {
                    let value = value.trim();
                    if (value.starts_with("http://")
                        || value.starts_with("https://")
                        || value.starts_with("data:"))
                        && !urls.iter().any(|existing| existing == value)
                    {
                        urls.push(value.to_string());
                    }
                }
            }
            for key in ["data", "output", "result", "content", "videos"] {
                if let Some(value) = map.get(key) {
                    collect_video_urls(value, urls);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_video_urls(item, urls);
            }
        }
        Value::String(value)
            if value.starts_with("http://")
                || value.starts_with("https://")
                || value.starts_with("data:") =>
        {
            if !urls.iter().any(|existing| existing == value) {
                urls.push(value.clone());
            }
        }
        _ => {}
    }
}

async fn checked_json_response(
    response: reqwest::Response,
    operation: &str,
) -> anyhow::Result<Value> {
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "{operation}失败: HTTP {status}: {}",
            truncate_response_body(&text)
        ));
    }
    serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("{operation}响应不是有效 JSON: {error}"))
}

async fn generate_videos_to_media_library(
    runtime: &GenerationRuntime,
    payload: &Value,
) -> anyhow::Result<Vec<Value>> {
    // Keep this check inside the provider boundary as well as at IPC submission entry points.
    ensure_real_generation_enabled(runtime, RealGenerationKind::Video)?;
    let prompt =
        payload_string(payload, "prompt").ok_or_else(|| anyhow::anyhow!("Prompt is required"))?;
    let endpoint = payload_string(payload, "endpoint")
        .or_else(|| non_empty_string(&runtime.config.video.endpoint))
        .ok_or_else(|| anyhow::anyhow!("Video endpoint is missing"))?;
    if !endpoint
        .to_lowercase()
        .contains("ark.cn-beijing.volces.com")
    {
        return Err(anyhow::anyhow!(
            "当前 Rust 视频后端先支持 Ark Plan/Seedance endpoint"
        ));
    }
    let api_key = payload_string(payload, "apiKey")
        .or_else(|| non_empty_string(&runtime.config.video.api_key))
        .or_else(|| non_empty_string(&runtime.config.goose.api_key))
        .ok_or_else(|| anyhow::anyhow!("Video API key is missing"))?;
    let requested_model = payload_string(payload, "model");
    let configured_model = non_empty_string(&runtime.config.video.model)
        .unwrap_or_else(|| "doubao-seedance-1.5-pro".into());
    let model = match requested_model {
        Some(value) if !value.starts_with("wan2.7-") => value,
        _ => configured_model,
    };
    let aspect_ratio = payload_string(payload, "aspectRatio")
        .filter(|value| value == "9:16")
        .unwrap_or_else(|| "16:9".into());
    let resolution = payload_string(payload, "resolution")
        .filter(|value| value == "1080p")
        .unwrap_or_else(|| "720p".into());
    let duration_seconds = payload
        .get("durationSeconds")
        .and_then(|value| value.as_u64())
        .unwrap_or(runtime.config.video.duration_seconds as u64)
        .clamp(5, 12);
    let count = payload
        .get("count")
        .and_then(|value| value.as_u64())
        .unwrap_or(1)
        .clamp(1, 2) as usize;
    let generation_mode =
        payload_string(payload, "generationMode").unwrap_or_else(|| "text-to-video".into());
    let mut references = payload
        .get("referenceImages")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .map(|value| normalize_video_reference(&value))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut provider_generation_mode = generation_mode.clone();
    if generation_mode == "continuation" {
        let first_clip = payload_string(payload, "firstClip")
            .ok_or_else(|| anyhow::anyhow!("视频续写模式需要上一段视频"))?;
        references.clear();
        references.push(extract_video_last_frame_reference(runtime, &first_clip)?);
        provider_generation_mode = "reference-guided".into();
        eprintln!("[视频] 已从上一段提取尾帧，通过 Ark 参考图模式保持镜头连续");
    }
    if provider_generation_mode == "reference-guided" && references.is_empty() {
        return Err(anyhow::anyhow!("参考图视频模式至少需要 1 张参考图"));
    }
    if generation_mode == "first-last-frame" && references.len() < 2 {
        return Err(anyhow::anyhow!("首尾帧视频模式需要 2 张参考图"));
    }

    let size = match (aspect_ratio.as_str(), resolution.as_str()) {
        ("9:16", "1080p") => "1024x1792",
        ("9:16", _) => "720x1280",
        (_, "1080p") => "1792x1024",
        _ => "1280x720",
    };
    let seconds = if duration_seconds <= 6 {
        "4"
    } else if duration_seconds <= 10 {
        "8"
    } else {
        "12"
    };
    let mut content = vec![json!({ "type": "text", "text": prompt })];
    let mut media = Vec::new();
    for reference in references.iter().take(5) {
        content.push(json!({ "type": "image_url", "image_url": { "url": reference } }));
        media.push(json!({ "type": "image", "image_url": reference }));
    }
    if let Some(value) = payload_string(payload, "drivingAudio") {
        media.push(json!({ "type": "audio", "audio_url": value }));
    }
    let generate_audio = payload
        .get("generateAudio")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut body = json!({
        "model": model,
        "content": content,
        "prompt": prompt,
        "ratio": aspect_ratio,
        "size": size,
        "resolution": resolution,
        "duration": duration_seconds,
        "seconds": seconds,
        "n": count,
        "generate_audio": generate_audio,
        "generation_mode": provider_generation_mode,
    });
    if !media.is_empty() {
        body["media"] = Value::Array(media);
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()?;
    let created = checked_json_response(
        client
            .post(ark_video_tasks_url(&endpoint, None))
            .bearer_auth(&api_key)
            .json(&body)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("视频任务创建失败: {error}"))?,
        "视频任务创建",
    )
    .await?;
    let task_id = video_task_id(&created)
        .ok_or_else(|| anyhow::anyhow!("视频任务创建成功但未返回 id/task_id"))?;
    let poll_timeout_seconds = std::env::var("YUNYING_VIDEO_POLL_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(20 * 60)
        .max(60);
    let deadline = std::time::Instant::now() + Duration::from_secs(poll_timeout_seconds);
    let mut final_payload = created;
    let mut final_status = video_task_status(&final_payload);
    while std::time::Instant::now() < deadline && !video_status_succeeded(&final_status) {
        if video_status_failed(&final_status) {
            return Err(anyhow::anyhow!(
                "视频任务失败: status={final_status}, payload={}",
                truncate_response_body(&final_payload.to_string())
            ));
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        final_payload = checked_json_response(
            client
                .get(ark_video_tasks_url(&endpoint, Some(&task_id)))
                .bearer_auth(&api_key)
                .send()
                .await
                .map_err(|error| anyhow::anyhow!("视频任务查询失败: {error}"))?,
            "视频任务查询",
        )
        .await?;
        final_status = video_task_status(&final_payload);
    }
    if !video_status_succeeded(&final_status) {
        return Err(anyhow::anyhow!(
            "视频任务超时: id={task_id}, status={final_status}"
        ));
    }

    let mut urls = Vec::new();
    collect_video_urls(&final_payload, &mut urls);
    if urls.is_empty() {
        return Err(anyhow::anyhow!(
            "视频任务完成但未返回可下载 URL: id={task_id}"
        ));
    }
    let mut assets = Vec::new();
    for url in urls.iter().take(count) {
        let (bytes, mime_type) =
            download_generated_asset(&client, url, &api_key, "video/mp4").await?;
        assets.push(persist_generated_asset(
            runtime,
            payload,
            &bytes,
            &mime_type,
            "ark-plan-video",
            &model,
            Some(aspect_ratio.clone()),
            Some(resolution.clone()),
            Some(format!("{duration_seconds}s")),
        )?);
    }
    Ok(assets)
}

fn save_and_emit_job(runtime: &GenerationRuntime, job: Value) {
    if let Some(id) = job.get("jobId").and_then(|value| value.as_str()) {
        jobs().as_mut().unwrap().insert(id.to_string(), job.clone());
    }
    runtime.emitter.emit("generation:job-updated", job);
}

async fn run_generation_job(
    runtime: GenerationRuntime,
    job_id: String,
    kind: &'static str,
    payload: Value,
) {
    let mut job = jobs()
        .as_ref()
        .and_then(|items| items.get(&job_id).cloned())
        .unwrap_or_else(|| json!({ "jobId": job_id, "kind": kind }));
    job["status"] = json!("running");
    job["updatedAt"] = json!(now_ts());
    save_and_emit_job(&runtime, job.clone());

    let result = if kind == "image" {
        generate_images_to_media_library(&runtime, &payload).await
    } else {
        generate_videos_to_media_library(&runtime, &payload).await
    };
    match result {
        Ok(assets) => {
            job["status"] = json!("completed");
            job["artifacts"] = json!(assets);
            job["completedAt"] = json!(now_ts());
            job["updatedAt"] = job["completedAt"].clone();
            save_and_emit_job(&runtime, job);
            runtime.emitter.emit(
                "renderer:data-changed",
                json!({ "scope": "media", "action": format!("generate-{kind}") }),
            );
        }
        Err(error) => {
            job["status"] = json!("failed");
            job["error"] = json!(error.to_string());
            job["completedAt"] = json!(now_ts());
            job["updatedAt"] = job["completedAt"].clone();
            save_and_emit_job(&runtime, job);
        }
    }
}

pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        "generation:submit-image" => {
            let runtime = generation_runtime(state);
            if let Err(error) = ensure_real_generation_enabled(&runtime, RealGenerationKind::Image)
            {
                return Ok(blocked_generation_response(error));
            }
            let id = new_job_id();
            let job = json!({
                "id": id,
                "jobId": id,
                "kind": "image",
                "source": payload.get("source").cloned().unwrap_or_else(|| json!("desktop")),
                "status": "pending",
                "prompt": payload.get("prompt").cloned().unwrap_or(Value::Null),
                "request": payload,
                "artifacts": [],
                "createdAt": now_ts(),
                "updatedAt": now_ts(),
            });
            jobs().as_mut().unwrap().insert(id.clone(), job.clone());
            state.emitter.emit("generation:job-updated", job.clone());
            let request = job["request"].clone();
            let task_id = id.clone();
            tokio::spawn(async move {
                run_generation_job(runtime, task_id, "image", request).await;
            });
            Ok(json!({ "success": true, "jobId": id, "status": "pending", "job": job }))
        }
        "generation:submit-video" => {
            let runtime = generation_runtime(state);
            if let Err(error) = ensure_real_generation_enabled(&runtime, RealGenerationKind::Video)
            {
                return Ok(blocked_generation_response(error));
            }
            let id = new_job_id();
            let job = json!({
                "id": id,
                "jobId": id,
                "kind": "video",
                "source": payload.get("source").cloned().unwrap_or_else(|| json!("desktop")),
                "status": "pending",
                "prompt": payload.get("prompt").cloned().unwrap_or(Value::Null),
                "request": payload,
                "artifacts": [],
                "createdAt": now_ts(),
                "updatedAt": now_ts(),
            });
            jobs().as_mut().unwrap().insert(id.clone(), job.clone());
            state.emitter.emit("generation:job-updated", job.clone());
            let request = job["request"].clone();
            let task_id = id.clone();
            tokio::spawn(async move {
                run_generation_job(runtime, task_id, "video", request).await;
            });
            Ok(json!({ "success": true, "jobId": id, "status": "pending", "job": job }))
        }
        "image-gen:generate" => {
            let runtime = generation_runtime(state);
            if let Err(error) = ensure_real_generation_enabled(&runtime, RealGenerationKind::Image)
            {
                return Ok(blocked_generation_response(error));
            }
            match generate_images_to_media_library(&runtime, &payload).await {
                Ok(assets) => {
                    state.emitter.emit(
                        "renderer:data-changed",
                        json!({ "scope": "media", "action": "generate-image" }),
                    );
                    Ok(json!({ "success": true, "assets": assets }))
                }
                Err(error) => {
                    Ok(json!({ "success": false, "error": error.to_string(), "assets": [] }))
                }
            }
        }
        "video-gen:generate" => {
            let runtime = generation_runtime(state);
            if let Err(error) = ensure_real_generation_enabled(&runtime, RealGenerationKind::Video)
            {
                return Ok(blocked_generation_response(error));
            }
            match generate_videos_to_media_library(&runtime, &payload).await {
                Ok(assets) => {
                    state.emitter.emit(
                        "renderer:data-changed",
                        json!({ "scope": "media", "action": "generate-video" }),
                    );
                    Ok(json!({ "success": true, "assets": assets }))
                }
                Err(error) => {
                    Ok(json!({ "success": false, "error": error.to_string(), "assets": [] }))
                }
            }
        }
        "generation:list-jobs" | "generation:list-job-summaries" => {
            let g = jobs();
            let all: Vec<Value> = g.as_ref().unwrap().values().cloned().collect();
            Ok(json!({ "success": true, "items": all }))
        }
        "generation:get-job" => {
            let id = payload.get("jobId").and_then(|v| v.as_str()).unwrap_or("");
            Ok(jobs()
                .as_ref()
                .unwrap()
                .get(id)
                .cloned()
                .unwrap_or(Value::Null))
        }
        "generation:get-job-artifacts" => {
            let id = payload.get("jobId").and_then(|v| v.as_str()).unwrap_or("");
            let artifacts = jobs()
                .as_ref()
                .unwrap()
                .get(id)
                .and_then(|job| job.get("artifacts"))
                .cloned()
                .unwrap_or_else(|| json!([]));
            Ok(json!({ "success": true, "artifacts": artifacts }))
        }
        "generation:await-job" => {
            let id = payload.get("jobId").and_then(|v| v.as_str()).unwrap_or("");
            let timeout_ms = payload
                .get("timeoutMs")
                .and_then(|v| v.as_u64())
                .unwrap_or(20 * 60 * 1000)
                .clamp(1_000, 30 * 60 * 1000);
            let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
            loop {
                let job = jobs()
                    .as_ref()
                    .unwrap()
                    .get(id)
                    .cloned()
                    .unwrap_or(Value::Null);
                let status = job
                    .get("status")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                if matches!(status, "completed" | "failed" | "cancelled") {
                    return Ok(json!({ "success": status == "completed", "job": job }));
                }
                if std::time::Instant::now() >= deadline {
                    return Ok(
                        json!({ "success": false, "error": "等待生成任务超时", "job": job }),
                    );
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        "generation:cancel-job" => {
            let id = payload.get("jobId").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(j) = jobs().as_mut().unwrap().get_mut(id) {
                j["status"] = json!("cancelled");
            }
            Ok(json!({ "ok": true }))
        }
        "generation:retry-job" => {
            let id = payload.get("jobId").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(j) = jobs().as_mut().unwrap().get_mut(id) {
                j["status"] = json!("pending");
            }
            Ok(json!({ "ok": true }))
        }
        "generation:get-runtime-status" => {
            let runtime = generation_runtime(state);
            let image_configured = (!runtime.config.image.endpoint.trim().is_empty()
                || !runtime.config.goose.base_url.trim().is_empty())
                && (!runtime.config.image.api_key.trim().is_empty()
                    || !runtime.config.goose.api_key.trim().is_empty());
            let video_configured = !runtime.config.video.endpoint.trim().is_empty()
                && (!runtime.config.video.api_key.trim().is_empty()
                    || !runtime.config.goose.api_key.trim().is_empty());
            let active_job_count = jobs()
                .as_ref()
                .unwrap()
                .values()
                .filter(|job| {
                    matches!(
                        job.get("status").and_then(|value| value.as_str()),
                        Some("pending" | "running")
                    )
                })
                .count();
            Ok(json!({
                "success": true,
                "runtimeReady": image_configured || video_configured,
                "runtimeRunning": active_job_count > 0,
                "imageConfigured": image_configured,
                "videoConfigured": video_configured,
                "realImageEnabled": runtime.config.safety.run_real_image,
                "realVideoEnabled": runtime.config.safety.run_real_video,
                "activeJobCount": active_job_count,
            }))
        }
        // ---- media ----
        "media:list" => {
            let root = media_root(state);
            let root_display = root.to_string_lossy().into_owned();
            let limit = payload
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(300)
                .clamp(1, 5000) as usize;
            match list_media_assets(&root, limit) {
                Ok(assets) => {
                    schedule_media_thumbnail_backfill(root, state.emitter.clone());
                    Ok(json!({
                        "success": true,
                        "assets": assets,
                        "root": root_display,
                    }))
                }
                Err(e) => Ok(json!({
                    "success": false,
                    "error": e.to_string(),
                    "assets": [],
                    "root": root_display,
                })),
            }
        }
        "media:import-files" => {
            let paths: Vec<PathBuf> = payload
                .get("files")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|item| {
                    item.as_str()
                        .or_else(|| item.get("path").and_then(|v| v.as_str()))
                        .map(PathBuf::from)
                })
                .collect();
            if paths.is_empty() {
                return Ok(json!({
                    "success": true,
                    "canceled": true,
                    "imported": [],
                    "added": 0,
                }));
            }
            let root = media_root(state);
            let source_kind = payload
                .get("source")
                .or_else(|| payload.get("sourceKind"))
                .and_then(|value| value.as_str())
                .unwrap_or("imported");
            match import_media_files(&root, &paths, source_kind) {
                Ok(imported) => Ok(json!({
                    "success": true,
                    "canceled": false,
                    "added": imported.len(),
                    "source": source_kind,
                    "imported": imported,
                })),
                Err(e) => Ok(json!({
                    "success": false,
                    "error": e.to_string(),
                    "imported": [],
                })),
            }
        }
        "media:update" => {
            let asset_id = payload
                .get("assetId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if asset_id.is_empty() {
                return Ok(json!({ "success": false, "error": "assetId is required" }));
            }
            let root = media_root(state);
            let mut catalog = read_media_catalog(&root)?;
            let Some(asset) = catalog.assets.iter_mut().find(|asset| asset.id == asset_id) else {
                return Ok(json!({ "success": false, "error": "Media asset not found" }));
            };
            if let Some(value) = payload.get("projectId").and_then(|v| v.as_str()) {
                move_generated_asset_to_project(&root, asset, non_empty_string(value))?;
            }
            if let Some(value) = payload.get("title").and_then(|v| v.as_str()) {
                asset.title = non_empty_string(value);
            }
            if let Some(value) = payload.get("prompt").and_then(|v| v.as_str()) {
                asset.prompt = non_empty_string(value).map(|value| normalize_prompt(&value));
            }
            asset.updated_at = now_iso();
            let enriched = enrich_media_asset(&root, asset);
            write_media_catalog(&root, &catalog)?;
            state.emitter.emit(
                "renderer:data-changed",
                json!({ "scope": "media", "action": "update", "entityId": asset_id }),
            );
            Ok(json!({ "success": true, "asset": enriched }))
        }
        "media:delete" => {
            let asset_id = payload
                .get("assetId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if asset_id.is_empty() {
                return Ok(json!({ "success": false, "error": "assetId is required" }));
            }
            let root = media_root(state);
            let mut catalog = read_media_catalog(&root)?;
            let Some(index) = catalog.assets.iter().position(|asset| asset.id == asset_id) else {
                return Ok(json!({ "success": false, "error": "Media asset not found" }));
            };
            let asset = catalog.assets.remove(index);
            write_media_catalog(&root, &catalog)?;
            if let Some(relative_path) = asset.relative_path.as_deref() {
                let path = root.join(relative_path);
                if let Err(e) = fs::remove_file(&path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        return Ok(json!({ "success": false, "error": e.to_string() }));
                    }
                }
            }
            let thumbnail = root.join(".thumbnails").join(format!("{}.jpg", asset.id));
            if let Err(error) = fs::remove_file(thumbnail) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    return Ok(json!({ "success": false, "error": error.to_string() }));
                }
            }
            state.emitter.emit(
                "renderer:data-changed",
                json!({ "scope": "media", "action": "delete", "entityId": asset_id }),
            );
            Ok(json!({
                "success": true,
                "deleted": { "id": asset.id, "relativePath": asset.relative_path },
            }))
        }
        "media:bind" => {
            let asset_id = payload
                .get("assetId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            let manuscript_path = payload
                .get("manuscriptPath")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if asset_id.is_empty() || manuscript_path.is_empty() {
                return Ok(json!({
                    "success": false,
                    "error": "assetId and manuscriptPath are required",
                }));
            }
            let normalized = normalize_relative_path(manuscript_path)?;
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let paths = crate::workspace::resolve(&settings);
            if !paths.manuscripts.join(&normalized).exists() {
                return Ok(json!({ "success": false, "error": "Manuscript not found" }));
            }
            let root = paths.media;
            let mut catalog = read_media_catalog(&root)?;
            let Some(asset) = catalog.assets.iter_mut().find(|asset| asset.id == asset_id) else {
                return Ok(json!({ "success": false, "error": "Media asset not found" }));
            };
            asset.bound_manuscript_path = Some(normalized);
            asset.updated_at = now_iso();
            let enriched = enrich_media_asset(&root, asset);
            write_media_catalog(&root, &catalog)?;
            state.emitter.emit(
                "renderer:data-changed",
                json!({ "scope": "media", "action": "bind", "entityId": asset_id }),
            );
            Ok(json!({ "success": true, "asset": enriched }))
        }
        "media:open" => {
            let asset_id = payload
                .get("assetId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if asset_id.is_empty() {
                return Ok(json!({ "success": false, "error": "assetId is required" }));
            }
            let root = media_root(state);
            let catalog = read_media_catalog(&root)?;
            let Some(asset) = catalog.assets.iter().find(|asset| asset.id == asset_id) else {
                return Ok(json!({ "success": false, "error": "Media asset not found" }));
            };
            let path = asset
                .relative_path
                .as_deref()
                .map(|relative| root.join(relative))
                .unwrap_or(root);
            match open_path(&path) {
                Ok(()) => Ok(json!({ "success": true })),
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }
        "media:open-root" => {
            let root = media_root(state);
            ensure_media_dirs(&root)?;
            match open_path(&root) {
                Ok(()) => Ok(json!({ "success": true })),
                Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
            }
        }
        // ---- cover ----
        "cover:list" => {
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let root = crate::workspace::resolve(&settings).cover;
            fs::create_dir_all(&root)?;
            let assets = discover_media_files(&root)
                .into_iter()
                .map(|path| {
                    json!({
                        "name": path.file_name().and_then(|v| v.to_str()).unwrap_or(""),
                        "path": path.to_string_lossy(),
                        "absolutePath": path.to_string_lossy(),
                        "previewUrl": path.to_string_lossy(),
                        "exists": true,
                    })
                })
                .collect::<Vec<_>>();
            Ok(json!({ "success": true, "assets": assets }))
        }
        "cover:generate" => {
            let id = new_job_id();
            Ok(json!({ "jobId": id, "status": "planned", "note": "cover:generate v1 dry-run" }))
        }
        other => Err(anyhow::anyhow!("generation 命名空间未实现通道: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::goose_bridge::GooseBridge;
    use crate::ipc::{AppState, NoopEmitter};
    use std::sync::Arc;

    fn test_state(workspace: &Path) -> AppState {
        let db = Db::open_in_memory().unwrap();
        db.settings()
            .save(&json!({
                "workspace_dir": workspace.to_string_lossy(),
                "active_space_id": "default",
            }))
            .unwrap();
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

    fn test_generation_runtime(
        workspace: &Path,
        run_real_image: bool,
        run_real_video: bool,
    ) -> GenerationRuntime {
        let mut config = yunying_config::Config::default();
        config.safety.run_real_image = run_real_image;
        config.safety.run_real_video = run_real_video;
        GenerationRuntime {
            media_root: workspace.join("media"),
            config,
            emitter: Arc::new(NoopEmitter),
        }
    }

    #[tokio::test]
    async fn safety_gate_blocks_image_and_video_before_provider_calls() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = test_generation_runtime(temp.path(), false, false);

        let image_error = generate_images_to_media_library(
            &runtime,
            &json!({ "prompt": "safe acceptance image" }),
        )
        .await
        .unwrap_err()
        .to_string();
        let video_error = generate_videos_to_media_library(
            &runtime,
            &json!({ "prompt": "safe acceptance video" }),
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(image_error.contains("safety.run_real_image=false"));
        assert!(video_error.contains("safety.run_real_video=false"));
        assert!(!runtime.media_root.exists());
    }

    #[test]
    fn blocked_generation_response_is_explicit_and_empty() {
        let response = blocked_generation_response(anyhow::anyhow!("safety disabled"));
        assert_eq!(response["success"], json!(false));
        assert_eq!(response["blocked"], json!(true));
        assert_eq!(response["reason"], json!("safety_disabled"));
        assert_eq!(response["assets"], json!([]));
    }

    #[test]
    fn job_pool_submit_list_cancel() {
        // 用静态池的纯逻辑：直接构造 job 入池。
        let id = "test-job-1";
        jobs().as_mut().unwrap().insert(
            id.into(),
            json!({ "id": id, "kind": "image", "status": "planned" }),
        );
        let g = jobs();
        assert!(g.as_ref().unwrap().contains_key(id));
        drop(g);
        jobs().as_mut().unwrap().remove(id);
    }

    #[test]
    fn new_job_id_unique() {
        let a = new_job_id();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = new_job_id();
        assert_ne!(a, b, "毫秒级 id 应唯一");
    }

    #[test]
    fn openai_image_endpoint_prefers_v1_for_versionless_base() {
        assert_eq!(
            image_generation_urls("https://api.example.com"),
            vec![
                "https://api.example.com/v1/images/generations".to_string(),
                "https://api.example.com/images/generations".to_string(),
            ]
        );
        assert_eq!(
            image_generation_urls("https://api.example.com/v1"),
            vec!["https://api.example.com/v1/images/generations".to_string()]
        );
    }

    #[test]
    fn local_video_reference_is_embedded_as_data_url() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("reference.png");
        fs::write(&path, b"reference-image").unwrap();

        let normalized = normalize_video_reference(path.to_str().unwrap()).unwrap();
        assert!(normalized.starts_with("data:image/png;base64,"));
        let encoded = normalized.split_once(',').unwrap().1;
        assert_eq!(BASE64_STANDARD.decode(encoded).unwrap(), b"reference-image");
    }

    #[test]
    fn remote_video_reference_is_preserved() {
        let reference = "https://example.com/reference.png";
        assert_eq!(normalize_video_reference(reference).unwrap(), reference);
    }

    #[test]
    fn generated_assets_use_project_type_directories() {
        assert_eq!(
            generated_asset_relative_path(
                &json!({ "projectId": "Learning Desk Ad 2026", "deliveryRole": "cover" }),
                "media_image",
                "image/png",
            ),
            "generated/learning-desk-ad-2026/images/media_image.png"
        );
        assert_eq!(
            generated_asset_relative_path(
                &json!({ "projectId": "Learning Desk Ad 2026", "deliveryRole": "final_video" }),
                "media_video",
                "video/mp4",
            ),
            "generated/learning-desk-ad-2026/output/media_video.mp4"
        );
    }

    #[test]
    fn assigning_project_moves_generated_asset_file() {
        let temp = tempfile::tempdir().unwrap();
        let old_relative = "generated/media_test.mp4";
        let old_path = temp.path().join(old_relative);
        fs::create_dir_all(old_path.parent().unwrap()).unwrap();
        fs::write(&old_path, b"video").unwrap();
        let mut asset = MediaAsset {
            id: "media_test".into(),
            source: "generated".into(),
            project_id: None,
            delivery_role: Some("final_video".into()),
            title: Some("video_stitch: 成片".into()),
            prompt: None,
            provider: Some("openmontage".into()),
            provider_template: None,
            model: None,
            aspect_ratio: None,
            size: None,
            requested_size: None,
            quality: None,
            mime_type: Some("video/mp4".into()),
            relative_path: Some(old_relative.into()),
            bound_manuscript_path: None,
            created_at: now_iso(),
            updated_at: now_iso(),
        };

        move_generated_asset_to_project(temp.path(), &mut asset, Some("学习桌广告".into()))
            .unwrap();

        assert_eq!(asset.project_id.as_deref(), Some("学习桌广告"));
        assert_eq!(
            asset.relative_path.as_deref(),
            Some("generated/学习桌广告/output/media_test.mp4")
        );
        assert!(!old_path.exists());
        assert!(temp
            .path()
            .join("generated/学习桌广告/output/media_test.mp4")
            .is_file());
    }

    #[test]
    fn openmontage_intermediate_video_remains_a_clip() {
        let asset = MediaAsset {
            id: "media_clip".into(),
            source: "generated".into(),
            project_id: Some("learning-desk-ad".into()),
            delivery_role: Some("intermediate_clip".into()),
            title: Some("学习桌广告片段1".into()),
            prompt: None,
            provider: Some("openmontage".into()),
            provider_template: None,
            model: None,
            aspect_ratio: None,
            size: None,
            requested_size: None,
            quality: None,
            mime_type: Some("video/mp4".into()),
            relative_path: Some("generated/media_clip.mp4".into()),
            bound_manuscript_path: None,
            created_at: now_iso(),
            updated_at: now_iso(),
        };

        assert_eq!(media_asset_category(&asset), "clips");
    }

    #[tokio::test]
    async fn continuation_rejects_missing_previous_clip_before_provider_call() {
        let temp = tempfile::tempdir().unwrap();
        let state = test_state(temp.path());
        let runtime = generation_runtime(&state);

        let error = extract_video_last_frame_reference(
            &runtime,
            temp.path().join("missing.mp4").to_str().unwrap(),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("上一段视频不存在"));
    }

    #[test]
    fn generated_image_catalog_uses_physical_size_after_aspect_crop() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = test_generation_runtime(temp.path(), true, false);
        let source = image::DynamicImage::new_rgb8(300, 200);
        let mut encoded = std::io::Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, image::ImageOutputFormat::Png)
            .unwrap();
        let (normalized, mime_type) =
            normalize_generated_image_aspect(encoded.into_inner(), "image/png".into(), Some("3:4"))
                .unwrap();
        let normalized = image::load_from_memory(&normalized).unwrap();
        assert_eq!((normalized.width(), normalized.height()), (150, 200));
        assert_eq!(mime_type, "image/png");

        let mut persisted_bytes = std::io::Cursor::new(Vec::new());
        normalized
            .write_to(&mut persisted_bytes, image::ImageOutputFormat::Png)
            .unwrap();
        let asset = persist_generated_asset(
            &runtime,
            &json!({ "prompt": "local fixture", "aspectRatio": "3:4" }),
            &persisted_bytes.into_inner(),
            &mime_type,
            "fixture-provider",
            "fixture-model",
            Some("3:4".into()),
            Some("1024x1536".into()),
            Some("high".into()),
        )
        .unwrap();

        assert_eq!(asset["size"], json!("150x200"));
        assert_eq!(asset["requestedSize"], json!("1024x1536"));
        let physical_path = PathBuf::from(asset["absolutePath"].as_str().unwrap());
        let physical = image::open(&physical_path).unwrap();
        assert_eq!(
            asset["size"],
            json!(format!("{}x{}", physical.width(), physical.height()))
        );

        let catalog = read_media_catalog(&runtime.media_root).unwrap();
        let catalog_asset = catalog.assets.last().unwrap();
        assert_eq!(catalog_asset.size.as_deref(), Some("150x200"));
        assert_eq!(catalog_asset.requested_size.as_deref(), Some("1024x1536"));
    }

    #[test]
    fn legacy_catalog_migrates_requested_image_size_to_physical_size() {
        let temp = tempfile::tempdir().unwrap();
        let media_root = temp.path().join("media");
        let relative_path = "generated/legacy.png";
        let physical_path = media_root.join(relative_path);
        fs::create_dir_all(physical_path.parent().unwrap()).unwrap();
        image::DynamicImage::new_rgb8(150, 200)
            .save(&physical_path)
            .unwrap();
        let timestamp = now_iso();
        let legacy = json!({
            "version": 1,
            "assets": [{
                "id": "legacy-image",
                "source": "generated",
                "aspectRatio": "3:4",
                "size": "1024x1536",
                "mimeType": "image/png",
                "relativePath": relative_path,
                "createdAt": timestamp,
                "updatedAt": timestamp
            }]
        });
        fs::write(
            media_root.join("catalog.json"),
            serde_json::to_vec_pretty(&legacy).unwrap(),
        )
        .unwrap();

        let migrated = read_media_catalog(&media_root).unwrap();
        assert_eq!(migrated.version, MEDIA_CATALOG_VERSION);
        assert_eq!(migrated.assets[0].size.as_deref(), Some("150x200"));
        assert_eq!(
            migrated.assets[0].requested_size.as_deref(),
            Some("1024x1536")
        );

        let persisted: Value =
            serde_json::from_slice(&fs::read(media_root.join("catalog.json")).unwrap()).unwrap();
        assert_eq!(persisted["version"], json!(MEDIA_CATALOG_VERSION));
        assert_eq!(persisted["assets"][0]["size"], json!("150x200"));
        assert_eq!(persisted["assets"][0]["requestedSize"], json!("1024x1536"));
    }

    #[tokio::test]
    async fn media_list_matches_renderer_contract_and_discovers_files() {
        let temp = tempfile::tempdir().unwrap();
        let media_dir = temp.path().join("media/generated");
        fs::create_dir_all(&media_dir).unwrap();
        fs::write(media_dir.join("cover.png"), b"png").unwrap();
        fs::write(temp.path().join("media/catalog.json.backup"), b"backup").unwrap();
        fs::write(media_dir.join("notes.txt"), b"not media").unwrap();
        let state = test_state(temp.path());

        let result = invoke("media:list", json!({ "limit": 500 }), &state)
            .await
            .unwrap();
        assert_eq!(result["success"], json!(true));
        let assets = result["assets"].as_array().unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0]["relativePath"], json!("generated/cover.png"));
        assert_eq!(assets[0]["mimeType"], json!("image/png"));
        assert_eq!(assets[0]["exists"], json!(true));
        assert!(assets[0]["absolutePath"]
            .as_str()
            .unwrap()
            .ends_with("media/generated/cover.png"));
        assert!(temp.path().join("media/catalog.json").is_file());
    }

    #[test]
    fn import_external_media_uses_source_and_date_folders() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("creator-reference.png");
        fs::write(&source, b"reference-image").unwrap();

        let imported =
            import_media_files(&temp.path().join("media"), &[source], "external").unwrap();

        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0]["source"], json!("external"));
        let relative = imported[0]["relativePath"].as_str().unwrap();
        assert!(relative.starts_with("external/"));
        assert!(relative.ends_with("creator-reference.png"));
        assert!(temp.path().join("media").join(relative).is_file());
    }

    #[test]
    fn media_catalog_discovers_html_assets() {
        let temp = tempfile::tempdir().unwrap();
        let html = temp.path().join("generated/page.html");
        fs::create_dir_all(html.parent().unwrap()).unwrap();
        fs::write(&html, "<html><body>preview</body></html>").unwrap();

        let assets = list_media_assets(temp.path(), 20).unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0]["mimeType"], json!("text/html"));
        assert_eq!(assets[0]["relativePath"], json!("generated/page.html"));
    }
}
