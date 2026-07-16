//! 商媒运营助手 · yunying-ops MCP server（rmcp，stdio）。
//!
//! 启用 `mcp` feature 后编译为 `yunying-ops-mcp` 二进制。Goose 经 `ExtensionConfig::stdio`
//! 拉起它，模型即可发现并自主调用 operations 工具（list_capabilities / start_task /
//! generate_image / upload_video / social_check_account / create_note）。
//!
//! 发布工具默认 **dry-run**；只有 `dry_run=false` 且 `confirm=true` 才调用设置页配置的
//! Social Connection 纯 Rust CDP 运行时。这样既保留计划闸门，也不需要 Python/Node
//! 浏览器自动化服务。
//!
//! 运行：`cargo run -p yunying-ops --bin yunying-ops-mcp --features mcp`（stdio JSON-RPC）。

#![cfg(feature = "mcp")]

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 工具入参（schemars 0.8 JsonSchema，供 rmcp 生成 schema 给模型）
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct StartTaskInput {
    #[schemars(
        description = "平台代号：xhs / douyin / bilibili / weibo / kuaishou / zhihu / tieba"
    )]
    platform: String,
    #[schemars(description = "搜索关键词列表")]
    keywords: Vec<String>,
    #[schemars(default, description = "采集条数上限（小样本，默认 20）")]
    max_notes_count: Option<usize>,
    #[schemars(
        default,
        description = "true=仅计划不执行（默认）；false=真实采集（需 cookie）"
    )]
    dry_run: Option<bool>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct GenerateImageInput {
    #[schemars(description = "生图 prompt")]
    prompt: String,
    #[schemars(default, description = "比例，默认 3:4")]
    aspect_ratio: Option<String>,
    #[schemars(default, description = "数量，默认 1")]
    count: Option<u32>,
    #[schemars(
        default,
        description = "true=仅返回计划（默认）；false=请求真实生成，仍需 safety.run_real_image=true"
    )]
    dry_run: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
struct UploadVideoInput {
    #[schemars(
        description = "平台：douyin / kuaishou / xiaohongshu / bilibili / tencent / youtube"
    )]
    platform: String,
    #[schemars(description = "本地账号 profile 名")]
    account: String,
    #[schemars(description = "视频文件路径")]
    file: String,
    #[schemars(description = "标题")]
    title: Option<String>,
    #[serde(default, alias = "desc")]
    #[schemars(default, description = "正文/简介")]
    description: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "标签列表，不需要带 #")]
    tags: Option<Vec<String>>,
    #[serde(default)]
    #[schemars(default, description = "排期时间，格式 YYYY-MM-DD HH:MM")]
    schedule: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "缩略图/封面路径")]
    thumbnail: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "抖音/视频号横版封面路径")]
    thumbnail_landscape: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "抖音/视频号竖版封面路径")]
    thumbnail_portrait: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "抖音商品链接")]
    product_link: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "抖音商品标题")]
    product_title: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "Bilibili 分区 tid，B站发布必填")]
    tid: Option<u64>,
    #[serde(default)]
    #[schemars(default, description = "视频号短标题")]
    short_title: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "视频号原创内容分类")]
    category: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "视频号保存草稿而非立即发布")]
    draft: Option<bool>,
    #[serde(default)]
    #[schemars(default, description = "YouTube 播放列表名称")]
    playlist: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "YouTube 可见性：public/unlisted/private")]
    visibility: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "浏览器是否无头运行")]
    headless: Option<bool>,
    #[serde(default)]
    #[schemars(default, description = "启用 Rust CDP 运行时调试输出")]
    debug: Option<bool>,
    #[serde(default)]
    #[schemars(default, description = "true=仅计划（默认）")]
    dry_run: Option<bool>,
    #[serde(default)]
    #[schemars(default, description = "必须显式 true 才允许真实发布")]
    confirm: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
struct UploadNoteInput {
    #[schemars(description = "平台：douyin / kuaishou / xiaohongshu")]
    platform: String,
    #[schemars(description = "本地账号 profile 名")]
    account: String,
    #[schemars(description = "图片文件路径列表")]
    images: Vec<String>,
    #[schemars(description = "标题")]
    title: String,
    #[serde(default)]
    #[schemars(default, description = "图文正文")]
    note: Option<String>,
    #[serde(default, alias = "notef")]
    #[schemars(
        default,
        description = "抖音图文正文文件（txt/md）；真实发布时优先于 note"
    )]
    note_file: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "标签列表，不需要带 #")]
    tags: Option<Vec<String>>,
    #[serde(default)]
    #[schemars(default, description = "排期时间，格式 YYYY-MM-DD HH:MM")]
    schedule: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "抖音图文 BGM 名称")]
    bgm: Option<String>,
    #[serde(default)]
    #[schemars(default, description = "浏览器是否无头运行")]
    headless: Option<bool>,
    #[serde(default)]
    #[schemars(default, description = "启用 Rust CDP 运行时调试输出")]
    debug: Option<bool>,
    #[serde(default)]
    #[schemars(default, description = "true=仅计划（默认）")]
    dry_run: Option<bool>,
    #[serde(default)]
    #[schemars(default, description = "必须显式 true 才允许真实发布")]
    confirm: Option<bool>,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct SocialCheckInput {
    #[schemars(description = "平台")]
    platform: String,
    #[schemars(description = "账号 profile")]
    account: String,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct SocialLoginPrepareInput {
    #[schemars(description = "平台")]
    platform: String,
    #[schemars(description = "账号 profile")]
    account: String,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct CreateNoteInput {
    #[schemars(description = "笔记标题")]
    title: String,
    #[schemars(description = "笔记正文（markdown）")]
    body: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedNote {
    id: String,
    title: String,
    body_chars: usize,
    bytes: u64,
    path: PathBuf,
    created_at: String,
    warnings: Vec<String>,
}

const NOTE_CREATE_ATTEMPTS: usize = 16;

fn note_storage_root() -> anyhow::Result<PathBuf> {
    let data_dir = std::env::var_os("YUNYING_DATA_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "YUNYING_DATA_DIR 未配置；create_note 已拒绝假成功，请先配置持久化数据目录"
            )
        })?;
    Ok(PathBuf::from(data_dir).join("operations").join("notes"))
}

fn normalize_note_title(raw: &str) -> anyhow::Result<String> {
    let title = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        anyhow::bail!("笔记标题不能为空");
    }
    Ok(title.chars().take(200).collect())
}

fn safe_note_slug(title: &str) -> String {
    let mut slug = String::new();
    let mut last_was_separator = false;
    let mut character_count = 0;
    for character in title.chars() {
        if character_count >= 48 {
            break;
        }
        let reserved = matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        );
        if character.is_control() || reserved || character.is_whitespace() {
            if !last_was_separator && !slug.is_empty() {
                slug.push('-');
                last_was_separator = true;
                character_count += 1;
            }
            continue;
        }
        slug.push(character);
        last_was_separator = false;
        character_count += 1;
    }
    let slug = slug.trim_matches(['.', '-', ' ']).to_owned();
    if slug.is_empty() {
        "untitled".to_owned()
    } else {
        slug
    }
}

fn note_markdown(id: &str, title: &str, body: &str, created_at: &str) -> String {
    let yaml_title = serde_json::to_string(title).unwrap_or_else(|_| "\"Untitled\"".to_owned());
    format!(
        "---\nid: {id}\ntitle: {yaml_title}\ncreated_at: {created_at}\n---\n\n# {title}\n\n{body}\n"
    )
}

fn persist_note_with_ids<F>(
    storage_root: &Path,
    title: &str,
    body: &str,
    created_at: &str,
    mut next_id: F,
) -> anyhow::Result<PersistedNote>
where
    F: FnMut() -> String,
{
    let title = normalize_note_title(title)?;
    fs::create_dir_all(storage_root).map_err(|error| {
        anyhow::anyhow!(
            "创建笔记目录失败（{}）：{error}",
            storage_root.to_string_lossy()
        )
    })?;
    let storage_root = fs::canonicalize(storage_root).map_err(|error| {
        anyhow::anyhow!(
            "解析笔记目录失败（{}）：{error}",
            storage_root.to_string_lossy()
        )
    })?;
    let slug = safe_note_slug(&title);

    for _ in 0..NOTE_CREATE_ATTEMPTS {
        let id = next_id();
        if id.is_empty() || id.contains(['/', '\\']) {
            anyhow::bail!("内部笔记 ID 非法");
        }
        let final_path = storage_root.join(format!("{id}-{slug}.md"));
        let temporary_path = storage_root.join(format!(".{id}-{}.tmp", Uuid::new_v4().simple()));
        let markdown = note_markdown(&id, &title, body, created_at);

        let mut temporary = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|error| anyhow::anyhow!("创建笔记临时文件失败：{error}"))?;
        let write_result = temporary
            .write_all(markdown.as_bytes())
            .and_then(|_| temporary.sync_all());
        drop(temporary);
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary_path);
            return Err(anyhow::anyhow!("写入笔记失败：{error}"));
        }

        match fs::hard_link(&temporary_path, &final_path) {
            Ok(()) => {
                let mut warnings = Vec::new();
                if let Err(error) = fs::remove_file(&temporary_path) {
                    warnings.push(format!("清理笔记临时文件失败：{error}"));
                }
                #[cfg(unix)]
                if let Err(error) =
                    File::open(&storage_root).and_then(|directory| directory.sync_all())
                {
                    warnings.push(format!("笔记已保存，但同步目录元数据失败：{error}"));
                }
                return Ok(PersistedNote {
                    id,
                    title,
                    body_chars: body.chars().count(),
                    bytes: markdown.len() as u64,
                    path: final_path,
                    created_at: created_at.to_owned(),
                    warnings,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&temporary_path);
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                return Err(anyhow::anyhow!("原子发布笔记失败：{error}"));
            }
        }
    }

    anyhow::bail!("连续 {NOTE_CREATE_ATTEMPTS} 次生成的笔记 ID 均已存在，已拒绝覆盖任何文件")
}

fn persist_note(storage_root: &Path, title: &str, body: &str) -> anyhow::Result<PersistedNote> {
    let created_at = chrono::Utc::now().to_rfc3339();
    persist_note_with_ids(storage_root, title, body, &created_at, || {
        format!("note-{}", Uuid::new_v4().simple())
    })
}

fn create_note_failure(error: impl std::fmt::Display) -> String {
    pretty_json(json!({
        "tool": "create_note",
        "success": false,
        "persisted": false,
        "error": error.to_string(),
    }))
}

// ---------------------------------------------------------------------------
// MCP server
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct OpsServer;

fn planned(tool: &str, payload: Value) -> String {
    serde_json::to_string_pretty(&json!({ "tool": tool, "plannedAction": payload, "dry_run": true, "note": "v1 dry-run：填充 cookies/keys/confirm 后真实执行（见 docs/17）" }))
        .unwrap_or_else(|_| "{}".into())
}

/// 构造采集计划（dry-run 用）。
fn planned_crawl(platform: &str, keywords: &[String], limit: usize) -> Value {
    json!({ "platform": platform, "keywords": keywords, "max_notes_count": limit, "save_option": "json", "enable_comments": true })
}

/// 从 config 取平台的 cookie（key 对齐 yunying-config schema）。
fn cookie_for(cfg: &yunying_config::Config, platform: &str) -> String {
    match platform {
        "xhs" | "xiaohongshu" => cfg.mediacrawler.xhs_cookies.clone(),
        "dy" | "douyin" => cfg.mediacrawler.douyin_cookies.clone(),
        "bili" | "bilibili" => cfg.mediacrawler.bili_cookies.clone(),
        _ => String::new(),
    }
}

/// 构造带 cookie 的 HTTP client。
fn http_with_cookies(cookies: &str) -> anyhow::Result<reqwest::Client> {
    let mut b = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30));
    if let Some(cookie_value) = cookie_header_value(cookies)? {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::COOKIE, cookie_value);
        b = b.default_headers(headers);
    }
    Ok(b.build()?)
}

fn cookie_header_value(cookies: &str) -> anyhow::Result<Option<reqwest::header::HeaderValue>> {
    let cookies = cookies.trim();
    if cookies.is_empty() {
        return Ok(None);
    }
    let value = cookies
        .split_once(':')
        .filter(|(name, _)| name.trim().eq_ignore_ascii_case("cookie"))
        .map(|(_, value)| value.trim())
        .unwrap_or(cookies);
    reqwest::header::HeaderValue::from_str(value)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("Cookie 配置包含非法 HTTP header 字符"))
}

/// 真实采集（纯 HTTP 平台：bilibili/weibo/kuaishou）。返回归一化内容（JSON）。
async fn real_crawl(
    platform: &str,
    keyword: &str,
    limit: usize,
    cookies: &str,
) -> anyhow::Result<Vec<Value>> {
    use mediacrawler::platform::PlatformCrawler;
    let http = http_with_cookies(cookies)?;
    let items: Vec<mediacrawler::model::Content> = match platform {
        "bilibili" | "bili" => {
            mediacrawler::platform::bilibili::BilibiliCrawler::new(http).search(keyword, limit).await?
        }
        "weibo" | "wb" => {
            mediacrawler::platform::weibo::WeiboCrawler::new(http).search(keyword, limit).await?
        }
        "kuaishou" | "ks" => {
            mediacrawler::platform::kuaishou::KuaishouCrawler::new(http).search(keyword, limit).await?
        }
        other => anyhow::bail!(
            "平台 {other} 真实采集需 cookie/JS 引擎/CDP（离线不支持）；可 dry_run 查看计划参数，或先 dry_run 规划再用对应平台爬虫"
        ),
    };
    Ok(items
        .into_iter()
        .filter_map(|c| serde_json::to_value(c).ok())
        .collect())
}

fn image_bridge_timeout() -> std::time::Duration {
    let seconds = std::env::var("YUNYING_IMAGE_REQUEST_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(20 * 60)
        .clamp(60, 2 * 60 * 60);
    std::time::Duration::from_secs(seconds)
}

fn build_image_bridge_request(
    client: &reqwest::Client,
    bridge_url: &str,
    bridge_token: &str,
    body: &Value,
) -> anyhow::Result<reqwest::Request> {
    Ok(client
        .post(bridge_url)
        .bearer_auth(bridge_token)
        .json(body)
        .build()?)
}

/// 真实生图统一走九伴主程序桥，复用桌面端 endpoint、密钥、下载和素材入库逻辑。
async fn real_image_gen(prompt: &str, aspect_ratio: &str, n: u32) -> anyhow::Result<Value> {
    let bridge_url = std::env::var("JIUBAN_APP_BRIDGE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("缺少 JIUBAN_APP_BRIDGE_URL，拒绝执行真实生图"))?;
    let bridge_token = std::env::var("JIUBAN_APP_BRIDGE_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("缺少 JIUBAN_APP_BRIDGE_TOKEN，拒绝执行真实生图"))?;
    let body = json!({
        "action": "app_cli",
        "source": "yunying-ops",
        "payload": {
            "command": "image generate",
            "payload": {
                "prompt": prompt,
                "title": "运营智能体图片生成",
                "count": n.clamp(1, 4),
                "aspectRatio": aspect_ratio,
                "generationMode": "text-to-image"
            }
        }
    });
    let client = reqwest::Client::builder()
        .timeout(image_bridge_timeout())
        .build()?;
    let request = build_image_bridge_request(&client, &bridge_url, &bridge_token, &body)?;
    let resp = client
        .execute(request)
        .await
        .map_err(|error| anyhow::anyhow!("连接九伴图片生成服务失败: {error}"))?;
    let status = resp.status();
    let txt: Value = resp
        .json()
        .await
        .map_err(|error| anyhow::anyhow!("九伴图片生成服务返回无效 JSON: {error}"))?;
    if !status.is_success() {
        anyhow::bail!("九伴图片生成服务 HTTP {status}: {txt}");
    }
    if let Some(error) = txt.get("error") {
        anyhow::bail!("九伴图片生成服务调用失败: {error}");
    }
    let result = txt
        .get("result")
        .ok_or_else(|| anyhow::anyhow!("九伴图片生成服务缺少 result: {txt}"))?;
    if result.get("success").and_then(Value::as_bool) != Some(true) {
        let error = result
            .get("error")
            .cloned()
            .unwrap_or_else(|| json!("图片生成失败"));
        anyhow::bail!("九伴图片生成失败: {error}");
    }
    let data = result.get("data").cloned().unwrap_or_else(|| json!({}));
    if data
        .get("assets")
        .and_then(Value::as_array)
        .map(Vec::is_empty)
        .unwrap_or(true)
    {
        anyhow::bail!("九伴图片生成成功但没有返回素材: {data}");
    }
    Ok(data)
}

fn social_check_timeout() -> std::time::Duration {
    let seconds = std::env::var("YUNYING_SOCIAL_CHECK_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(120)
        .clamp(10, 10 * 60);
    std::time::Duration::from_secs(seconds)
}

fn social_upload_timeout() -> std::time::Duration {
    let seconds = std::env::var("YUNYING_SOCIAL_UPLOAD_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(2 * 60 * 60)
        .clamp(60, 6 * 60 * 60);
    std::time::Duration::from_secs(seconds)
}

fn pretty_json(value: Value) -> String {
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
}

fn social_failure(tool: &str, error: impl ToString) -> String {
    pretty_json(json!({
        "tool": tool,
        "executed": false,
        "success": false,
        "error": error.to_string(),
    }))
}

#[derive(Clone, Copy)]
enum RealOperation {
    Crawl,
    Image,
    Publish,
}

impl RealOperation {
    fn enabled(self, config: &yunying_config::Config) -> bool {
        match self {
            Self::Crawl => config.safety.run_real_crawler,
            Self::Image => config.safety.run_real_image,
            Self::Publish => config.safety.run_real_publish,
        }
    }

    fn config_key(self) -> &'static str {
        match self {
            Self::Crawl => "safety.run_real_crawler",
            Self::Image => "safety.run_real_image",
            Self::Publish => "safety.run_real_publish",
        }
    }
}

fn ensure_real_operation_enabled(
    config: &yunying_config::Config,
    operation: RealOperation,
) -> anyhow::Result<()> {
    if operation.enabled(config) {
        return Ok(());
    }
    anyhow::bail!(
        "安全策略已阻止真实操作：config.json 中 {}=false",
        operation.config_key()
    )
}

fn safety_failure(tool: &str, error: impl ToString) -> String {
    pretty_json(json!({
        "tool": tool,
        "executed": false,
        "success": false,
        "blocked": true,
        "reason": "safety_disabled",
        "error": error.to_string(),
    }))
}

enum NativeSocialUpload {
    Video(socialconnect::cli::VideoUploadOptions),
    Note(socialconnect::cli::NoteUploadOptions),
}

async fn execute_social_upload(
    tool: &str,
    account: &socialconnect::account::Account,
    upload: NativeSocialUpload,
    planned_action: Value,
    dry_run: bool,
    confirm: bool,
) -> String {
    let runtime = socialconnect::native::configured_native_runtime();

    if dry_run || !confirm {
        let planned_result = match &upload {
            NativeSocialUpload::Video(options) => {
                runtime.upload_video(account, options, true, false).await
            }
            NativeSocialUpload::Note(options) => {
                runtime.upload_note(account, options, true, false).await
            }
        };
        return pretty_json(json!({
            "tool": tool,
            "executed": false,
            "success": true,
            "dry_run": true,
            "confirmRequired": true,
            "confirmed": confirm,
            "plannedAction": planned_action,
            "runtimeMode": "rust-cdp",
            "plan": planned_result.ok().map(|result| result.note),
            "note": if dry_run {
                "当前为 dry-run；设置 dry_run=false 且 confirm=true 才会真实发布。"
            } else {
                "真实发布被确认闸门拦截；请显式设置 confirm=true。"
            },
        }));
    }

    let config = yunying_config::load(None).unwrap_or_default();
    if let Err(error) = ensure_real_operation_enabled(&config, RealOperation::Publish) {
        return safety_failure(tool, error);
    }

    if !runtime.browser_available() {
        return social_failure(
            tool,
            "未找到可用的 Chrome/Chromium/Edge；请在设置中指定浏览器可执行文件。",
        );
    }
    let account_info = match runtime.account_info(account) {
        Ok(info) => info,
        Err(error) => return social_failure(tool, format!("读取账号文件失败：{error}")),
    };
    if !account_info.exists || !account_info.valid_json {
        return social_failure(
            tool,
            format!(
                "账号文件不存在或不是有效 JSON：{}；请先在设置 > 工具 > Social Connection 登录或导入账号",
                account_info.cookie_path
            ),
        );
    }

    let checked = match runtime.check_account(account, social_check_timeout()).await {
        Ok(output) => output,
        Err(error) => return social_failure(tool, format!("发布前账号校验失败：{error}")),
    };
    if !checked.success {
        return pretty_json(json!({
            "tool": tool,
            "executed": false,
            "success": false,
            "stage": "check-account",
            "platform": account.platform,
            "account": account.profile,
            "currentUrl": checked.current_url,
            "detail": checked.message,
            "error": "账号在线校验未通过，已阻止真实发布。",
        }));
    }

    let future = async {
        match &upload {
            NativeSocialUpload::Video(options) => {
                runtime.upload_video(account, options, false, true).await
            }
            NativeSocialUpload::Note(options) => {
                runtime.upload_note(account, options, false, true).await
            }
        }
    };
    let output = match tokio::time::timeout(social_upload_timeout(), future).await {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return social_failure(tool, format!("Rust CDP 发布失败：{error}")),
        Err(_) => return social_failure(tool, "Rust CDP 发布执行超时"),
    };
    pretty_json(json!({
        "tool": tool,
        "executed": true,
        "success": true,
        "platform": account.platform,
        "account": account.profile,
        "runtimeMode": "rust-cdp",
        "url": output.url,
        "platformPostId": output.platform_post_id,
        "note": output.note,
        "accountFile": account_info.cookie_path,
        "error": Value::Null,
    }))
}

#[tool_router(server_handler)]
impl OpsServer {
    #[tool(
        name = "list_capabilities",
        description = "列出运营智能体可用的全部 operations 工具"
    )]
    async fn list_capabilities(&self) -> String {
        serde_json::to_string_pretty(&json!({ "tools": yunying_ops::tools::all() }))
            .unwrap_or_default()
    }

    #[tool(
        name = "start_task",
        description = "采集任务：按关键词采集平台内容。默认 dry-run 返回计划；dryRun:false 时真实采集（bilibili/weibo/kuaishou 纯 HTTP，需 cookie 平台见 planned）。"
    )]
    async fn start_task(&self, Parameters(p): Parameters<StartTaskInput>) -> String {
        let dry = p.dry_run.unwrap_or(true);
        let limit = p.max_notes_count.unwrap_or(20);
        let keyword = p.keywords.first().cloned().unwrap_or_default();
        if !dry && !keyword.is_empty() {
            let cfg = yunying_config::load(None).unwrap_or_default();
            if let Err(error) = ensure_real_operation_enabled(&cfg, RealOperation::Crawl) {
                return safety_failure("start_task", error);
            }
            let cookies = cookie_for(&cfg, &p.platform);
            match real_crawl(&p.platform, &keyword, limit, &cookies).await {
                Ok(items) => {
                    return serde_json::to_string_pretty(&json!({
                        "tool": "start_task", "executed": true, "platform": p.platform,
                        "keyword": keyword, "count": items.len(), "items": items,
                    }))
                    .unwrap_or_default();
                }
                Err(e) => {
                    return serde_json::to_string_pretty(&json!({
                        "tool": "start_task", "executed": false, "error": e.to_string(),
                        "plannedAction": planned_crawl(&p.platform, &p.keywords, limit),
                    }))
                    .unwrap_or_default();
                }
            }
        }
        planned("start_task", planned_crawl(&p.platform, &p.keywords, limit))
    }

    #[tool(
        name = "generate_image",
        description = "生成配图并写入九伴素材库（GPT image 2 / Seedream）。默认 dry-run；真实生成需 dry_run=false 且 safety.run_real_image=true。"
    )]
    async fn generate_image(&self, Parameters(p): Parameters<GenerateImageInput>) -> String {
        let dry = p.dry_run.unwrap_or(true);
        let aspect_ratio = p.aspect_ratio.unwrap_or_else(|| "3:4".into());
        let count = p.count.unwrap_or(1).clamp(1, 4);
        if !dry {
            let config = yunying_config::load(None).unwrap_or_default();
            if let Err(error) = ensure_real_operation_enabled(&config, RealOperation::Image) {
                return safety_failure("generate_image", error);
            }
            match real_image_gen(&p.prompt, &aspect_ratio, count).await {
                Ok(data) => {
                    return serde_json::to_string_pretty(&json!({
                        "tool": "generate_image",
                        "executed": true,
                        "assets": data.get("assets").cloned().unwrap_or_else(|| json!([])),
                        "provider": data.get("provider").cloned().unwrap_or(Value::Null),
                        "model": data.get("model").cloned().unwrap_or(Value::Null),
                    }))
                    .unwrap_or_default();
                }
                Err(e) => {
                    return serde_json::to_string_pretty(&json!({
                        "tool": "generate_image", "executed": false, "error": e.to_string(),
                        "plannedAction": json!({
                            "model": "gpt-image-2",
                            "prompt": p.prompt,
                            "aspect_ratio": aspect_ratio,
                            "count": count,
                        }),
                    }))
                    .unwrap_or_default();
                }
            }
        }
        planned(
            "generate_image",
            json!({
                "model": "gpt-image-2",
                "prompt": p.prompt,
                "aspect_ratio": aspect_ratio,
                "count": count,
            }),
        )
    }

    #[tool(
        name = "upload_video",
        description = "发布视频到抖音/快手/小红书/B站/视频号/YouTube。默认 dry-run；只有 dry_run=false 且 confirm=true，并通过真实账号 check 后才执行。"
    )]
    async fn upload_video(&self, Parameters(p): Parameters<UploadVideoInput>) -> String {
        let account = match socialconnect::account::Account::try_new(&p.platform, &p.account) {
            Ok(account) => account,
            Err(error) => return social_failure("upload_video", error),
        };
        let options = socialconnect::cli::VideoUploadOptions {
            file: p.file.clone(),
            title: p.title.clone().unwrap_or_default(),
            description: p.description.clone().unwrap_or_default(),
            tags: p.tags.clone().unwrap_or_default(),
            schedule: p.schedule.clone(),
            thumbnail: p.thumbnail.clone(),
            thumbnail_landscape: p.thumbnail_landscape.clone(),
            thumbnail_portrait: p.thumbnail_portrait.clone(),
            product_link: p.product_link.clone(),
            product_title: p.product_title.clone(),
            tid: p.tid,
            short_title: p.short_title.clone(),
            category: p.category.clone(),
            draft: p.draft.unwrap_or(false),
            playlist: p.playlist.clone(),
            visibility: p.visibility.clone(),
            debug: p.debug.unwrap_or(false),
            headless: p.headless,
        };
        execute_social_upload(
            "upload_video",
            &account,
            NativeSocialUpload::Video(options.clone()),
            json!({
                "platform": account.platform,
                "account": account.profile,
                "file": p.file,
                "title": options.title,
                "description": options.description,
                "tags": options.tags,
                "schedule": options.schedule,
                "thumbnail": options.thumbnail,
                "tid": options.tid,
                "draft": options.draft,
                "playlist": options.playlist,
                "visibility": options.visibility,
            }),
            p.dry_run.unwrap_or(true),
            p.confirm.unwrap_or(false),
        )
        .await
    }

    #[tool(
        name = "upload_note",
        description = "发布图文到抖音/快手/小红书。默认 dry-run；只有 dry_run=false 且 confirm=true，并通过真实账号 check 后才执行。"
    )]
    async fn upload_note(&self, Parameters(p): Parameters<UploadNoteInput>) -> String {
        let account = match socialconnect::account::Account::try_new(&p.platform, &p.account) {
            Ok(account) => account,
            Err(error) => return social_failure("upload_note", error),
        };
        let options = socialconnect::cli::NoteUploadOptions {
            images: p.images.clone(),
            title: p.title.clone(),
            note: p.note.clone().unwrap_or_default(),
            note_file: p.note_file.clone(),
            tags: p.tags.clone().unwrap_or_default(),
            schedule: p.schedule.clone(),
            bgm: p.bgm.clone(),
            debug: p.debug.unwrap_or(false),
            headless: p.headless,
        };
        execute_social_upload(
            "upload_note",
            &account,
            NativeSocialUpload::Note(options.clone()),
            json!({
                "platform": account.platform,
                "account": account.profile,
                "images": options.images,
                "title": options.title,
                "note": options.note,
                "noteFile": options.note_file,
                "tags": options.tags,
                "schedule": options.schedule,
                "bgm": options.bgm,
            }),
            p.dry_run.unwrap_or(true),
            p.confirm.unwrap_or(false),
        )
        .await
    }

    #[tool(
        name = "social_login_prepare",
        description = "检查 Social Connection 登录运行时与账号文件位置，并返回应在九伴设置页发起扫码/浏览器登录的准备信息。"
    )]
    async fn social_login_prepare(
        &self,
        Parameters(p): Parameters<SocialLoginPrepareInput>,
    ) -> String {
        let account = match socialconnect::account::Account::try_new(&p.platform, &p.account) {
            Ok(account) => account,
            Err(error) => return social_failure("social_login_prepare", error),
        };
        let runtime = socialconnect::native::configured_native_runtime();
        let info = match runtime.account_info(&account) {
            Ok(info) => info,
            Err(error) => return social_failure("social_login_prepare", error),
        };
        let capability = socialconnect::native::native_capabilities()
            .iter()
            .find(|item| item.platform == account.platform);
        pretty_json(json!({
            "tool": "social_login_prepare",
            "success": runtime.browser_available(),
            "platform": account.platform,
            "account": account.profile,
            "runtimeMode": "rust-cdp",
            "browserExecutable": runtime.browser_executable().map(|path| path.to_string_lossy().into_owned()),
            "browserAvailable": runtime.browser_available(),
            "accountFile": info.cookie_path,
            "accountExists": info.exists,
            "validJson": info.valid_json,
            "interactiveLogin": capability.map(|item| item.interactive_login).unwrap_or(false),
            "settingsPath": "设置 > 工具 > Social Connection 账号池",
            "message": if account.platform == "youtube" {
                "请在设置页点击“浏览器登录”，并在弹出的 Google/YouTube 页面完成登录。"
            } else {
                "请在设置页点击“扫码登录”；登录完成后账号文件会自动进入账号池。"
            },
        }))
    }

    #[tool(
        name = "social_check_account",
        description = "通过纯 Rust CDP 浏览器在线检查发布账号登录态。"
    )]
    async fn social_check_account(&self, Parameters(p): Parameters<SocialCheckInput>) -> String {
        let account = match socialconnect::account::Account::try_new(&p.platform, &p.account) {
            Ok(account) => account,
            Err(error) => return social_failure("social_check_account", error),
        };
        let runtime = socialconnect::native::configured_native_runtime();
        if !runtime.browser_available() {
            return social_failure(
                "social_check_account",
                "未找到可用的 Chrome/Chromium/Edge；请在设置中指定浏览器可执行文件。",
            );
        }
        let info = match runtime.account_info(&account) {
            Ok(info) => info,
            Err(error) => return social_failure("social_check_account", error),
        };
        if !info.exists || !info.valid_json {
            return pretty_json(json!({
                "tool": "social_check_account",
                "success": false,
                "logged_in": false,
                "platform": account.platform,
                "account": account.profile,
                "accountFile": info.cookie_path,
                "error": "账号文件不存在或不是有效 JSON。",
            }));
        }
        match runtime
            .check_account(&account, social_check_timeout())
            .await
        {
            Ok(output) => pretty_json(json!({
                "tool": "social_check_account",
                "success": output.success,
                "logged_in": output.success,
                "platform": account.platform,
                "account": account.profile,
                "accountFile": info.cookie_path,
                "runtimeMode": "rust-cdp",
                "currentUrl": output.current_url,
                "message": output.message,
            })),
            Err(error) => social_failure("social_check_account", error),
        }
    }

    #[tool(
        name = "create_note",
        description = "把运营笔记（标题+Markdown 正文）原子保存到 YUNYING_DATA_DIR/operations/notes，返回唯一 ID、绝对路径和字节数；保存失败时明确失败，绝不假成功。"
    )]
    async fn create_note(&self, Parameters(p): Parameters<CreateNoteInput>) -> String {
        let storage_root = match note_storage_root() {
            Ok(path) => path,
            Err(error) => return create_note_failure(error),
        };
        let title = p.title;
        let body = p.body;
        match tokio::task::spawn_blocking(move || persist_note(&storage_root, &title, &body)).await
        {
            Ok(Ok(note)) => pretty_json(json!({
                "tool": "create_note",
                "success": true,
                "persisted": true,
                "id": note.id,
                "title": note.title,
                "body_chars": note.body_chars,
                "bytes": note.bytes,
                "path": note.path,
                "createdAt": note.created_at,
                "warnings": note.warnings,
            })),
            Ok(Err(error)) => create_note_failure(error),
            Err(error) => create_note_failure(format!("笔记持久化任务异常终止：{error}")),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let server = OpsServer;
    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let serving = server.serve(transport).await?;
    serving.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};

    #[test]
    fn default_config_blocks_every_real_operation() {
        let config = yunying_config::Config::default();
        for operation in [
            RealOperation::Crawl,
            RealOperation::Image,
            RealOperation::Publish,
        ] {
            let error = ensure_real_operation_enabled(&config, operation)
                .expect_err("default safety policy must block real operations")
                .to_string();
            assert!(error.contains(operation.config_key()));
        }
    }

    #[tokio::test]
    async fn generate_image_defaults_to_dry_run_without_contacting_bridge() {
        let output = OpsServer
            .generate_image(Parameters(GenerateImageInput {
                prompt: "安全验收图片".into(),
                aspect_ratio: None,
                count: None,
                dry_run: None,
            }))
            .await;
        let output: Value = serde_json::from_str(&output).expect("valid planned JSON");
        assert_eq!(output["tool"], json!("generate_image"));
        assert_eq!(output["dry_run"], json!(true));
        assert!(output.get("plannedAction").is_some());
    }

    #[test]
    fn real_image_bridge_request_carries_bearer_token() {
        let client = reqwest::Client::new();
        let request = build_image_bridge_request(
            &client,
            "http://127.0.0.1:32100/mcp/beav",
            "ephemeral-local-token",
            &json!({"action": "app_cli"}),
        )
        .expect("build authenticated request");

        assert_eq!(request.url().port(), Some(32100));
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer ephemeral-local-token")
        );
    }

    #[test]
    fn safety_failure_is_explicit() {
        let output: Value = serde_json::from_str(&safety_failure(
            "start_task",
            "safety.run_real_crawler=false",
        ))
        .expect("valid blocked JSON");
        assert_eq!(output["success"], json!(false));
        assert_eq!(output["executed"], json!(false));
        assert_eq!(output["blocked"], json!(true));
        assert_eq!(output["reason"], json!("safety_disabled"));
    }

    #[test]
    fn cookie_header_uses_only_the_cookie_value() {
        for input in ["sid=abc; theme=dark", "Cookie: sid=abc; theme=dark"] {
            let value = cookie_header_value(input)
                .expect("valid cookie header")
                .expect("non-empty cookie header");
            assert_eq!(value.to_str().unwrap(), "sid=abc; theme=dark");
            assert!(!value.to_str().unwrap().starts_with("Cookie:"));
        }
        assert!(cookie_header_value("  ").unwrap().is_none());
        assert!(cookie_header_value("sid=ok\r\nX-Evil: injected").is_err());
    }

    #[test]
    fn create_note_persists_full_markdown_and_remains_readable() {
        let data_dir = tempfile::tempdir().expect("temporary data directory");
        let notes_root = data_dir.path().join("operations").join("notes");
        let body = "## 本周完成\n\n- 完成 3 篇稿件\n- 未执行任何真实发布";

        let note = persist_note(&notes_root, "周报 / 验收", body).expect("persist note");
        let canonical_notes_root = fs::canonicalize(&notes_root).expect("canonical notes root");
        assert!(note.path.is_absolute());
        assert_eq!(note.path.parent(), Some(canonical_notes_root.as_path()));
        assert_eq!(note.body_chars, body.chars().count());

        // 模拟 MCP 进程重启后从返回路径重新打开；正文必须真实存在，而不是内存句柄。
        let reopened = fs::read_to_string(&note.path).expect("reopen persisted note");
        assert!(reopened.contains("# 周报 / 验收"));
        assert!(reopened.contains(body));
        assert_eq!(note.bytes, reopened.len() as u64);
    }

    #[test]
    fn concurrent_same_timestamp_notes_are_unique_and_never_overwrite() {
        const THREADS: usize = 24;
        let data_dir = tempfile::tempdir().expect("temporary data directory");
        let notes_root = data_dir.path().join("operations").join("notes");
        let barrier = Arc::new(Barrier::new(THREADS));
        let mut workers = Vec::new();

        for index in 0..THREADS {
            let root = notes_root.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                persist_note_with_ids(
                    &root,
                    "同一毫秒并发报告",
                    &format!("worker={index}"),
                    "2026-07-16T12:00:00.000Z",
                    || format!("note-{}", Uuid::new_v4().simple()),
                )
                .expect("persist concurrent note")
            }));
        }

        let notes = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker should finish"))
            .collect::<Vec<_>>();
        let ids = notes
            .iter()
            .map(|note| note.id.clone())
            .collect::<HashSet<_>>();
        let paths = notes
            .iter()
            .map(|note| note.path.clone())
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), THREADS);
        assert_eq!(paths.len(), THREADS);
        assert_eq!(
            fs::read_dir(&notes_root).expect("list notes").count(),
            THREADS
        );
        for (index, note) in notes.iter().enumerate() {
            let markdown = fs::read_to_string(&note.path).expect("read concurrent note");
            assert!(markdown.contains(&format!("worker={index}")));
        }
    }

    #[test]
    fn collision_retries_without_touching_existing_note() {
        let data_dir = tempfile::tempdir().expect("temporary data directory");
        let notes_root = data_dir.path().join("operations").join("notes");
        fs::create_dir_all(&notes_root).expect("create notes root");
        let colliding_id = "note-fixed-collision";
        let colliding_path =
            notes_root.join(format!("{colliding_id}-{}.md", safe_note_slug("碰撞报告")));
        fs::write(&colliding_path, "original-content").expect("seed colliding note");
        let mut attempt = 0;

        let created = persist_note_with_ids(
            &notes_root,
            "碰撞报告",
            "new-content",
            "2026-07-16T12:00:00.000Z",
            || {
                attempt += 1;
                if attempt == 1 {
                    colliding_id.to_owned()
                } else {
                    "note-after-collision".to_owned()
                }
            },
        )
        .expect("retry after collision");

        assert_eq!(
            fs::read_to_string(colliding_path).unwrap(),
            "original-content"
        );
        assert_eq!(created.id, "note-after-collision");
        assert!(fs::read_to_string(created.path)
            .expect("read new note")
            .contains("new-content"));
    }

    #[test]
    fn note_filename_cannot_escape_storage_root() {
        let data_dir = tempfile::tempdir().expect("temporary data directory");
        let notes_root = data_dir.path().join("operations").join("notes");
        let note = persist_note(&notes_root, "../../CON:<报告>?*\\", "safe")
            .expect("persist note with hostile title");
        let canonical_notes_root = fs::canonicalize(&notes_root).expect("canonical notes root");
        assert_eq!(note.path.parent(), Some(canonical_notes_root.as_path()));
        let file_name = note.path.file_name().unwrap().to_string_lossy();
        assert!(!file_name.contains(".."));
        assert!(!file_name.contains(['/', '\\']));
    }

    /// 真实采集 bilibili（无 cookie，纯 HTTP）——必须返回至少一条可用业务结果。
    #[tokio::test]
    #[ignore]
    async fn real_bilibili_crawl_works() {
        let items = real_crawl("bilibili", "猫爬架", 1, "")
            .await
            .expect("Bilibili 真实采集链路应成功");
        assert!(!items.is_empty(), "Bilibili 真实采集不得空跑");
        assert_eq!(items[0]["platform"], "bilibili");
        assert!(items[0]["platform_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("BV")));
    }
}
