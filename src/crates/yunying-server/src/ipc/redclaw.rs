//! RedClaw 后台创作 runner 命名空间（`redclaw:*`）。
//!
//! 1:1 复刻 Beav (`desktop/electron/main.ts` + `core/redclawBackgroundRunner.ts` /
//! `core/redclawStore.ts` / `core/redclawProfileStore.ts`) 的 IPC 通道行为，把
//! [`super::dispatch_invoke`] 的请求按通道全名 match 路由进来。本模块只定义 [`invoke`]；
//! RedClaw 命名空间没有纯单向（send）通道（runner 推送用 `redclaw:runner-status` /
//! `redclaw:runner-log` / `redclaw:runner-message` 事件，由 runner 自身 emit，不走 ipc_send）。
//!
//! # 通道分组
//!
//! - `redclaw:runner-*` → [`super::redclaw_runner::RedClawScheduler`]：SQLite 持久化定义与
//!   执行轨迹、Tokio 到点触发、独立 Goose 会话真实执行，并生成 JSON 回执。
//! - `redclaw:list-projects` / `redclaw:get-project` → fs 扫描 `<workspace>/<space>/redclaw/projects`；
//!   目录不存在时返回空（占位）。
//! - `redclaw:open-project` → `webbrowser::open`（系统 API，测试 `#[ignore]`）。
//! - `redclaw:profile:*` → `agent_tasks`（`task_type='redclaw-profile'`，单行
//!   `id='redclaw-profile'`），`metadata_json` 存 agent/soul/identity/user/creatorProfile
//!   markdown + 版本 + onboardingState。
//! - `redclaw:task-*` → `agent_tasks`（`task_type='redclaw-task'`），状态机
//!   `draft → confirmed → scheduled`（外加 `cancelled`）。
//!
//! # 安全默认
//!
//! profile/task 草稿写操作默认 `dry_run`（返回预览而不落库）；payload 带 `dryRun:false`
//! 或 `confirm:true` 时才真正执行（见 [`dry_run`]）。runner 的新增、暂停和删除操作则按 UI
//! 明确动作直接持久化，执行结果仍以 SQLite 轨迹和 JSON 回执为准。
//!
//! # 时间戳
//!
//! 全部时间戳为 `std::time::SystemTime` 毫秒（i64），与 [`crate::ipc::tasks`] 一致；Beav 原始
//! ISO 字符串仅用于 `runAt`（once 模式）输入解析。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Map, Value};

use super::{AppState, EventEmitter};
use crate::db::Db;

/// 需要真实系统窗口操作、自动化测试不能无副作用调用的通道。
pub const STUB_CHANNELS: &[&str] = &["redclaw:open-project"];

// ===========================================================================
// 入口分发
// ===========================================================================

/// 双向调用分发。按通道全名 match；未知通道返回 [`Err`]。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    let db = &state.db;
    let emitter: &dyn EventEmitter = &*state.emitter;
    match channel {
        // ---- runner（SQLite 持久化 + Tokio 自动调度 + Goose 真实执行）----
        "redclaw:runner-status" => state.redclaw_scheduler.status().await,
        "redclaw:runner-start" => state.redclaw_scheduler.start_runner().await,
        "redclaw:runner-stop" => state.redclaw_scheduler.stop_runner().await,
        "redclaw:runner-run-now" => state.redclaw_scheduler.run_due_now().await,
        "redclaw:runner-set-project" => state.redclaw_scheduler.set_project(&payload).await,
        "redclaw:runner-set-config" => state.redclaw_scheduler.set_config(&payload).await,
        "redclaw:runner-list-scheduled" => state.redclaw_scheduler.list_scheduled().await,
        "redclaw:runner-add-scheduled" => state.redclaw_scheduler.add_scheduled(&payload).await,
        "redclaw:runner-remove-scheduled" => {
            state.redclaw_scheduler.remove_scheduled(&payload).await
        }
        "redclaw:runner-set-scheduled-enabled" => {
            state
                .redclaw_scheduler
                .set_scheduled_enabled(&payload)
                .await
        }
        "redclaw:runner-run-scheduled-now" => {
            state.redclaw_scheduler.run_scheduled_now(&payload).await
        }
        "redclaw:runner-list-long-cycle" => state.redclaw_scheduler.list_long_cycle().await,
        "redclaw:runner-add-long-cycle" => state.redclaw_scheduler.add_long_cycle(&payload).await,
        "redclaw:runner-remove-long-cycle" => {
            state.redclaw_scheduler.remove_long_cycle(&payload).await
        }
        "redclaw:runner-set-long-cycle-enabled" => {
            state
                .redclaw_scheduler
                .set_long_cycle_enabled(&payload)
                .await
        }
        "redclaw:runner-run-long-cycle-now" => {
            state.redclaw_scheduler.run_long_cycle_now(&payload).await
        }
        // ---- projects（fs）----
        "redclaw:list-projects" => Ok(list_projects(db, &payload)?),
        "redclaw:get-project" => Ok(get_project(db, &payload)?),
        "redclaw:open-project" => Ok(open_project(&payload)),
        // ---- profile（DB）----
        "redclaw:profile:get-bundle" => Ok(profile_get_bundle(db)?),
        "redclaw:profile:update-doc" => Ok(profile_update_doc(db, &payload)?),
        "redclaw:profile:onboarding-status" => Ok(profile_onboarding_status(db)?),
        "redclaw:profile:onboarding-turn" => Ok(profile_onboarding_turn(db, &payload)?),
        "redclaw:profile:save-initialization-progress" => {
            Ok(profile_save_initialization_progress(db, &payload)?)
        }
        "redclaw:profile:complete-initialization" => {
            Ok(profile_complete_initialization(db, &payload)?)
        }
        // ---- task（DB）----
        "redclaw:task-preview" => Ok(task_preview(&payload)),
        "redclaw:task-create" => Ok(task_create(db, emitter, &payload)?),
        "redclaw:task-confirm" => Ok(task_confirm(db, emitter, &payload)?),
        "redclaw:task-update" => Ok(task_update(db, emitter, &payload)?),
        "redclaw:task-cancel" => Ok(task_cancel(db, emitter, &payload)?),
        "redclaw:task-list" => Ok(task_list(db, &payload)?),
        "redclaw:task-stats" => Ok(task_stats(db)?),
        other => Err(anyhow::anyhow!("redclaw 命名空间未实现通道: {other}")),
    }
}

// ===========================================================================
// 通用助手
// ===========================================================================

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

fn clamp_i64(v: i64, lo: i64, hi: i64) -> i64 {
    v.max(lo).min(hi)
}

/// `Option<String>` → JSON（None→Null）。
fn opt_value(o: &Option<String>) -> Value {
    match o {
        Some(s) => json!(s),
        None => Value::Null,
    }
}

/// 短随机后缀（原子计数器 XOR 纳秒）。
fn short_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    format!("{:x}", nanos ^ n.rotate_left(7))
}

fn new_task_id(prefix: &str) -> String {
    format!("{prefix}_{}:{}", now_ts(), short_suffix())
}

/// 写操作是否 dry_run：默认 dry；payload `dryRun:false` 或 `confirm:true` 时才落库。
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

/// 把 `*_json` 文本列解析成 [`Value`]；空/非法时回退 `fallback`。
fn parse_json_col(v: Option<&Value>, fallback: &Value) -> Value {
    match v.and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => serde_json::from_str(s).unwrap_or_else(|_| fallback.clone()),
        _ => fallback.clone(),
    }
}

fn emit_data_changed(emitter: &dyn EventEmitter, scope: &str, action: &str, entity_id: &str) {
    emitter.emit(
        "data:changed",
        json!({ "scope": scope, "action": action, "entityId": entity_id }),
    );
}

/// ISO 8601 / epoch 字符串转毫秒；项目列表也用它解析 `updatedAt`。
fn parse_iso_ms(v: &str) -> Option<i64> {
    let value = v.trim();
    if value.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp_millis())
        .or_else(|| {
            value.parse::<i64>().ok().map(|number| {
                if value.len() <= 10 {
                    number.saturating_mul(1000)
                } else {
                    number
                }
            })
        })
}

// ===========================================================================
// projects —— fs 扫描 workspace/redclaw/projects
// ===========================================================================

/// `redclaw:list-projects`：扫 `<root>/projects/*/project.json`，按 updatedAt desc 排序。
fn list_projects(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let limit = clamp_i64(opt_i64(payload, "limit").unwrap_or(20), 1, 200) as usize;
    // 用临时 AppState 只为拿路径解析；这里 db 已是 &Db，直接构造路径解析器。
    let root = redclaw_root_from_db(db);
    Ok(json!(list_projects_in_dir(&root.join("projects"), limit)))
}

/// `redclaw:get-project`：读单个 project.json。
fn get_project(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let project_id = opt_str(payload, "projectId")
        .unwrap_or("")
        .trim()
        .to_string();
    if project_id.is_empty() {
        return Ok(json!({ "success": false, "error": "projectId is required" }));
    }
    let root = redclaw_root_from_db(db);
    match get_project_in_dir(&root.join("projects"), &project_id) {
        Ok(v) => {
            Ok(json!({ "success": true, "project": v["project"], "projectDir": v["projectDir"] }))
        }
        Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
    }
}

/// `redclaw:open-project`：系统文件管理器打开目录（webbrowser）。
fn open_project(payload: &Value) -> Value {
    let dir = opt_str(payload, "projectDir")
        .unwrap_or("")
        .trim()
        .to_string();
    if dir.is_empty() {
        return json!({ "success": false, "error": "projectDir is required" });
    }
    match webbrowser::open(&dir) {
        Ok(()) => json!({ "success": true }),
        Err(e) => json!({ "success": false, "error": e.to_string() }),
    }
}

/// 从 Db settings 解析 redclaw 根（无 AppState 时复用）。
fn redclaw_root_from_db(db: &Db) -> PathBuf {
    let settings = db
        .settings()
        .get()
        .unwrap_or_else(|_| Value::Object(Map::new()));
    let base = settings
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(".");
    let mut root = PathBuf::from(base);
    if let Some(space) = settings.get("active_space_id").and_then(|v| v.as_str()) {
        if !space.is_empty() {
            root = root.join(space);
        }
    }
    root.join("redclaw")
}

/// 纯 fs 扫描（可单测，不依赖 settings）。
fn list_projects_in_dir(projects_dir: &std::path::Path, limit: usize) -> Value {
    let entries = match std::fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(_) => return json!([]), // 目录不存在 → 空列表（占位）
    };
    let mut out: Vec<Value> = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let id = entry.file_name().to_string_lossy().to_string();
        if let Ok(mut proj) = read_project_json(projects_dir, &id) {
            if let Some(m) = proj.as_object_mut() {
                m.insert(
                    "projectDir".into(),
                    json!(projects_dir.join(&id).to_string_lossy().to_string()),
                );
            }
            out.push(proj);
        }
    }
    out.sort_by(|a, b| {
        let at = a
            .get("updatedAt")
            .and_then(|v| v.as_str())
            .and_then(parse_iso_ms)
            .unwrap_or(0);
        let bt = b
            .get("updatedAt")
            .and_then(|v| v.as_str())
            .and_then(parse_iso_ms)
            .unwrap_or(0);
        bt.cmp(&at)
    });
    out.truncate(limit.max(1));
    json!(out)
}

/// 纯 fs 读单个 project（可单测）。
fn get_project_in_dir(projects_dir: &std::path::Path, project_id: &str) -> anyhow::Result<Value> {
    let normalized = normalize_project_id(project_id);
    if normalized.is_empty() {
        anyhow::bail!("projectId is required");
    }
    let project = read_project_json(projects_dir, &normalized)?;
    Ok(json!({
        "project": project,
        "projectDir": projects_dir.join(&normalized).to_string_lossy().to_string(),
    }))
}

fn read_project_json(projects_dir: &std::path::Path, id: &str) -> anyhow::Result<Value> {
    let path = projects_dir.join(id).join("project.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("Project not found: {id} ({}) - {e}", path.display()))?;
    let v: Value = serde_json::from_str(&raw)?;
    Ok(v)
}

/// 对齐 Beav `normalizeProjectId`：取 `rc_...` 片段。
fn normalize_project_id(input: &str) -> String {
    let raw = input.trim();
    if raw.is_empty() {
        return String::new();
    }
    if let Some(idx) = raw.find("rc_") {
        let tail = &raw[idx..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c == '/' || c == '\\')
            .unwrap_or(tail.len());
        return tail[..end].trim().to_string();
    }
    raw.to_string()
}

// ===========================================================================
// profile —— DB（agent_tasks task_type='redclaw-profile'）
// ===========================================================================
//
// 单行 `id='redclaw-profile'`，`metadata_json` 存完整 bundle：
// `{agent, soul, identity, user, creatorProfile, bootstrap, version, onboardingState, updatedAt}`。

const PROFILE_ROW_ID: &str = "redclaw-profile";

/// 5 步 onboarding 问题（key / question / default）。
const ONBOARDING_STEPS: &[(&str, &str, &str)] = &[
    (
        "assistant_style",
        "1/5 先定一下我的协作风格。你希望商媒运营助手在对话里更偏向哪种风格？",
        "高执行 + 强结构 + 直接反馈",
    ),
    (
        "creator_goal",
        "2/5 你的核心创作目标是什么？",
        "主目标：稳定涨粉；次目标：建立可信个人品牌",
    ),
    (
        "target_audience",
        "3/5 你的目标用户是谁？请描述人群画像。",
        "25-35岁的一线和新一线职场人，关注效率、成长和副业机会",
    ),
    (
        "content_lane",
        "4/5 你主要做哪些内容赛道？以及偏好的笔记结构。",
        "AI效率工具 + 职场成长；偏好教程体和复盘体",
    ),
    (
        "tone_and_constraints",
        "5/5 最后确认表达风格和边界。",
        "语气真实克制；避免夸张承诺；每周3-5篇；成功指标看收藏率与私信转化",
    ),
];

fn load_profile_row(db: &Db) -> anyhow::Result<Option<Value>> {
    db.query_one_json(
        "SELECT * FROM agent_tasks WHERE id = ?1 AND task_type = 'redclaw-profile'",
        &[json!(PROFILE_ROW_ID)],
    )
}

/// 读取 profile metadata；不存在则插入默认行。返回 `(metadata, row_exists)`。
fn ensure_profile_meta(db: &Db) -> anyhow::Result<Value> {
    if let Some(row) = load_profile_row(db)? {
        return Ok(parse_json_col(
            row.get("metadata_json"),
            &default_profile_meta(),
        ));
    }
    let meta = default_profile_meta();
    let now = now_ts();
    db.execute_json(
        "INSERT INTO agent_tasks \
         (id, task_type, status, runtime_mode, owner_session_id, intent, role_id, goal, \
          current_node, route_json, graph_json, artifacts_json, checkpoints_json, metadata_json, \
          last_error, created_at, updated_at, started_at, completed_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        &[
            json!(PROFILE_ROW_ID),
            json!("redclaw-profile"),
            json!("active"),
            json!("redclaw"),
            Value::Null,
            json!("redclaw-profile"),
            Value::Null,
            Value::Null,
            Value::Null,
            json!("{}"),
            json!("[]"),
            json!("[]"),
            json!("[]"),
            json!(meta.to_string()),
            Value::Null,
            json!(now),
            json!(now),
            Value::Null,
            Value::Null,
        ],
    )?;
    Ok(meta)
}

fn save_profile_meta(db: &Db, meta: &Value) -> anyhow::Result<()> {
    let now = now_ts();
    db.execute_json(
        "UPDATE agent_tasks SET metadata_json = ?1, updated_at = ?2 WHERE id = ?3 AND task_type = 'redclaw-profile'",
        &[json!(meta.to_string()), json!(now), json!(PROFILE_ROW_ID)],
    )?;
    Ok(())
}

fn default_profile_meta() -> Value {
    json!({
        "agent": default_agent_md(),
        "soul": default_soul_md(),
        "identity": default_identity_md(),
        "user": default_user_md(),
        "creatorProfile": default_creator_profile_md(),
        "bootstrap": default_bootstrap_md(),
        "version": 1,
        "updatedAt": now_ts(),
        "onboardingState": default_onboarding_state(),
    })
}

fn default_onboarding_state() -> Value {
    json!({
        "version": 1,
        "updatedAt": now_ts(),
        "askedFirstQuestion": false,
        "stepIndex": 0,
        "answers": {},
    })
}

fn default_agent_md() -> String {
    "# Agent.md\n\n你是商媒运营助手，多平台内容创作执行 Agent。先执行再解释，优先给可落地动作。"
        .into()
}

fn default_soul_md() -> String {
    "# Soul.md\n\n行动导向，不空谈；对结果负责；务实、直接、尊重用户时间。".into()
}

fn default_identity_md() -> String {
    "# identity.md\n\n- Name: 商媒运营助手\n- Role: 多平台内容创作自动化 Agent\n- Vibe: 执行型、结构化、结果导向".into()
}

fn default_user_md() -> String {
    "# user.md\n\n## 用户创作档案（待首次设定补全）".into()
}

fn default_creator_profile_md() -> String {
    "# CreatorProfile.md\n\n## 定位总览（待首次设定补全）".into()
}

fn default_bootstrap_md() -> String {
    "# BOOTSTRAP.md\n\n首次设定引导：通过聊天收集偏好后删除本文件。".into()
}

/// `profile:get-bundle`：返回完整 bundle。
fn profile_get_bundle(db: &Db) -> anyhow::Result<Value> {
    let meta = ensure_profile_meta(db)?;
    Ok(json!({
        "success": true,
        "profileRoot": "redclaw/profile",
        "agent": meta.get("agent").cloned().unwrap_or(Value::Null),
        "soul": meta.get("soul").cloned().unwrap_or(Value::Null),
        "identity": meta.get("identity").cloned().unwrap_or(Value::Null),
        "user": meta.get("user").cloned().unwrap_or(Value::Null),
        "creatorProfile": meta.get("creatorProfile").cloned().unwrap_or(Value::Null),
        "bootstrap": meta.get("bootstrap").cloned().unwrap_or(Value::Null),
        "onboardingState": meta.get("onboardingState").cloned().unwrap_or(Value::Null),
    }))
}

/// `profile:update-doc`：更新 agent/soul/user/creatorProfile markdown，bump version。
fn profile_update_doc(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let doc_type = opt_str(payload, "docType").unwrap_or("").trim().to_string();
    let markdown = opt_str(payload, "markdown").unwrap_or("").to_string();
    if doc_type.is_empty() {
        return Ok(json!({ "success": false, "error": "docType is required" }));
    }
    if markdown.trim().is_empty() {
        return Ok(json!({ "success": false, "error": "markdown is required" }));
    }
    let (key, file_name, title) = match doc_type.as_str() {
        "agent" => ("agent", "Agent.md", "Agent.md"),
        "soul" => ("soul", "Soul.md", "Soul.md"),
        "user" => ("user", "user.md", "user.md"),
        "creator_profile" => ("creatorProfile", "CreatorProfile.md", "CreatorProfile.md"),
        other => {
            return Ok(
                json!({ "success": false, "error": format!("Unsupported profile doc type: {other}") }),
            )
        }
    };
    let normalized = normalize_doc_markdown(title, &markdown);

    if dry_run(payload) {
        return Ok(json!({
            "success": true,
            "dryRun": true,
            "docType": doc_type,
            "fileName": file_name,
            "path": format!("redclaw/profile/{file_name}"),
            "content": normalized,
            "reason": opt_str(payload, "reason"),
        }));
    }

    let mut meta = ensure_profile_meta(db)?;
    if let Some(m) = meta.as_object_mut() {
        m.insert(key.into(), json!(normalized));
        let version = m.get("version").and_then(|v| v.as_i64()).unwrap_or(1) + 1;
        m.insert("version".into(), json!(version));
        m.insert("updatedAt".into(), json!(now_ts()));
    }
    save_profile_meta(db, &meta)?;
    Ok(json!({
        "success": true,
        "docType": doc_type,
        "fileName": file_name,
        "path": format!("redclaw/profile/{file_name}"),
        "content": normalized,
        "reason": opt_str(payload, "reason"),
    }))
}

fn normalize_doc_markdown(title: &str, markdown: &str) -> String {
    let normalized = markdown.trim();
    if normalized.is_empty() {
        return format!("# {title}\n");
    }
    if normalized.starts_with('#') {
        normalized.to_string()
    } else {
        format!("# {title}\n\n{normalized}")
    }
}

/// `profile:onboarding-status`。
fn profile_onboarding_status(db: &Db) -> anyhow::Result<Value> {
    let meta = ensure_profile_meta(db)?;
    let state = meta
        .get("onboardingState")
        .cloned()
        .unwrap_or_else(default_onboarding_state);
    let completed = state.get("completedAt").is_some();
    Ok(json!({ "success": true, "completed": completed, "state": state }))
}

/// `profile:onboarding-turn`：确定性状态机（无需 LLM）。
fn profile_onboarding_turn(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let input = opt_str(payload, "input").unwrap_or("").trim().to_string();
    let mut meta = ensure_profile_meta(db)?;
    let mut state = meta
        .get("onboardingState")
        .cloned()
        .unwrap_or_else(default_onboarding_state);
    let mut state_map = state.as_object().cloned().unwrap_or_default();

    if state_map.get("completedAt").is_some() {
        return Ok(
            json!({ "success": true, "handled": false, "completed": true, "responseText": Value::Null,
            "result": { "responseText": Value::Null, "completed": true } }),
        );
    }

    let now = now_ts();
    let asked = state_map
        .get("askedFirstQuestion")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !asked {
        state_map.insert("askedFirstQuestion".into(), json!(true));
        state_map.entry("startedAt").or_insert(json!(now));
        state_map.insert("stepIndex".into(), json!(0));
        state_map.insert("updatedAt".into(), json!(now));
        state = Value::Object(state_map.clone());
        if let Some(m) = meta.as_object_mut() {
            m.insert("onboardingState".into(), state.clone());
        }
        save_profile_meta(db, &meta)?;
        let text = format!(
            "在开始创作前，我们先做一次商媒运营助手个性化设定（只需 1-2 分钟）。\n{}\n\n你也可以回复“跳过”使用默认配置，后续随时可再改。",
            ONBOARDING_STEPS[0].1
        );
        return Ok(json!({
            "success": true, "handled": true, "completed": false, "responseText": text,
            "result": { "responseText": text, "completed": false }
        }));
    }

    if input.is_empty() {
        let step_idx = state_map
            .get("stepIndex")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as usize;
        let step = &ONBOARDING_STEPS[step_idx.min(ONBOARDING_STEPS.len() - 1)];
        let text = format!("我需要你先回答这个设定问题：\n{}", step.1);
        return Ok(json!({
            "success": true, "handled": true, "completed": false, "responseText": text,
            "result": { "responseText": text, "completed": false }
        }));
    }

    let skip = matches!(
        input.to_lowercase().as_str(),
        "跳过" | "先跳过" | "使用默认" | "默认" | "/skip" | "skip"
    );

    if skip {
        let answers = state_map
            .entry("answers")
            .or_insert_with(|| json!({}))
            .as_object()
            .cloned()
            .unwrap_or_default();
        let mut answers = answers;
        for step in ONBOARDING_STEPS {
            if !answers.contains_key(step.0) {
                answers.insert(step.0.into(), json!(step.2));
            }
        }
        state_map.insert("answers".into(), Value::Object(answers));
        state_map.insert("stepIndex".into(), json!(ONBOARDING_STEPS.len()));
        let (meta, _completed) = finalize_onboarding(meta, state_map, &json!({}))?;
        save_profile_meta(db, &meta)?;
        let text = "已按默认配置完成商媒运营助手设定，并写入当前空间档案与长期记忆。现在可以直接给我创作目标。";
        return Ok(json!({
            "success": true, "handled": true, "completed": true, "responseText": text,
            "result": { "responseText": text, "completed": true }
        }));
    }

    // 记录当前步骤答案，前进
    let step_idx = state_map
        .get("stepIndex")
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as usize;
    let cur = &ONBOARDING_STEPS[step_idx.min(ONBOARDING_STEPS.len() - 1)];
    let answers_obj = state_map
        .entry("answers")
        .or_insert_with(|| json!({}))
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut answers_obj = answers_obj;
    answers_obj.insert(cur.0.into(), json!(input));
    state_map.insert("answers".into(), Value::Object(answers_obj));
    let next_idx = step_idx + 1;
    state_map.insert("stepIndex".into(), json!(next_idx as i64));

    if next_idx >= ONBOARDING_STEPS.len() {
        let (meta, completed) = finalize_onboarding(meta, state_map, &json!({}))?;
        save_profile_meta(db, &meta)?;
        let text = "设定完成。我已经更新了 Agent/Soul/identity/user 档案。接下来直接告诉我你的创作目标即可。";
        return Ok(json!({
            "success": true, "handled": true, "completed": completed, "responseText": text,
            "result": { "responseText": text, "completed": completed }
        }));
    }

    state_map.insert("updatedAt".into(), json!(now));
    state = Value::Object(state_map);
    if let Some(m) = meta.as_object_mut() {
        m.insert("onboardingState".into(), state);
    }
    save_profile_meta(db, &meta)?;
    let next = &ONBOARDING_STEPS[next_idx];
    let text = format!(
        "已记录（{}/{}）。\n{}\n\n如果你想快速完成，也可以回复“跳过”。",
        next_idx,
        ONBOARDING_STEPS.len(),
        next.1
    );
    Ok(json!({
        "success": true, "handled": true, "completed": false, "responseText": text,
        "result": { "responseText": text, "completed": false }
    }))
}

/// `profile:save-initialization-progress`：保存 UI draft 进度。
fn profile_save_initialization_progress(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let mut meta = ensure_profile_meta(db)?;
    let now = now_ts();
    let step_index = opt_i64(payload, "stepIndex").unwrap_or(0);
    let answers = payload.get("answers").cloned().unwrap_or(json!({}));
    let state = meta
        .get("onboardingState")
        .cloned()
        .unwrap_or_else(default_onboarding_state);
    let mut state_map = state.as_object().cloned().unwrap_or_default();
    let ui_flow = state_map
        .entry("uiFlow")
        .or_insert_with(|| json!({}))
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut ui_flow = ui_flow;
    ui_flow.insert(
        "draft".into(),
        json!({ "stepIndex": step_index.max(0), "answers": answers }),
    );
    ui_flow.insert("updatedAt".into(), json!(now));
    state_map.insert("uiFlow".into(), Value::Object(ui_flow));
    state_map.insert("updatedAt".into(), json!(now));
    if let Some(m) = meta.as_object_mut() {
        m.insert("onboardingState".into(), Value::Object(state_map));
        m.insert("updatedAt".into(), json!(now));
    }
    save_profile_meta(db, &meta)?;
    let state_out = meta.get("onboardingState").cloned().unwrap_or(Value::Null);
    Ok(json!({ "success": true, "state": state_out }))
}

/// `profile:complete-initialization`：summarize UI 答案 → finalize。
fn profile_complete_initialization(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let meta = ensure_profile_meta(db)?;
    let now = now_ts();
    let answers = payload.get("answers").cloned().unwrap_or(json!({}));
    let state = meta
        .get("onboardingState")
        .cloned()
        .unwrap_or_else(default_onboarding_state);
    let mut state_map = state.as_object().cloned().unwrap_or_default();

    let summarized = summarize_ui_answers(&answers);
    state_map.insert("answers".into(), summarized);
    state_map.insert("askedFirstQuestion".into(), json!(true));
    state_map.entry("startedAt").or_insert(json!(now));
    state_map.insert("stepIndex".into(), json!(ONBOARDING_STEPS.len() as i64));

    let mut ui_flow = state_map
        .get("uiFlow")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    ui_flow.insert(
        "draft".into(),
        json!({ "stepIndex": opt_i64(payload, "stepIndex").unwrap_or(ONBOARDING_STEPS.len() as i64), "answers": answers }),
    );
    ui_flow.insert("updatedAt".into(), json!(now));
    ui_flow.insert("completedAt".into(), json!(now));
    state_map.insert("uiFlow".into(), Value::Object(ui_flow));

    let (meta, _completed) = finalize_onboarding(meta, state_map, &answers)?;
    save_profile_meta(db, &meta)?;
    let latest = meta.get("onboardingState").cloned().unwrap_or(Value::Null);
    Ok(json!({
        "success": true,
        "onboardingState": latest,
        "profileRoot": "redclaw/profile",
    }))
}

/// finalize：从 5 个 canonical answers 生成 identity/user/soul/creatorProfile，标 completedAt，bump version。
fn finalize_onboarding(
    mut meta: Value,
    mut state_map: Map<String, Value>,
    _ui_answers: &Value,
) -> Result<(Value, bool), anyhow::Error> {
    let answers = state_map
        .get("answers")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let get_ans = |key: &str, fallback: &str| -> String {
        answers
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| fallback.to_string())
    };
    let style = get_ans("assistant_style", ONBOARDING_STEPS[0].2);
    let goal = get_ans("creator_goal", ONBOARDING_STEPS[1].2);
    let audience = get_ans("target_audience", ONBOARDING_STEPS[2].2);
    let lane = get_ans("content_lane", ONBOARDING_STEPS[3].2);
    let constraints = get_ans("tone_and_constraints", ONBOARDING_STEPS[4].2);
    let now = now_ts();

    let identity = format!(
        "# identity.md\n\n- Name: 商媒运营助手\n- Role: 多平台内容创作自动化 Agent\n- Vibe: {style}\n- Signature: 🦀\n- UpdatedAt: {now}"
    );
    let user = format!(
        "# user.md\n\n## 用户创作档案\n- 核心创作目标: {goal}\n- 目标用户画像: {audience}\n- 内容赛道与结构偏好: {lane}\n- 语气/边界/节奏/指标: {constraints}"
    );
    let soul = format!(
        "# Soul.md\n\n## 当前人格与协作偏好（来自首次设定）\n- 协作风格: {style}\n\n## 执行原则\n- 先明确目标，再拆解步骤。\n- 每一步要有产物和下一步动作。"
    );
    let creator = format!(
        "# CreatorProfile.md\n\n## 定位总览\n- 自媒体定位: 小红书创作与增长\n- 核心目标: {goal}\n## 目标群体\n- 核心受众: {audience}\n## 内容风格\n- 内容赛道: {lane}\n- 文案风格: {style}\n- 执行边界: {constraints}\n- UpdatedAt: {now}"
    );

    if let Some(m) = meta.as_object_mut() {
        m.insert("identity".into(), json!(identity));
        m.insert("user".into(), json!(user));
        m.insert("soul".into(), json!(soul));
        m.insert("creatorProfile".into(), json!(creator));
        m.insert("bootstrap".into(), Value::Null);
        let version = m.get("version").and_then(|v| v.as_i64()).unwrap_or(1) + 1;
        m.insert("version".into(), json!(version));
        m.insert("updatedAt".into(), json!(now));
    }

    state_map.insert("completedAt".into(), json!(now));
    state_map.insert("updatedAt".into(), json!(now));
    if let Some(m) = meta.as_object_mut() {
        m.insert("onboardingState".into(), Value::Object(state_map));
    }
    Ok((meta, true))
}

/// UI（百分比/选项）答案 → 5 个 canonical 字符串答案（对齐 Beav `summarizeUiOnboardingAnswers`）。
fn summarize_ui_answers(answers: &Value) -> Value {
    let read_pct = |k: &str, fb: i64| -> i64 {
        answers
            .get(k)
            .and_then(|v| v.as_f64())
            .map(|n| clamp_i64(n.round() as i64, 0, 100))
            .unwrap_or(fb)
    };
    let content_vs_commerce = read_pct("contentVsCommerce", 50);
    let persona_vs_brand = read_pct("personaVsBrand", 50);
    let consistency_vs_virality = read_pct("consistencyVsVirality", 50);
    let authority = read_pct("authorityPosture", 55);
    let emotional = read_pct("emotionalTemperature", 45);
    let sales = read_pct("salesExplicitness", 45);
    let structure = read_pct("structureValue", 60);
    let primary = read_choice(
        answers,
        "primaryModel",
        &[
            "persona-commerce",
            "brand-commerce",
            "service-conversion",
            "content-account",
        ],
        "brand-commerce",
    );
    let role = read_choice(
        answers,
        "rolePosition",
        &["advisor", "experienced", "experimenter", "founder"],
        "advisor",
    );
    let opening = read_choice(
        answers,
        "openingPreference",
        &["hook", "observational"],
        "observational",
    );
    let primary_label = match primary.as_str() {
        "persona-commerce" => "人设带货",
        "brand-commerce" => "品牌带货",
        "service-conversion" => "高客单服务转化",
        "content-account" => "纯内容账号",
        _ => primary.as_str(),
    };
    let role_label = match role.as_str() {
        "advisor" => "专业顾问",
        "experienced" => "有经验的过来人",
        "experimenter" => "真实试错者",
        "founder" => "品牌主理人",
        _ => role.as_str(),
    };
    let opening_label = match opening.as_str() {
        "hook" => "强判断钩子",
        "observational" => "观察式开头",
        _ => opening.as_str(),
    };

    json!({
        "assistant_style": format!(
            "专业判断 {authority}% / 亲近自然 {}; 情绪感染 {emotional}% / 冷静克制 {}; 框架拆解 {structure}% / 故事表达 {}",
            100 - authority, 100 - emotional, 100 - structure
        ),
        "creator_goal": format!(
            "经营方式：{primary_label}；内容导向 {}% / 商业导向 {content_vs_commerce}%；一致性 {}% / 爆发力 {consistency_vs_virality}%",
            100 - content_vs_commerce, 100 - consistency_vs_virality
        ),
        "target_audience": format!("受众主要把账号视为：{role_label}"),
        "content_lane": format!(
            "品牌驱动 {}% / 人设驱动 {persona_vs_brand}%；长期默认采用{opening_label}",
            100 - persona_vs_brand
        ),
        "tone_and_constraints": format!("转化表达强度 {sales}%；保持真实、清晰、合规，不使用夸张承诺"),
    })
}

fn read_choice(answers: &Value, key: &str, allowed: &[&str], fallback: &str) -> String {
    let v = answers
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if allowed.contains(&v) { v } else { fallback }.to_string()
}

// ===========================================================================
// task —— DB（agent_tasks task_type='redclaw-task'）
// ===========================================================================
//
// 状态机：`draft → confirmed → scheduled`（外加 `cancelled`）。
// metadata_json：`{title, goal, platform, taskType, projectId, prompt, source, ...}`。

/// `redclaw:task-preview`：构造快照不落库。
fn task_preview(payload: &Value) -> Value {
    build_task_snapshot(None, payload, dry_run(payload))
}

/// `redclaw:task-create`：INSERT status='draft'。
fn task_create(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let dry = dry_run(payload);
    let now = now_ts();
    let id = new_task_id("rct");
    let snapshot = build_task_snapshot(Some(&id), payload, dry);
    if dry {
        return Ok(snapshot);
    }
    let goal = opt_str(payload, "goal").unwrap_or("").to_string();
    db.execute_json(
        "INSERT INTO agent_tasks \
         (id, task_type, status, runtime_mode, owner_session_id, intent, role_id, goal, \
          current_node, route_json, graph_json, artifacts_json, checkpoints_json, metadata_json, \
          last_error, created_at, updated_at, started_at, completed_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        &[
            json!(id),
            json!("redclaw-task"),
            json!("draft"),
            json!("redclaw"),
            opt_value(&opt_str(payload, "sessionId").map(String::from)),
            json!("redclaw-task"),
            Value::Null,
            opt_value(&Some(goal).filter(|s| !s.is_empty())),
            Value::Null,
            json!("{}"),
            json!("[]"),
            json!("[]"),
            json!("[]"),
            json!(snapshot["metadata"].to_string()),
            Value::Null,
            json!(now),
            json!(now),
            Value::Null,
            Value::Null,
        ],
    )?;
    emit_data_changed(emitter, "redclaw-task", "create", &id);
    Ok(snapshot)
}

/// `redclaw:task-confirm`：draft → confirmed。
fn task_confirm(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    update_task_status(db, payload, "confirmed", &["draft"], emitter, "confirm")
}

/// `redclaw:task-cancel`：→ cancelled。
fn task_cancel(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    update_task_status(
        db,
        payload,
        "cancelled",
        &["draft", "confirmed", "scheduled"],
        emitter,
        "cancel",
    )
}

/// `redclaw:task-update`：合并更新 metadata 字段。
fn task_update(db: &Db, emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "taskId") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(Value::Null),
    };
    let row = db.query_one_json(
        "SELECT * FROM agent_tasks WHERE id = ?1 AND task_type = 'redclaw-task'",
        &[json!(&id)],
    )?;
    let row = match row {
        Some(r) => r,
        None => return Ok(Value::Null),
    };
    let mut meta = parse_json_col(row.get("metadata_json"), &json!({}));
    if let Some(m) = meta.as_object_mut() {
        for k in [
            "title",
            "goal",
            "platform",
            "taskType",
            "projectId",
            "prompt",
            "source",
        ] {
            if let Some(v) = payload.get(k) {
                m.insert(k.into(), v.clone());
            }
        }
    }
    if dry_run(payload) {
        let mut snap = hydrate_redclaw_task(&row);
        if let Some(s) = snap.as_object_mut() {
            s.insert("metadata".into(), meta);
            s.insert("dryRun".into(), json!(true));
        }
        return Ok(snap);
    }
    let now = now_ts();
    db.execute_json(
        "UPDATE agent_tasks SET metadata_json = ?1, updated_at = ?2 WHERE id = ?3",
        &[json!(meta.to_string()), json!(now), json!(&id)],
    )?;
    emit_data_changed(emitter, "redclaw-task", "update", &id);
    let fresh = db.query_one_json(
        "SELECT * FROM agent_tasks WHERE id = ?1 AND task_type = 'redclaw-task'",
        &[json!(&id)],
    )?;
    Ok(fresh
        .map(|r| hydrate_redclaw_task(&r))
        .unwrap_or(Value::Null))
}

/// `redclaw:task-list`：可按 status 过滤。
fn task_list(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let (where_, params) = match opt_str(payload, "status") {
        Some(st) if !st.is_empty() => (
            "WHERE task_type = 'redclaw-task' AND status = ?1",
            vec![json!(st)],
        ),
        _ => ("WHERE task_type = 'redclaw-task'", vec![]),
    };
    let limit = clamp_i64(opt_i64(payload, "limit").unwrap_or(100), 1, 500);
    let mut params = params;
    params.push(json!(limit));
    let sql = format!(
        "SELECT * FROM agent_tasks {where_} ORDER BY updated_at DESC LIMIT ?{}",
        params.len()
    );
    let rows = db.query_all_json(&sql, &params)?;
    let out: Vec<Value> = rows.iter().map(hydrate_redclaw_task).collect();
    Ok(json!(out))
}

/// `redclaw:task-stats`：按 status 聚合计数。
fn task_stats(db: &Db) -> anyhow::Result<Value> {
    let rows = db.query_all_json(
        "SELECT status, COUNT(*) AS n FROM agent_tasks WHERE task_type = 'redclaw-task' GROUP BY status",
        &[],
    )?;
    let mut stats = Map::new();
    let mut total: i64 = 0;
    for r in rows {
        let st = r
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let n = r.get("n").and_then(|v| v.as_i64()).unwrap_or(0);
        total += n;
        stats.insert(st, json!(n));
    }
    stats.insert("total".into(), json!(total));
    Ok(json!(stats))
}

fn update_task_status(
    db: &Db,
    payload: &Value,
    target: &str,
    allowed_from: &[&str],
    emitter: &dyn EventEmitter,
    action: &str,
) -> anyhow::Result<Value> {
    let id = match opt_str(payload, "taskId") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return Ok(Value::Null),
    };
    let row = db.query_one_json(
        "SELECT * FROM agent_tasks WHERE id = ?1 AND task_type = 'redclaw-task'",
        &[json!(&id)],
    )?;
    let row = match row {
        Some(r) => r,
        None => return Ok(Value::Null),
    };
    let current = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if dry_run(payload) {
        let mut snap = hydrate_redclaw_task(&row);
        if let Some(s) = snap.as_object_mut() {
            s.insert("status".into(), json!(target));
            s.insert("dryRun".into(), json!(true));
        }
        return Ok(snap);
    }
    if !allowed_from.contains(&current) {
        anyhow::bail!("task {id} 状态 {current} 不允许转到 {target}");
    }
    let now = now_ts();
    db.execute_json(
        "UPDATE agent_tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
        &[json!(target), json!(now), json!(&id)],
    )?;
    emit_data_changed(emitter, "redclaw-task", action, &id);
    let fresh = db.query_one_json(
        "SELECT * FROM agent_tasks WHERE id = ?1 AND task_type = 'redclaw-task'",
        &[json!(&id)],
    )?;
    Ok(fresh
        .map(|r| hydrate_redclaw_task(&r))
        .unwrap_or(Value::Null))
}

fn build_task_snapshot(id: Option<&str>, payload: &Value, dry: bool) -> Value {
    let now = now_ts();
    let id = id.map(String::from).unwrap_or_else(|| new_task_id("rct"));
    let mut metadata = match payload.get("metadata").and_then(|v| v.as_object()) {
        Some(o) => o.clone(),
        None => Map::new(),
    };
    for k in [
        "title",
        "goal",
        "platform",
        "taskType",
        "projectId",
        "prompt",
        "source",
    ] {
        if let Some(v) = payload.get(k) {
            metadata.insert(k.into(), v.clone());
        }
    }
    if !metadata.contains_key("title") {
        metadata.insert(
            "title".into(),
            json!(opt_str(payload, "goal").unwrap_or("RedClaw Task")),
        );
    }
    json!({
        "id": id,
        "taskType": "redclaw-task",
        "status": "draft",
        "runtimeMode": "redclaw",
        "ownerSessionId": opt_str(payload, "sessionId"),
        "goal": opt_str(payload, "goal"),
        "metadata": Value::Object(metadata),
        "createdAt": now,
        "updatedAt": now,
        "dryRun": dry,
    })
}

fn hydrate_redclaw_task(row: &Value) -> Value {
    json!({
        "id": row.get("id").cloned().unwrap_or(Value::Null),
        "taskType": "redclaw-task",
        "status": row.get("status").cloned().unwrap_or(Value::Null),
        "runtimeMode": row.get("runtime_mode").cloned().unwrap_or(Value::Null),
        "ownerSessionId": row.get("owner_session_id").cloned().unwrap_or(Value::Null),
        "goal": row.get("goal").cloned().unwrap_or(Value::Null),
        "metadata": parse_json_col(row.get("metadata_json"), &json!({})),
        "createdAt": row.get("created_at").cloned().unwrap_or(json!(0)),
        "updatedAt": row.get("updated_at").cloned().unwrap_or(json!(0)),
    })
}

// ===========================================================================
// 测试
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::ipc::NoopEmitter;
    use serde_json::json;
    use std::sync::Arc;

    fn noop() -> Arc<dyn EventEmitter> {
        Arc::new(NoopEmitter)
    }

    #[test]
    fn profile_bundle_and_update_doc_db() {
        let db = Db::open_in_memory().unwrap();
        let bundle = profile_get_bundle(&db).unwrap();
        assert_eq!(bundle["success"], json!(true));
        assert!(bundle["agent"].is_string());
        // 默认未完成 onboarding
        let status = profile_onboarding_status(&db).unwrap();
        assert_eq!(status["completed"], json!(false));

        // update-doc（confirm 落库）
        let updated = profile_update_doc(
            &db,
            &json!({ "docType": "agent", "markdown": "# Agent.md\n\n新契约", "confirm": true }),
        )
        .unwrap();
        assert_eq!(updated["success"], json!(true));
        assert_eq!(updated["fileName"], json!("Agent.md"));

        // 再次读取应含新内容 + version 自增
        let bundle2 = profile_get_bundle(&db).unwrap();
        let meta = ensure_profile_meta(&db).unwrap();
        assert!(bundle2["agent"].as_str().unwrap().contains("新契约"));
        assert!(meta["version"].as_i64().unwrap() >= 2);

        // dry_run 不落库
        let before = ensure_profile_meta(&db).unwrap()["version"]
            .as_i64()
            .unwrap();
        let _ = profile_update_doc(
            &db,
            &json!({ "docType": "soul", "markdown": "# Soul\n临时" }),
        )
        .unwrap();
        let after = ensure_profile_meta(&db).unwrap()["version"]
            .as_i64()
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn profile_onboarding_turn_flow_db() {
        let db = Db::open_in_memory().unwrap();
        // 第一次 turn：发首问
        let t1 = profile_onboarding_turn(&db, &json!({ "input": "" })).unwrap();
        assert_eq!(t1["handled"], json!(true));
        assert!(t1["responseText"].as_str().unwrap().contains("1/5"));
        // 回答 5 步
        for _ in 0..ONBOARDING_STEPS.len() {
            let _ = profile_onboarding_turn(&db, &json!({ "input": "测试回答" })).unwrap();
        }
        // 应已完成
        let status = profile_onboarding_status(&db).unwrap();
        assert_eq!(status["completed"], json!(true));
        // 再 turn：handled=false
        let again = profile_onboarding_turn(&db, &json!({ "input": "x" })).unwrap();
        assert_eq!(again["handled"], json!(false));

        // skip 分支：新建一个 profile 行先清空（同一行已 completed，故 skip 走 handled:false）。
        // 这里改测 save/complete UI 流程。
        let saved = profile_save_initialization_progress(
            &db,
            &json!({ "stepIndex": 2, "answers": { "authorityPosture": 70 } }),
        )
        .unwrap();
        assert_eq!(saved["success"], json!(true));
    }

    #[test]
    fn redclaw_task_lifecycle_db() {
        let db = Db::open_in_memory().unwrap();
        let em = noop();
        let emitter: &dyn EventEmitter = &*em;

        // preview（不落库）
        let prev = task_preview(&json!({ "goal": "写小红书", "projectId": "rc_1" }));
        assert_eq!(prev["status"], json!("draft"));
        assert!(prev["dryRun"].as_bool().unwrap());

        // create（confirm 落库）
        let created = task_create(
            &db,
            emitter,
            &json!({ "goal": "写小红书", "projectId": "rc_1", "confirm": true }),
        )
        .unwrap();
        let id = created["id"].as_str().unwrap().to_string();
        assert_eq!(created["status"], json!("draft"));

        // list
        assert_eq!(
            task_list(&db, &json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // confirm（draft→confirmed）
        let confirmed =
            task_confirm(&db, emitter, &json!({ "taskId": &id, "confirm": true })).unwrap();
        assert_eq!(confirmed["status"], json!("confirmed"));

        // update metadata（confirm）
        let updated = task_update(
            &db,
            emitter,
            &json!({ "taskId": &id, "prompt": "新提示", "confirm": true }),
        )
        .unwrap();
        assert_eq!(updated["metadata"]["prompt"], json!("新提示"));

        // cancel
        let cancelled =
            task_cancel(&db, emitter, &json!({ "taskId": &id, "confirm": true })).unwrap();
        assert_eq!(cancelled["status"], json!("cancelled"));

        // stats
        let stats = task_stats(&db).unwrap();
        assert_eq!(stats["total"], json!(1));
        assert_eq!(stats["cancelled"], json!(1));

        // 非法状态转移（cancelled→confirmed）应失败
        assert!(task_confirm(&db, emitter, &json!({ "taskId": &id, "confirm": true })).is_err());
    }

    #[test]
    fn list_projects_in_dir_with_tempdir() {
        let tmp = std::env::temp_dir().join(format!("redclaw_test_{}", short_suffix()));
        let projects = tmp.join("projects");
        let p1 = projects.join("rc_1");
        let p2 = projects.join("rc_2");
        std::fs::create_dir_all(&p1).unwrap();
        std::fs::create_dir_all(&p2).unwrap();
        std::fs::write(
            p1.join("project.json"),
            json!({ "id": "rc_1", "goal": "g1", "updatedAt": "2024-01-01T00:00:00Z" }).to_string(),
        )
        .unwrap();
        std::fs::write(
            p2.join("project.json"),
            json!({ "id": "rc_2", "goal": "g2", "updatedAt": "2024-06-01T00:00:00Z" }).to_string(),
        )
        .unwrap();

        let list = list_projects_in_dir(&projects, 20);
        let arr = list.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        // 按 updatedAt desc → rc_2 在前
        assert_eq!(arr[0]["id"], json!("rc_2"));
        assert!(arr[0]["projectDir"].as_str().unwrap().contains("rc_2"));

        // get 单个
        let got = get_project_in_dir(&projects, "rc_1").unwrap();
        assert_eq!(got["project"]["id"], json!("rc_1"));

        // 不存在目录 → 空列表
        let empty = list_projects_in_dir(std::path::Path::new("/no/such/redclaw/path"), 20);
        assert_eq!(empty.as_array().unwrap().len(), 0);

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    #[ignore = "需要真实系统文件管理器（webbrowser 打开窗口）"]
    fn open_project_invokes_webbrowser() {
        let res = open_project(&json!({ "projectDir": "/tmp" }));
        assert_eq!(res["success"], json!(true));
    }
}
