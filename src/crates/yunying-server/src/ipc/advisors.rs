//! 数字人顾问命名空间：`advisors`。
//!
//! 1:1 复刻 Beav (`desktop/electron/main.ts`) 的 `advisors:*` IPC 通道行为，落库到 SQLite 的
//! `agent_tasks`（`task_type='advisor'`），把 [`super::dispatch_invoke`] 的请求按全名 match
//! 路由进来。本模块只定义 [`invoke`]；该命名空间没有纯单向（send）通道——`advisors:changed` /
//! `advisors:download-progress` 等事件由写通道内部通过 [`EventEmitter::emit`] 推送。
//!
//! # 存储策略
//!
//! 原 Beav 每个 advisor 是 `<workspace>/advisors/<id>/config.json` + `knowledge/` 目录。本端口
//! 改为 DB 主存：每条 advisor 对应一行 `agent_tasks(task_type='advisor')`，完整对象（`name` /
//! `avatar` / `personality` / `systemPrompt` / `knowledgeLanguage` / `youtubeChannel` / `videos` /
//! `knowledgeFiles` …）序列化进 `metadata_json`（`{kind:"advisor",...}`）。知识文件正文仍落
//! `<workspace>/advisors/<id>/knowledge/`（`advisors:upload-knowledge` / `advisors:delete-knowledge`
//! 用 tokio::fs 操作），但其清单 `knowledgeFiles` 同时回写 `metadata_json`，便于 list/get 直接
//! 返回（对齐 `buildAdvisorDetail` 读取目录的行为）。
//!
//! # 通道分类
//!
//! - DB 读写：`advisors:list` / `advisors:list-templates` / `advisors:get` / `advisors:create` /
//!   `advisors:update` / `advisors:delete` / `advisors:update-youtube-settings` / `advisors:get-videos`。
//! - 文件系统：`advisors:upload-knowledge` / `advisors:delete-knowledge`（tokio::fs）。
//! - 占位（结构完整、返回 stub，依赖外部能力）：`advisors:select-avatar` / `advisors:pick-knowledge-files`
//!   （原生对话框=系统 API）、`advisors:optimize-prompt` / `advisors:optimize-prompt-deep` /
//!   `advisors:generate-persona`（需 LLM）、`advisors:fetch-youtube-info` /
//!   `advisors:download-youtube-subtitles` / `advisors:refresh-videos` / `advisors:download-video` /
//!   `advisors:retry-failed`（需 youtubeScraper / yt-dlp 子进程）、`advisors:youtube-runner-status` /
//!   `advisors:youtube-runner-run-now`（后台 runner 子进程）。详见 [`STUB_CHANNELS`]。
//!
//! # 安全默认
//!
//! 所有写操作默认 `dry_run`（返回预览而不落库/落盘）；payload 带 `dryRun:false` 或 `confirm:true`
//! 时才真正执行（见 [`dry_run`]）。

use std::sync::Mutex;

use serde_json::{json, Value};

use super::{AppState, EventEmitter};
use crate::db::Db;

/// advisor 默认 task_type（落 `agent_tasks.task_type`）。
const ADVISOR_TASK_TYPE: &str = "advisor";
/// advisor 默认 status（advisor 无任务生命周期，用占位值）。
const ADVISOR_STATUS: &str = "ready";
/// advisor 默认 runtime_mode。
const ADVISOR_RUNTIME_MODE: &str = "advisor";

/// 内置常量模板（对齐 `advisors:list-templates`，原 Beav 返回 `[]`，这里给出最小常量集）。
/// 用 fn 而非 const：json! 宏非 const-evaluable。
fn advisor_templates() -> Vec<Value> {
    vec![
        json!({ "id": "tpl_writer", "name": "写作顾问", "avatar": "✍️", "personality": "细腻严谨", "systemPrompt": "你是一位资深写作顾问，擅长打磨文案结构与表达。", "knowledgeLanguage": "中文" }),
        json!({ "id": "tpl_marketing", "name": "营销顾问", "avatar": "📈", "personality": "敏锐务实", "systemPrompt": "你是一位营销增长顾问，擅长选题、转化与投放分析。", "knowledgeLanguage": "中文" }),
        json!({ "id": "tpl_tech", "name": "技术顾问", "avatar": "🧑‍💻", "personality": "理性系统", "systemPrompt": "你是一位全栈技术顾问，擅长架构与工程方案。", "knowledgeLanguage": "中文" }),
    ]
}

/// youtube runner 进程内状态占位（真实后台 runner 子进程未接入）。
static RUNNER_STATE: Mutex<Option<Value>> = Mutex::new(None);

/// 列入 stub 的通道（依赖原生对话框 / LLM / youtubeScraper / 后台 runner 等外部能力）。
/// 这些通道结构完整、返回确定性 stub，真实逻辑待外部能力接入；单测以 `#[ignore]` 形式覆盖。
pub const STUB_CHANNELS: &[&str] = &[
    "advisors:select-avatar",
    "advisors:pick-knowledge-files",
    "advisors:optimize-prompt",
    "advisors:optimize-prompt-deep",
    "advisors:generate-persona",
    "advisors:fetch-youtube-info",
    "advisors:youtube-runner-status",
    "advisors:youtube-runner-run-now",
    "advisors:download-youtube-subtitles",
    "advisors:refresh-videos",
    "advisors:download-video",
    "advisors:retry-failed",
];

/// 双向调用分发。按通道全名 match；未知通道返回 [`Err`]。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let db = &state.db;
    let emitter: &dyn EventEmitter = &*state.emitter;
    match channel {
        // ---- 顾问 CRUD（DB） ----
        "advisors:list" => Ok(list_advisors(db)?),
        "advisors:list-templates" => Ok(list_templates()),
        "advisors:get" => Ok(get_advisor(db, &payload)?),
        "advisors:create" => Ok(create_advisor(db, emitter, &payload)?),
        "advisors:update" => Ok(update_advisor(db, emitter, &payload)?),
        "advisors:delete" => Ok(delete_advisor(db, emitter, &payload)?),
        // ---- YouTube 设置/视频清单（DB metadata） ----
        "advisors:update-youtube-settings" => Ok(update_youtube_settings(db, emitter, &payload)?),
        "advisors:get-videos" => Ok(get_videos(db, &payload)?),
        // ---- 知识文件（fs） ----
        "advisors:upload-knowledge" => upload_knowledge(db, emitter, &payload).await,
        "advisors:delete-knowledge" => delete_knowledge(db, emitter, &payload).await,
        // ---- 占位（stub，依赖外部能力） ----
        "advisors:select-avatar" => Ok(select_avatar_stub(&payload)),
        "advisors:pick-knowledge-files" => Ok(pick_knowledge_files_stub(&payload)),
        "advisors:optimize-prompt" => Ok(optimize_prompt_stub(&payload)),
        "advisors:optimize-prompt-deep" => Ok(optimize_prompt_deep_stub(&payload)),
        "advisors:generate-persona" => Ok(generate_persona_stub(&payload)),
        "advisors:fetch-youtube-info" => Ok(fetch_youtube_info_stub(&payload)),
        "advisors:youtube-runner-status" => Ok(youtube_runner_status_stub()),
        "advisors:youtube-runner-run-now" => Ok(youtube_runner_run_now_stub(&payload)),
        "advisors:download-youtube-subtitles" => {
            Ok(download_youtube_subtitles_stub(emitter, &payload))
        }
        "advisors:refresh-videos" => Ok(refresh_videos_stub(&payload)),
        "advisors:download-video" => Ok(download_video_stub(&payload)),
        "advisors:retry-failed" => Ok(retry_failed_stub(emitter, &payload)),
        other => Err(anyhow::anyhow!("advisors 命名空间未实现通道: {other}")),
    }
}

// ============================================================================
// 通用助手
// ============================================================================

/// 当前毫秒时间戳（`std::time::SystemTime`）。
fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn opt_str<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(|v| v.as_str())
}

fn opt_i64(payload: &Value, key: &str) -> Option<i64> {
    payload.get(key).and_then(|v| v.as_i64())
}

fn opt_bool(payload: &Value, key: &str) -> Option<bool> {
    payload.get(key).and_then(|v| v.as_bool())
}

/// 写操作是否 dry_run：默认 dry；payload `dryRun:false` 或 `confirm:true` 时才落库/落盘。
fn dry_run(payload: &Value) -> bool {
    if let Some(d) = opt_bool(payload, "dryRun") {
        if !d {
            return false;
        }
    }
    if opt_bool(payload, "confirm").unwrap_or(false) {
        return false;
    }
    true
}

/// 短随机后缀（原子计数器 XOR 纳秒，足够测试/单机去重）。
fn short_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    format!("{:x}", nanos ^ n.rotate_left(7))
}

/// 解析 `*_json` 文本列；空/非法时回退 `fallback`。
fn parse_json_col(v: Option<&Value>, fallback: &Value) -> Value {
    match v.and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or_else(|_| fallback.clone()),
        _ => fallback.clone(),
    }
}

/// 推 `advisors:changed` 事件（对齐 Beav `win.webContents.send('advisors:changed', ...)`）。
fn emit_advisors_changed(emitter: &dyn EventEmitter, action: &str, advisor_id: &str) {
    emitter.emit(
        "advisors:changed",
        json!({ "action": action, "advisorId": advisor_id }),
    );
    emitter.emit(
        "data:changed",
        json!({ "scope": "advisors", "action": action, "entityId": advisor_id }),
    );
}

/// 读取 settings.workspace_dir（为空时回退当前目录），拼出 advisor 工作目录。
fn advisor_dir(db: &Db, advisor_id: &str) -> std::path::PathBuf {
    let ws = db
        .settings()
        .get()
        .ok()
        .and_then(|s| {
            s.get("workspace_dir")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_default();
    let base = if ws.trim().is_empty() {
        std::path::PathBuf::from(".")
    } else {
        std::path::PathBuf::from(ws)
    };
    base.join("advisors").join(advisor_id)
}

/// 默认 YouTube 频道配置（合并输入 + 默认值，对齐 `getDefaultAdvisorYoutubeChannelConfig`）。
fn default_youtube_channel_config(input: &Value) -> Value {
    /// 取整数；输入为空或非法用默认值 `def`，且强制下限 `lo`。
    fn num(input: &Value, key: &str, def: i64, lo: i64) -> i64 {
        input
            .get(key)
            .and_then(|v| {
                v.as_i64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .map(|n| n.max(lo))
            .unwrap_or(def)
    }
    json!({
        "url": input.get("url").and_then(|v| v.as_str()).unwrap_or(""),
        "channelId": input.get("channelId").and_then(|v| v.as_str()).unwrap_or(""),
        "lastRefreshed": input.get("lastRefreshed").cloned().unwrap_or(Value::Null),
        "backgroundEnabled": opt_bool(input, "backgroundEnabled").unwrap_or(true),
        "refreshIntervalMinutes": num(input, "refreshIntervalMinutes", 180, 15),
        "subtitleDownloadIntervalSeconds": num(input, "subtitleDownloadIntervalSeconds", 8, 3),
        "maxVideosPerRefresh": num(input, "maxVideosPerRefresh", 20, 1),
        "maxDownloadsPerRun": num(input, "maxDownloadsPerRun", 3, 1),
        "lastBackgroundRunAt": input.get("lastBackgroundRunAt").cloned().unwrap_or(Value::Null),
        "lastBackgroundError": input.get("lastBackgroundError").cloned().unwrap_or(Value::Null),
    })
}

// ============================================================================
// advisor 行读写
// ============================================================================

/// 读取 advisor 完整 metadata_json（解析为 Value）；不存在返回 None。
fn read_advisor_meta(db: &Db, id: &str) -> anyhow::Result<Option<Value>> {
    let row = db.query_one_json(
        "SELECT metadata_json FROM agent_tasks WHERE id = ?1 AND task_type = ?2",
        &[json!(id), json!(ADVISOR_TASK_TYPE)],
    )?;
    Ok(row.and_then(|r| {
        r.get("metadata_json")
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
    }))
}

/// 覆盖写 advisor 的 metadata_json，刷新 updated_at。
fn write_advisor_meta(db: &Db, id: &str, meta: &Value) -> anyhow::Result<()> {
    let now = now_ts();
    db.execute_json(
        "UPDATE agent_tasks SET metadata_json = ?1, updated_at = ?2 \
         WHERE id = ?3 AND task_type = ?4",
        &[
            json!(meta.to_string()),
            json!(now),
            json!(id),
            json!(ADVISOR_TASK_TYPE),
        ],
    )?;
    Ok(())
}

/// 全量列出 advisor 行（`metadata_json` 解析后挂回 `_meta`）。
fn select_all_advisors(db: &Db) -> anyhow::Result<Vec<Value>> {
    db.query_all_json(
        "SELECT id, metadata_json, created_at, updated_at FROM agent_tasks \
         WHERE task_type = ?1 ORDER BY updated_at DESC",
        &[json!(ADVISOR_TASK_TYPE)],
    )
}

/// metadata → 列表摘要（对齐 `buildAdvisorSummary`）。
fn hydrate_summary(meta: &Value) -> Value {
    let yc = meta.get("youtubeChannel").filter(|v| !v.is_null());
    json!({
        "id": meta.get("id").cloned().unwrap_or(Value::Null),
        "name": meta.get("name").cloned().unwrap_or(Value::Null),
        "avatar": meta.get("avatar").cloned().unwrap_or_else(|| json!("🧠")),
        "personality": meta.get("personality").cloned().unwrap_or(Value::Null),
        "knowledgeLanguage": meta.get("knowledgeLanguage").cloned().unwrap_or_else(|| json!("")),
        "createdAt": meta.get("createdAt").cloned().unwrap_or(Value::Null),
        "hasYoutubeChannel": yc
            .and_then(|y| y.get("url").and_then(|v| v.as_str()).map(|s| !s.is_empty()))
            .unwrap_or(false),
    })
}

/// metadata → 详情摘要（对齐 `buildAdvisorDetail`，附带 systemPrompt + knowledgeFiles）。
fn hydrate_detail(meta: &Value) -> Value {
    let mut out = hydrate_summary(meta);
    if let Some(obj) = out.as_object_mut() {
        obj.insert(
            "systemPrompt".into(),
            meta.get("systemPrompt")
                .cloned()
                .unwrap_or_else(|| json!("")),
        );
        obj.insert(
            "knowledgeFiles".into(),
            meta.get("knowledgeFiles")
                .cloned()
                .and_then(|v| if v.is_array() { Some(v) } else { None })
                .unwrap_or_else(|| json!([])),
        );
        obj.insert(
            "youtubeChannel".into(),
            meta.get("youtubeChannel").cloned().unwrap_or(Value::Null),
        );
        obj.insert(
            "videos".into(),
            meta.get("videos").cloned().unwrap_or_else(|| json!([])),
        );
    }
    out
}

// ============================================================================
// 通道实现：DB CRUD
// ============================================================================

/// `advisors:list`：按 task_type 查全部，返回摘要数组（对齐 `advisors:list`，按 createdAt 降序）。
fn list_advisors(db: &Db) -> anyhow::Result<Value> {
    let rows = select_all_advisors(db)?;
    let mut summaries: Vec<Value> = rows
        .iter()
        .filter_map(|r| {
            parse_json_col(r.get("metadata_json"), &Value::Null)
                .as_object()
                .map(|_| ())
                .and_then(|_| {
                    let meta = parse_json_col(r.get("metadata_json"), &Value::Null);
                    if meta.is_null() {
                        None
                    } else {
                        Some(hydrate_summary(&meta))
                    }
                })
        })
        .collect();
    // 按 createdAt 降序（数值或字符串均可比较；缺失排末尾）。
    summaries.sort_by(|a, b| {
        let av = a
            .get("createdAt")
            .and_then(|v| v.as_i64().or_else(|| v.as_str().map(|_| 0i64)));
        let bv = b
            .get("createdAt")
            .and_then(|v| v.as_i64().or_else(|| v.as_str().map(|_| 0i64)));
        bv.cmp(&av)
    });
    Ok(json!(summaries))
}

/// `advisors:list-templates`：返回内置常量模板。
fn list_templates() -> Value {
    json!(advisor_templates())
}

/// `advisors:get`：返回 `{success, advisor}`（详情摘要）。
fn get_advisor(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "advisorId").or_else(|| opt_str(payload, "id")) {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(json!({ "success": false, "error": "advisorId is required" }));
        }
    };
    match read_advisor_meta(db, id)? {
        Some(meta) => Ok(json!({ "success": true, "advisor": hydrate_detail(&meta) })),
        None => Ok(json!({ "success": false, "error": "advisor not found" })),
    }
}

/// `advisors:create`：INSERT agent_tasks(task_type='advisor') + emit `advisors:changed`。
fn create_advisor(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let dry = dry_run(payload);
    let now = now_ts();
    let id = format!("advisor_{now}_{}", short_suffix());
    let name = opt_str(payload, "name").unwrap_or("").to_string();
    let avatar = opt_str(payload, "avatar").unwrap_or("").to_string();
    let personality = opt_str(payload, "personality").unwrap_or("").to_string();
    let system_prompt = opt_str(payload, "systemPrompt").unwrap_or("").to_string();
    let knowledge_language = opt_str(payload, "knowledgeLanguage")
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("中文")
        .to_string();

    let mut meta = json!({
        "kind": "advisor",
        "id": id,
        "name": name,
        "avatar": avatar,
        "personality": personality,
        "systemPrompt": system_prompt,
        "knowledgeLanguage": knowledge_language,
        "createdAt": now,
        "knowledgeFiles": [],
    });

    // youtubeChannel 可选
    if let Some(yc) = payload.get("youtubeChannel").filter(|v| !v.is_null()) {
        meta["youtubeChannel"] = default_youtube_channel_config(yc);
        meta["videos"] = json!([]);
    }

    if dry {
        return Ok(json!({ "success": true, "id": id, "advisor": meta, "dryRun": true }));
    }

    db.execute_json(
        "INSERT INTO agent_tasks \
         (id, task_type, status, runtime_mode, metadata_json, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        &[
            json!(id),
            json!(ADVISOR_TASK_TYPE),
            json!(ADVISOR_STATUS),
            json!(ADVISOR_RUNTIME_MODE),
            json!(meta.to_string()),
            json!(now),
            json!(now),
        ],
    )?;
    emit_advisors_changed(emitter, "create", &id);
    Ok(json!({ "success": true, "id": id }))
}

/// `advisors:update`：合并更新 metadata_json（name/avatar/personality/systemPrompt/knowledgeLanguage）+ emit。
fn update_advisor(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "id").or_else(|| opt_str(payload, "advisorId")) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return Ok(json!({ "success": false, "error": "advisorId is required" }));
        }
    };

    let mut meta = match read_advisor_meta(db, &id)? {
        Some(m) => m,
        None => {
            return Ok(json!({ "success": false, "error": "advisor not found" }));
        }
    };
    let obj = meta
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("advisor metadata 损坏"))?;

    if let Some(v) = opt_str(payload, "name") {
        obj.insert("name".into(), json!(v));
    }
    // 头像：若为托管本地资源协议则保留原值（对齐 update handler 的还原逻辑）。
    if let Some(v) = opt_str(payload, "avatar") {
        let is_managed = v.starts_with("local-file://") || v.starts_with("redbox-asset://");
        if !is_managed {
            obj.insert("avatar".into(), json!(v));
        }
    }
    if let Some(v) = opt_str(payload, "personality") {
        obj.insert("personality".into(), json!(v));
    }
    if let Some(v) = opt_str(payload, "systemPrompt") {
        obj.insert("systemPrompt".into(), json!(v));
    }
    if let Some(v) = opt_str(payload, "knowledgeLanguage") {
        let lang = if v.trim().is_empty() {
            obj.get("knowledgeLanguage")
                .and_then(|x| x.as_str())
                .unwrap_or("中文")
                .to_string()
        } else {
            v.to_string()
        };
        obj.insert("knowledgeLanguage".into(), json!(lang));
    }

    if dry_run(payload) {
        return Ok(json!({ "success": true, "advisor": meta, "dryRun": true }));
    }

    write_advisor_meta(db, &id, &meta)?;
    emit_advisors_changed(emitter, "update", &id);
    Ok(json!({ "success": true }))
}

/// `advisors:delete`：DELETE agent_tasks(task_type='advisor') + emit。
fn delete_advisor(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "advisorId").or_else(|| opt_str(payload, "id")) {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(json!({ "success": false, "error": "advisorId is required" }));
        }
    };

    if dry_run(payload) {
        return Ok(json!({ "success": true, "advisorId": id, "dryRun": true }));
    }

    let affected = db.execute_json(
        "DELETE FROM agent_tasks WHERE id = ?1 AND task_type = ?2",
        &[json!(id), json!(ADVISOR_TASK_TYPE)],
    )?;
    emit_advisors_changed(emitter, "delete", id);
    Ok(json!({ "success": affected > 0 }))
}

/// `advisors:update-youtube-settings`：合并更新 metadata.youtubeChannel + emit。
fn update_youtube_settings(
    db: &Db,
    emitter: &dyn EventEmitter,
    payload: &Value,
) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "advisorId") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return Ok(json!({ "success": false, "error": "advisorId is required" }));
        }
    };

    let mut meta = match read_advisor_meta(db, &id)? {
        Some(m) => m,
        None => {
            return Ok(json!({ "success": false, "error": "advisor not found" }));
        }
    };
    let existing = meta.get("youtubeChannel").cloned().unwrap_or(json!({}));
    // payload.settings 为增量；与现有合并后用默认值补全。
    let merged = if let Some(patch) = payload.get("settings") {
        let mut base = existing.clone();
        if let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    } else {
        existing
    };
    let normalized = default_youtube_channel_config(&merged);

    if dry_run(payload) {
        return Ok(json!({
            "success": true,
            "youtubeChannel": normalized,
            "dryRun": true
        }));
    }

    if let Some(obj) = meta.as_object_mut() {
        obj.insert("youtubeChannel".into(), normalized.clone());
    }
    write_advisor_meta(db, &id, &meta)?;
    emit_advisors_changed(emitter, "update", &id);
    Ok(json!({ "success": true, "youtubeChannel": normalized }))
}

/// `advisors:get-videos`：返回 metadata.videos 与 metadata.youtubeChannel。
fn get_videos(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "advisorId") {
        Some(s) if !s.is_empty() => s,
        _ => {
            return Ok(json!({ "success": false, "error": "advisorId is required" }));
        }
    };
    match read_advisor_meta(db, id)? {
        Some(meta) => {
            let videos = meta
                .get("videos")
                .cloned()
                .and_then(|v| if v.is_array() { Some(v) } else { None })
                .unwrap_or_else(|| json!([]));
            let yc = meta.get("youtubeChannel").cloned().unwrap_or(Value::Null);
            Ok(json!({ "success": true, "videos": videos, "youtubeChannel": yc }))
        }
        None => Ok(json!({ "success": false, "error": "advisor not found" })),
    }
}

// ============================================================================
// 通道实现：知识文件（tokio::fs）
// ============================================================================

/// `advisors:upload-knowledge`：复制知识文件到 `<workspace>/advisors/<id>/knowledge/`，
/// 同步更新 metadata.knowledgeFiles，并 emit。
/// 兼容 payload 为字符串（advisorId）或 `{advisorId, filePaths:[...]}`。filePaths 缺省时返回需要选择文件。
async fn upload_knowledge(
    db: &Db,
    emitter: &dyn EventEmitter,
    payload: &Value,
) -> anyhow::Result<Value> {
    let advisor_id = if let Some(s) = payload.as_str() {
        s.to_string()
    } else {
        opt_str(payload, "advisorId")
            .unwrap_or("")
            .trim()
            .to_string()
    };
    if advisor_id.is_empty() {
        return Ok(json!({ "success": false, "error": "advisorId is required" }));
    }
    let file_paths: Vec<String> = payload
        .get("filePaths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    if file_paths.is_empty() {
        // 原 Beav 在此弹原生对话框选择文件；对话框不可用，故返回需要选择文件。
        return Ok(
            json!({ "success": false, "error": "filePaths is required (dialog unavailable)" }),
        );
    }

    let knowledge_dir = advisor_dir(db, &advisor_id).join("knowledge");

    if dry_run(payload) {
        return Ok(json!({
            "success": true,
            "count": file_paths.len(),
            "knowledgeDir": knowledge_dir.to_string_lossy(),
            "dryRun": true
        }));
    }

    // 确保 advisor 存在
    let mut meta = match read_advisor_meta(db, &advisor_id)? {
        Some(m) => m,
        None => {
            return Ok(json!({ "success": false, "error": "advisor not found" }));
        }
    };

    tokio::fs::create_dir_all(&knowledge_dir).await?;
    let mut files_meta = meta
        .get("knowledgeFiles")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut count = 0usize;
    for fp in &file_paths {
        let src = std::path::Path::new(fp);
        let file_name = src
            .file_name()
            .and_then(|n| n.to_str())
            .map(String::from)
            .unwrap_or_else(|| format!("file_{}", count));
        // 只接受文本类（对齐对话框过滤器）
        let is_text = file_name.ends_with(".txt")
            || file_name.ends_with(".md")
            || file_name.ends_with(".markdown");
        if !is_text {
            continue;
        }
        let dest = knowledge_dir.join(&file_name);
        if tokio::fs::copy(src, &dest).await.is_ok() {
            count += 1;
            // 去重加入清单
            let entry = json!({ "name": file_name, "path": dest.to_string_lossy() });
            if !files_meta
                .iter()
                .any(|v| v.get("name").and_then(|n| n.as_str()) == Some(file_name.as_str()))
            {
                files_meta.push(entry);
            }
        }
    }

    if let Some(obj) = meta.as_object_mut() {
        obj.insert("knowledgeFiles".into(), Value::Array(files_meta));
    }
    write_advisor_meta(db, &advisor_id, &meta)?;
    emit_advisors_changed(emitter, "update", &advisor_id);
    Ok(json!({ "success": true, "count": count }))
}

/// `advisors:delete-knowledge`：删除 knowledge 目录下指定文件，回写 metadata.knowledgeFiles，并 emit。
async fn delete_knowledge(
    db: &Db,
    emitter: &dyn EventEmitter,
    payload: &Value,
) -> anyhow::Result<Value> {
    let advisor_id = match opt_str(payload, "advisorId") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return Ok(json!({ "success": false, "error": "advisorId is required" }));
        }
    };
    let file_name = match opt_str(payload, "fileName") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return Ok(json!({ "success": false, "error": "fileName is required" }));
        }
    };

    let target = advisor_dir(db, &advisor_id)
        .join("knowledge")
        .join(&file_name);

    if dry_run(payload) {
        return Ok(json!({ "success": true, "target": target.to_string_lossy(), "dryRun": true }));
    }

    let mut meta = match read_advisor_meta(db, &advisor_id)? {
        Some(m) => m,
        None => {
            return Ok(json!({ "success": false, "error": "advisor not found" }));
        }
    };

    // 文件不存在也算成功（对齐原 Beav 失败兜底）。
    let _ = tokio::fs::remove_file(&target).await;

    if let Some(arr) = meta
        .get_mut("knowledgeFiles")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| v.get("name").and_then(|n| n.as_str()) != Some(file_name.as_str()));
    }
    write_advisor_meta(db, &advisor_id, &meta)?;
    emit_advisors_changed(emitter, "update", &advisor_id);
    Ok(json!({ "success": true }))
}

// ============================================================================
// 占位通道（stub）—— 依赖原生对话框 / LLM / youtubeScraper / yt-dlp / 后台 runner
// ============================================================================
//
// 以下结构完整：参数解析、配置读取、进度事件、状态机骨架都在；仅因依赖外部能力（系统对话框、
// LLM 推理、yt-dlp 子进程、常驻 runner）而返回确定性 stub。外部能力接入后替换函数体即可。

/// `advisors:select-avatar`（stub）：原生文件对话框不可用。payload 带 `path` 时复制到临时暂存区。
fn select_avatar_stub(payload: &Value) -> Value {
    // 给定明确路径时，复制到运行时暂存区（与原 handler 的 staging 语义对齐）。
    if let Some(src) = opt_str(payload, "path") {
        let staging = std::env::temp_dir().join("avatar-picker");
        let _ = std::fs::create_dir_all(&staging);
        let src_path = std::path::Path::new(src);
        let ext = src_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{e}"))
            .unwrap_or_else(|| ".jpg".into());
        let dest = staging.join(format!("avatar_{}{ext}", now_ts()));
        if std::fs::copy(src_path, &dest).is_ok() {
            return json!(dest.to_string_lossy());
        }
        return json!(src);
    }
    // 无对话框能力：返回 null（与对话框取消等价）。
    Value::Null
}

/// `advisors:pick-knowledge-files`（stub）：原生文件对话框不可用。payload 带 `filePaths` 时透传。
fn pick_knowledge_files_stub(payload: &Value) -> Value {
    if let Some(paths) = payload.get("filePaths").and_then(|v| v.as_array()) {
        let files: Vec<Value> = paths
            .iter()
            .filter_map(|v| v.as_str())
            .map(|p| {
                json!({ "path": p, "name": std::path::Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or("") })
            })
            .collect();
        return json!({
            "success": true,
            "canceled": false,
            "filePaths": paths,
            "files": files
        });
    }
    // 对话框不可用：返回取消。
    json!({ "success": true, "canceled": true, "filePaths": [], "files": [] })
}

/// `advisors:optimize-prompt`（stub，需 LLM）：返回需配置 API 的确定性响应。
/// 真实实现需读 settings 的 api_endpoint/api_key/model_name 并调 OpenAI 兼容接口（见 `#[ignore]` 测试）。
fn optimize_prompt_stub(payload: &Value) -> Value {
    let info = opt_str(payload, "info").unwrap_or("");
    if info.is_empty() {
        return json!({ "success": false, "error": "info is required" });
    }
    json!({
        "success": false,
        "error": "LLM 优化未接入（需配置 api_endpoint/api_key/model_name 后调用 OpenAI 兼容接口）",
        "input": info,
        "prompt": Value::Null
    })
}

/// `advisors:optimize-prompt-deep`（stub，需 LLM + 搜索）：返回需外部能力的确定性响应。
fn optimize_prompt_deep_stub(payload: &Value) -> Value {
    let advisor_id = opt_str(payload, "advisorId").unwrap_or("");
    let name = opt_str(payload, "name").unwrap_or("");
    if name.is_empty() {
        return json!({ "success": false, "error": "name is required" });
    }
    json!({
        "success": false,
        "error": "深度优化未接入（需 web 搜索 + 知识库读取 + LLM 生成）",
        "advisorId": advisor_id,
        "name": name,
        "prompt": Value::Null
    })
}

/// `advisors:generate-persona`（stub，需 LLM）：返回需外部能力的确定性响应。
fn generate_persona_stub(payload: &Value) -> Value {
    let channel_name = opt_str(payload, "channelName").unwrap_or("");
    if channel_name.is_empty() {
        return json!({ "success": false, "error": "channelName is required" });
    }
    json!({
        "success": false,
        "error": "AI persona 生成未接入（需 LLM 调 advisorPersonaGenerator）",
        "channelName": channel_name,
        "prompt": Value::Null,
        "personality": Value::Null,
        "searchResults": [],
        "research": Value::Null
    })
}

/// `advisors:fetch-youtube-info`（stub，需 youtubeScraper）：返回需 yt-dlp 的确定性响应。
fn fetch_youtube_info_stub(payload: &Value) -> Value {
    let url = opt_str(payload, "channelUrl").unwrap_or("");
    if url.is_empty() {
        return json!({ "success": false, "error": "channelUrl is required" });
    }
    json!({
        "success": false,
        "error": "yt-dlp/scraper 未接入（需 youtubeScraper.fetchChannelInfo 子进程）",
        "channelUrl": url,
        "data": Value::Null
    })
}

/// youtube runner 当前状态（内存占位，对齐 `AdvisorYoutubeRunnerStatus`）。
fn runner_default_status() -> Value {
    json!({
        "enabled": false,
        "isTicking": false,
        "tickIntervalMinutes": 0,
        "lastTickAt": Value::Null,
        "nextTickAt": Value::Null,
        "lastError": Value::Null
    })
}

/// `advisors:youtube-runner-status`（stub，需后台 runner 进程）：返回内存 runner 状态。
fn youtube_runner_status_stub() -> Value {
    let guard = RUNNER_STATE.lock().ok();
    let status = guard
        .and_then(|g| g.clone())
        .unwrap_or_else(runner_default_status);
    json!({ "success": true, "status": status })
}

/// `advisors:youtube-runner-run-now`（stub，需后台 runner 进程）：写入 runner 状态并返回未接入响应。
fn youtube_runner_run_now_stub(payload: &Value) -> Value {
    // 标记一次 run 尝试（payload.advisorId 可选）。
    let advisor_id = opt_str(payload, "advisorId").unwrap_or("");
    if let Ok(mut g) = RUNNER_STATE.lock() {
        let mut status = g.clone().unwrap_or_else(runner_default_status);
        if let Some(obj) = status.as_object_mut() {
            obj.insert("isTicking".into(), json!(true));
            obj.insert("lastTickAt".into(), json!(now_ts()));
        }
        *g = Some(status);
    }
    json!({
        "success": false,
        "processed": 0,
        "error": "youtube runner 未接入（需常驻 runner 子进程，按 advisorId 同步频道）",
        "advisorId": advisor_id
    })
}

/// 推 youtube 下载进度事件（对齐 `advisors:download-progress`）。
fn emit_download_progress(emitter: &dyn EventEmitter, advisor_id: &str, progress: &str) {
    emitter.emit(
        "advisors:download-progress",
        json!({ "advisorId": advisor_id, "progress": progress }),
    );
}

/// `advisors:download-youtube-subtitles`（stub，需 youtubeScraper + yt-dlp 子进程）：
/// 发送进度事件骨架，返回未接入响应。
fn download_youtube_subtitles_stub(emitter: &dyn EventEmitter, payload: &Value) -> Value {
    let channel_url = opt_str(payload, "channelUrl").unwrap_or("");
    let advisor_id = opt_str(payload, "advisorId").unwrap_or("");
    let video_count = opt_i64(payload, "videoCount").unwrap_or(0);

    if channel_url.is_empty() || advisor_id.is_empty() {
        return json!({
            "success": false,
            "error": "channelUrl and advisorId are required"
        });
    }

    emit_download_progress(emitter, advisor_id, "正在获取视频列表...");
    emit_download_progress(
        emitter,
        advisor_id,
        &format!("找到 {video_count} 个视频，开始下载字幕..."),
    );
    emit_download_progress(emitter, advisor_id, "下载失败: yt-dlp/scraper 未接入");

    json!({
        "success": false,
        "error": "yt-dlp/scraper 未接入（需 youtubeScraper.fetchVideoList + queueSubtitleDownload 子进程）",
        "successCount": 0,
        "failCount": 0
    })
}

/// `advisors:refresh-videos`（stub，需 youtubeScraper）：返回需 scraper 的确定性响应。
fn refresh_videos_stub(payload: &Value) -> Value {
    let advisor_id = opt_str(payload, "advisorId").unwrap_or("");
    let limit = opt_i64(payload, "limit").unwrap_or(50);
    if advisor_id.is_empty() {
        return json!({ "success": false, "error": "advisorId is required" });
    }
    json!({
        "success": false,
        "error": "yt-dlp/scraper 未接入（需 youtubeScraper.fetchVideoList）",
        "advisorId": advisor_id,
        "limit": limit,
        "videos": []
    })
}

/// `advisors:download-video`（stub，需 youtubeScraper）：返回需 scraper 的确定性响应。
fn download_video_stub(payload: &Value) -> Value {
    let advisor_id = opt_str(payload, "advisorId").unwrap_or("");
    let video_id = opt_str(payload, "videoId").unwrap_or("");
    if advisor_id.is_empty() || video_id.is_empty() {
        return json!({ "success": false, "error": "advisorId and videoId are required" });
    }
    json!({
        "success": false,
        "error": "yt-dlp/scraper 未接入（需 queueSubtitleDownload 子进程）",
        "advisorId": advisor_id,
        "videoId": video_id,
        "subtitleFile": Value::Null
    })
}

/// `advisors:retry-failed`（stub，需 youtubeScraper）：发送进度事件骨架，返回未接入响应。
fn retry_failed_stub(emitter: &dyn EventEmitter, payload: &Value) -> Value {
    let advisor_id = match opt_str(payload, "advisorId") {
        Some(s) if !s.is_empty() => s,
        _ => {
            return json!({ "success": false, "error": "advisorId is required" });
        }
    };
    emitter.emit(
        "advisors:retry-progress",
        json!({ "current": 0, "total": 0, "videoId": Value::Null }),
    );
    json!({
        "success": false,
        "error": "yt-dlp/scraper 未接入（需 queueSubtitleDownload 子进程）",
        "advisorId": advisor_id,
        "successCount": 0,
        "failCount": 0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{EventEmitter, NoopEmitter, RecordingEmitter};
    use serde_json::{json, Value};

    fn noop() -> NoopEmitter {
        NoopEmitter
    }

    /// 用临时工作目录打开内存库，并把 workspace_dir 写进去，便于 fs 通道测试。
    fn fresh_db(ws: &std::path::Path) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.settings()
            .save(&json!({ "workspace_dir": ws.to_string_lossy() }))
            .unwrap();
        db
    }

    #[test]
    fn advisors_crud_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let db = fresh_db(tmp.path());
        let em: &dyn EventEmitter = &noop();

        // create（confirm:true 真实落库）
        let created = create_advisor(
            &db,
            em,
            &json!({
                "name": "测试顾问",
                "avatar": "🧠",
                "personality": "稳重",
                "systemPrompt": "你是一位测试顾问。",
                "confirm": true,
            }),
        )
        .unwrap();
        assert_eq!(created["success"], json!(true));
        let id = created["id"].as_str().unwrap().to_string();
        assert!(id.starts_with("advisor_"));

        // create 默认 dry_run 不落库
        let dry = create_advisor(&db, em, &json!({ "name": "dry" })).unwrap();
        assert_eq!(dry["dryRun"], json!(true));
        let all = list_advisors(&db).unwrap();
        assert_eq!(all.as_array().unwrap().len(), 1, "dry_run 不应落库");

        // list 摘要
        let summary = &all.as_array().unwrap()[0];
        assert_eq!(summary["name"], json!("测试顾问"));
        assert_eq!(summary["avatar"], json!("🧠"));
        assert_eq!(summary["hasYoutubeChannel"], json!(false));

        // get 详情
        let got = get_advisor(&db, &json!({ "advisorId": &id })).unwrap();
        assert_eq!(got["success"], json!(true));
        assert_eq!(got["advisor"]["systemPrompt"], json!("你是一位测试顾问。"));
        assert_eq!(
            got["advisor"]["knowledgeFiles"].as_array().unwrap().len(),
            0
        );

        // update（confirm）
        let updated = update_advisor(
            &db,
            em,
            &json!({ "id": &id, "name": "改名后", "confirm": true }),
        )
        .unwrap();
        assert_eq!(updated["success"], json!(true));
        let got2 = get_advisor(&db, &json!({ "advisorId": &id })).unwrap();
        assert_eq!(got2["advisor"]["name"], json!("改名后"));

        // update 默认 dry 不落库
        let _ = update_advisor(&db, em, &json!({ "id": &id, "name": "不应生效" })).unwrap();
        let got3 = get_advisor(&db, &json!({ "advisorId": &id })).unwrap();
        assert_eq!(got3["advisor"]["name"], json!("改名后"));

        // delete（confirm）
        let deleted =
            delete_advisor(&db, em, &json!({ "advisorId": &id, "confirm": true })).unwrap();
        assert_eq!(deleted["success"], json!(true));
        assert!(list_advisors(&db).unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn advisors_emits_changed_on_write() {
        let tmp = tempfile::tempdir().unwrap();
        let db = fresh_db(tmp.path());
        let rec = RecordingEmitter::new();
        let em: &dyn EventEmitter = &rec;

        let created = create_advisor(&db, em, &json!({ "name": "A", "confirm": true })).unwrap();
        let id = created["id"].as_str().unwrap().to_string();

        let events = rec.events.blocking_lock();
        // 至少有 advisors:changed(create) + data:changed 两类
        assert!(events.iter().any(|(c, p)| c == "advisors:changed"
            && p["action"] == "create"
            && p["advisorId"] == id));
    }

    #[tokio::test]
    async fn advisors_knowledge_files_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let db = fresh_db(tmp.path());
        let em: &dyn EventEmitter = &noop();

        // 先建一个 advisor
        let created = create_advisor(&db, em, &json!({ "name": "K", "confirm": true })).unwrap();
        let id = created["id"].as_str().unwrap().to_string();

        // 在临时目录造两个文本文件 + 一个非文本
        let src_dir = tmp.path().join("src");
        tokio::fs::create_dir_all(&src_dir).await.unwrap();
        let a = src_dir.join("a.txt");
        let b = src_dir.join("b.md");
        let c = src_dir.join("c.png");
        tokio::fs::write(&a, "hello").await.unwrap();
        tokio::fs::write(&b, "# md").await.unwrap();
        tokio::fs::write(&c, "binary").await.unwrap();

        // upload（confirm）—— 应只接受 a.txt / b.md
        let up = upload_knowledge(
            &db,
            em,
            &json!({
                "advisorId": &id,
                "filePaths": [a.to_string_lossy(), b.to_string_lossy(), c.to_string_lossy()],
                "confirm": true
            }),
        )
        .await
        .unwrap();
        assert_eq!(up["success"], json!(true));
        assert_eq!(up["count"], json!(2));

        // 文件确实落到 knowledge 目录
        let knowledge_dir = tmp.path().join("advisors").join(&id).join("knowledge");
        assert!(knowledge_dir.join("a.txt").exists());
        assert!(knowledge_dir.join("b.md").exists());
        assert!(!knowledge_dir.join("c.png").exists());

        // knowledgeFiles 清单回写
        let got = get_advisor(&db, &json!({ "advisorId": &id })).unwrap();
        let files = got["advisor"]["knowledgeFiles"].as_array().unwrap();
        assert_eq!(files.len(), 2);

        // upload 默认 dry_run 不落盘
        let up_dry = upload_knowledge(
            &db,
            em,
            &json!({
                "advisorId": &id,
                "filePaths": [a.to_string_lossy()],
            }),
        )
        .await
        .unwrap();
        assert_eq!(up_dry["dryRun"], json!(true));

        // delete-knowledge（confirm）
        let del = delete_knowledge(
            &db,
            em,
            &json!({ "advisorId": &id, "fileName": "a.txt", "confirm": true }),
        )
        .await
        .unwrap();
        assert_eq!(del["success"], json!(true));
        assert!(!knowledge_dir.join("a.txt").exists());
        let got2 = get_advisor(&db, &json!({ "advisorId": &id })).unwrap();
        assert_eq!(
            got2["advisor"]["knowledgeFiles"].as_array().unwrap().len(),
            1
        );
    }

    #[test]
    fn advisors_youtube_settings_and_videos() {
        let tmp = tempfile::tempdir().unwrap();
        let db = fresh_db(tmp.path());
        let em: &dyn EventEmitter = &noop();

        // 带 youtubeChannel 创建
        let created = create_advisor(
            &db,
            em,
            &json!({
                "name": "Y",
                "youtubeChannel": { "url": "https://youtube.com/@x", "channelId": "c1" },
                "confirm": true
            }),
        )
        .unwrap();
        let id = created["id"].as_str().unwrap().to_string();

        // hasYoutubeChannel 应为 true
        let all = list_advisors(&db).unwrap();
        assert_eq!(all[0]["hasYoutubeChannel"], json!(true));

        // get-videos（空列表 + 频道配置带默认值）
        let vids = get_videos(&db, &json!({ "advisorId": &id })).unwrap();
        assert_eq!(vids["success"], json!(true));
        assert_eq!(vids["videos"].as_array().unwrap().len(), 0);
        assert_eq!(
            vids["youtubeChannel"]["url"],
            json!("https://youtube.com/@x")
        );
        assert_eq!(vids["youtubeChannel"]["refreshIntervalMinutes"], json!(180));

        // update-youtube-settings（confirm）：覆盖 refreshIntervalMinutes（低于下限 15 应被钳到 15）
        let upd = update_youtube_settings(
            &db,
            em,
            &json!({
                "advisorId": &id,
                "settings": { "refreshIntervalMinutes": 5, "backgroundEnabled": false },
                "confirm": true
            }),
        )
        .unwrap();
        assert_eq!(upd["success"], json!(true));
        assert_eq!(upd["youtubeChannel"]["refreshIntervalMinutes"], json!(15));
        assert_eq!(upd["youtubeChannel"]["backgroundEnabled"], json!(false));
        // 原 url 保留
        assert_eq!(
            upd["youtubeChannel"]["url"],
            json!("https://youtube.com/@x")
        );
    }

    #[test]
    fn advisors_list_templates_returns_constants() {
        let t = list_templates();
        let arr = t.as_array().unwrap();
        assert!(!arr.is_empty());
        assert!(arr
            .iter()
            .all(|v| v.get("id").is_some() && v.get("systemPrompt").is_some()));
    }

    #[test]
    #[ignore] // GooseBridge::default()→Agent::new() 触发 sqlx pool，非 tokio 测试环境 panic
    fn advisors_unknown_channel_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let db = fresh_db(tmp.path());
        let emitter: std::sync::Arc<dyn EventEmitter> = std::sync::Arc::new(NoopEmitter);
        let state = AppState {
            redclaw_scheduler: crate::ipc::redclaw_runner::RedClawScheduler::inactive(db.clone()),
            db,
            goose: crate::goose_bridge::GooseBridge::default(),
            emitter,
            login: std::sync::Arc::new(crate::login::LoginService::new(std::sync::Arc::new(
                crate::login::StubLoginDriver,
            ))),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let res = rt.block_on(invoke("advisors:nope", json!({}), &state));
        assert!(res.is_err());
    }

    // ---- 依赖外部能力的 stub 通道：结构完整，#[ignore] ----

    #[test]
    fn advisors_optimize_prompt_stub_shape() {
        let out = optimize_prompt_stub(&json!({ "info": "角色描述" }));
        assert_eq!(out["success"], json!(false));
        assert!(out["error"].as_str().unwrap().contains("LLM"));
        assert_eq!(out["input"], json!("角色描述"));
    }

    #[test]
    fn advisors_youtube_runner_status_stub_shape() {
        let out = youtube_runner_status_stub();
        assert_eq!(out["success"], json!(true));
        assert!(out["status"]["enabled"].is_boolean());
    }

    #[test]
    fn advisors_default_youtube_config_clamps() {
        let cfg = default_youtube_channel_config(&json!({
            "refreshIntervalMinutes": 1,
            "subtitleDownloadIntervalSeconds": 1,
            "maxVideosPerRefresh": 0,
            "maxDownloadsPerRun": 0
        }));
        assert_eq!(cfg["refreshIntervalMinutes"], json!(15));
        assert_eq!(cfg["subtitleDownloadIntervalSeconds"], json!(3));
        assert_eq!(cfg["maxVideosPerRefresh"], json!(1));
        assert_eq!(cfg["maxDownloadsPerRun"], json!(1));
    }

    #[test]
    fn advisors_select_avatar_stub_without_path() {
        assert_eq!(select_avatar_stub(&json!({})), Value::Null);
    }

    #[test]
    #[ignore] // 真实优化需 LLM（api_endpoint/api_key/model_name）
    fn advisors_optimize_prompt_needs_ai() {
        let out = optimize_prompt_stub(&json!({ "info": "描述" }));
        assert!(!out["success"].as_bool().unwrap());
    }

    #[test]
    #[ignore] // 真实抓取需 youtubeScraper/yt-dlp 子进程
    fn advisors_fetch_youtube_info_needs_scraper() {
        let out = fetch_youtube_info_stub(&json!({ "channelUrl": "https://youtube.com/@x" }));
        assert!(!out["success"].as_bool().unwrap());
    }

    #[test]
    #[ignore] // 真实下载需 yt-dlp 子进程 + 网络
    fn advisors_download_subtitles_needs_ytdlp() {
        let rec = RecordingEmitter::new();
        let em: &dyn EventEmitter = &rec;
        let out = download_youtube_subtitles_stub(
            em,
            &json!({ "channelUrl": "https://youtube.com/@x", "advisorId": "advisor_x", "videoCount": 3 }),
        );
        assert!(!out["success"].as_bool().unwrap());
        let evs = rec.events.blocking_lock();
        assert!(evs.iter().any(|(c, _)| c == "advisors:download-progress"));
    }
}
