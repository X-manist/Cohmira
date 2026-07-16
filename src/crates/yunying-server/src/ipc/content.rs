//! 内容命名空间（`content`）IPC 通道：memory / archives / manuscripts。
//!
//! 对应 Beav `electron/main.ts` 中以下三组 `ipcMain.handle`：
//! - **memory**：`memory:list/archived/history/search/add/delete/update/maintenance-status`。
//! - **archives**：`archives:list/create/update/delete` + `archives:samples:list/create/update/delete`。
//! - **manuscripts**：`manuscripts:list/read/save/delete/rename/move/create-folder/create-file/
//!   get-layout/save-layout/format-wechat/get-package-state`。
//!
//! ## 存储策略
//! - memory → `user_memories` 表（按 `active_space_id` 作用域，从 settings 取，缺省 `default`）。
//! - archives → `archive_profiles` + `archive_samples` 表。
//! - manuscripts → 文件系统，根目录 = `settings.workspace_dir/manuscripts`（缺省 `~/.redconvert/manuscripts`）。
//!
//! ## 与 TS 实现的差异
//! - TS 的 memory 走文件存储（`fileMemoryStore`，含归档/历史/去重）。本实现按规格要求改用 DB
//!   （`user_memories`），而该表无 `archived`/历史列，故 `memory:archived`/`memory:history`
//!   返回空数组（结构完整，待后续扩展列或独立表）。
//! - `memory:maintenance-status` 为占位状态（后台维护服务未迁移）。
//! - `manuscripts:format-wechat` 返回原文（占位）。
//! - frontmatter 解析为简化版（行式 `key: value`，数组/对象用 JSON），保证常见字段往返一致。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::AppState;
use crate::db::Db;

/// content 命名空间双向通道分发。未知通道返回 `Err`。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        // ---- memory ----
        m @ ("memory:list"
        | "memory:archived"
        | "memory:history"
        | "memory:search"
        | "memory:add"
        | "memory:delete"
        | "memory:update"
        | "memory:maintenance-status") => {
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let space_id = active_space_id(&settings);
            memory_dispatch(m, &payload, &space_id, &state.db)
        }
        // ---- archives ----
        a @ ("archives:list"
        | "archives:create"
        | "archives:update"
        | "archives:delete"
        | "archives:samples:list"
        | "archives:samples:create"
        | "archives:samples:update"
        | "archives:samples:delete") => archives_dispatch(a, &payload, state),
        // ---- manuscripts ----
        ms @ ("manuscripts:list"
        | "manuscripts:read"
        | "manuscripts:save"
        | "manuscripts:delete"
        | "manuscripts:rename"
        | "manuscripts:move"
        | "manuscripts:create-folder"
        | "manuscripts:create-file"
        | "manuscripts:get-layout"
        | "manuscripts:save-layout"
        | "manuscripts:format-wechat"
        | "manuscripts:get-package-state") => {
            let settings = state.db.settings().get().unwrap_or_else(|_| json!({}));
            let root = manuscripts_root(&settings);
            manuscripts_dispatch(ms, &payload, &root, state).await
        }
        other => Err(anyhow::anyhow!("content 通道未实现: {other}")),
    }
}

// =============================================================================
// memory
// =============================================================================

fn memory_dispatch(
    channel: &str,
    payload: &Value,
    space_id: &str,
    db: &Db,
) -> anyhow::Result<Value> {
    match channel {
        "memory:list" => Ok(json!(memory_list(db, space_id)?)),
        // DB 表无 archived 列：返回空数组（结构对齐）。
        "memory:archived" => Ok(json!([])),
        // 无独立历史表：返回空数组。
        "memory:history" => Ok(json!([])),
        "memory:search" => {
            let query = opt_str(payload, "query").unwrap_or("").to_lowercase();
            let include_archived = payload
                .get("includeArchived")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let limit = payload
                .get("limit")
                .and_then(|v| v.as_i64())
                .unwrap_or(20)
                .clamp(1, 100) as usize;
            Ok(json!(memory_search(
                db,
                space_id,
                &query,
                include_archived,
                limit
            )?))
        }
        "memory:add" => {
            let content = need_str(payload, "content")?;
            let type_ = opt_str(payload, "type").unwrap_or("general");
            let tags = opt_str_array(payload, "tags");
            memory_add(db, space_id, content, type_, &tags)
        }
        "memory:delete" => {
            let id = extract_id(payload, &["id"]).ok_or_else(|| anyhow::anyhow!("缺少字段 id"))?;
            memory_delete(db, space_id, &id)
        }
        "memory:update" => {
            let id = extract_id(payload, &["id"]).ok_or_else(|| anyhow::anyhow!("缺少字段 id"))?;
            let upd = payload.get("updates").filter(|v| v.is_object());
            let content = upd
                .and_then(|u| u.get("content"))
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("content").and_then(|v| v.as_str()));
            let type_ = upd
                .and_then(|u| u.get("type"))
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("type").and_then(|v| v.as_str()));
            let tags_val = upd
                .and_then(|u| u.get("tags"))
                .or_else(|| payload.get("tags"));
            let tags = tags_val.map(opt_str_array_owned);
            memory_update(db, space_id, &id, content, type_, tags.as_deref())
        }
        // 后台维护服务未迁移：返回空闲占位状态。
        "memory:maintenance-status" => Ok(json!({
            "started": false,
            "running": false,
            "lockState": "passive",
            "blockedBy": null,
            "pendingMutations": 0,
            "lastRunAt": null,
            "lastScanAt": null,
            "lastReason": null,
            "lastSummary": "",
            "lastError": null,
            "nextScheduledAt": null
        })),
        _ => Err(anyhow::anyhow!("memory 通道未实现: {channel}")),
    }
}

/// 列出当前空间的全部记忆（按 created_at 倒序）。
fn memory_list(db: &Db, space_id: &str) -> anyhow::Result<Vec<Value>> {
    let rows = db.query_all_json(
        "SELECT id, space_id, content, type, tags, created_at, updated_at, last_accessed \
         FROM user_memories WHERE space_id = ? ORDER BY created_at DESC",
        &[json!(space_id)],
    )?;
    Ok(rows.into_iter().map(decorate_memory).collect())
}

/// 新增记忆；返回新建对象。
fn memory_add(
    db: &Db,
    space_id: &str,
    content: &str,
    type_: &str,
    tags: &[String],
) -> anyhow::Result<Value> {
    let content = content.trim();
    if content.is_empty() {
        anyhow::bail!("记忆内容不能为空");
    }
    let now = now_ts();
    let id = gen_id("mem");
    let type_ = normalize_type(type_);
    let tags_json = serde_json::to_string(tags)?;
    db.execute_json(
        "INSERT INTO user_memories \
         (id, space_id, content, type, tags, created_at, updated_at, last_accessed) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            json!(id),
            json!(space_id),
            json!(content),
            json!(type_),
            json!(tags_json),
            json!(now),
            json!(now),
            json!(now),
        ],
    )?;
    Ok(json!({
        "id": id,
        "space_id": space_id,
        "content": content,
        "type": type_,
        "tags": tags,
        "created_at": now,
        "updated_at": now,
        "last_accessed": now,
    }))
}

fn memory_delete(db: &Db, space_id: &str, id: &str) -> anyhow::Result<Value> {
    let n = db.execute_json(
        "DELETE FROM user_memories WHERE id = ? AND space_id = ?",
        &[json!(id), json!(space_id)],
    )?;
    Ok(json!({ "ok": true, "deleted": n }))
}

/// 按 `updates` 子对象动态 UPDATE；返回受影响行数。
fn memory_update(
    db: &Db,
    space_id: &str,
    id: &str,
    content: Option<&str>,
    type_: Option<&str>,
    tags: Option<&[String]>,
) -> anyhow::Result<Value> {
    let now = now_ts();
    let mut sets: Vec<String> = vec!["updated_at = ?".into()];
    let mut params: Vec<Value> = vec![json!(now)];
    if let Some(c) = content {
        sets.push("content = ?".into());
        params.push(json!(c));
    }
    if let Some(t) = type_ {
        sets.push("type = ?".into());
        params.push(json!(normalize_type(t)));
    }
    if let Some(tg) = tags {
        sets.push("tags = ?".into());
        params.push(json!(serde_json::to_string(tg)?));
    }
    params.push(json!(id));
    params.push(json!(space_id));
    let sql = format!(
        "UPDATE user_memories SET {} WHERE id = ? AND space_id = ?",
        sets.join(", ")
    );
    let n = db.execute_json(&sql, &params)?;
    Ok(json!({ "ok": true, "updated": n }))
}

/// 关键字评分搜索（移植自 `searchUserMemoriesInFile` 的简化版）。
fn memory_search(
    db: &Db,
    space_id: &str,
    query_lower: &str,
    _include_archived: bool,
    limit: usize,
) -> anyhow::Result<Vec<Value>> {
    if query_lower.is_empty() {
        return Ok(Vec::new());
    }
    let tokens: Vec<String> = query_lower.split_whitespace().map(String::from).collect();
    let now = now_ts();
    let mut scored: Vec<Value> = memory_list(db, space_id)?
        .into_iter()
        .filter_map(|mem| {
            let (score, reasons) = score_memory(&mem, query_lower, &tokens, now);
            if score <= 0 {
                return None;
            }
            let mut m = mem;
            if let Some(obj) = m.as_object_mut() {
                obj.insert("score".into(), json!(score));
                obj.insert("matchReasons".into(), json!(reasons));
            }
            Some(m)
        })
        .collect();
    scored.sort_by(|a, b| {
        let sa = a.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        let sb = b.get("score").and_then(|v| v.as_i64()).unwrap_or(0);
        sb.cmp(&sa)
    });
    scored.truncate(limit);
    Ok(scored)
}

fn score_memory(mem: &Value, query_lower: &str, tokens: &[String], now: i64) -> (i64, Vec<String>) {
    let content = mem.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let content_lower = content.to_lowercase();
    let mtype = mem.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let tags: Vec<String> = mem
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let mut score: i64 = 0;
    let mut reasons: Vec<String> = Vec::new();

    if content_lower == query_lower {
        score += 120;
        reasons.push("exact-content".into());
    } else if content_lower.contains(query_lower) {
        score += 80;
        reasons.push("content-contains".into());
    }
    if tags.iter().any(|t| t.to_lowercase() == query_lower) {
        score += 60;
        reasons.push("tag-exact".into());
    }
    if mtype.to_lowercase() == query_lower {
        score += 40;
        reasons.push("type-exact".into());
    }
    let haystack = format!(
        "{}\n{}\n{}",
        content_lower,
        tags.join(" "),
        mtype.to_lowercase()
    );
    for tok in tokens {
        if haystack.contains(tok.as_str()) {
            score += 12;
        }
    }
    let updated = mem
        .get("updated_at")
        .and_then(|v| v.as_i64())
        .or_else(|| mem.get("created_at").and_then(|v| v.as_i64()))
        .unwrap_or(now);
    let weeks = ((now - updated) / (1000 * 60 * 60 * 24 * 7)).max(0);
    // recency 仅作为排序加权：无任何内容/标签/类型/token 命中时不计（避免新鲜但无关的记忆被返回）。
    if score > 0 {
        score += (10 - weeks).max(0);
    }
    (score, reasons)
}

// =============================================================================
// archives
// =============================================================================

fn archives_dispatch(channel: &str, payload: &Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        "archives:list" => Ok(json!(archives_list(&state.db)?)),
        "archives:create" => {
            let res = archives_create(&state.db, payload)?;
            if let Some(id) = res.get("id").and_then(|v| v.as_str()) {
                emit_data_changed(state, "archives", "create", Some(id));
            }
            Ok(res)
        }
        "archives:update" => {
            let res = archives_update(&state.db, payload)?;
            if let Some(id) = res.get("id").and_then(|v| v.as_str()) {
                emit_data_changed(state, "archives", "update", Some(id));
            }
            Ok(res)
        }
        "archives:delete" => {
            let id = extract_id(payload, &["id", "profileId"])
                .ok_or_else(|| anyhow::anyhow!("缺少字段 id/profileId"))?;
            let res = archives_delete(&state.db, &id)?;
            emit_data_changed(state, "archives", "delete", Some(&id));
            Ok(res)
        }
        "archives:samples:list" => {
            let profile_id = extract_id(payload, &["profileId", "id", "profile_id"])
                .ok_or_else(|| anyhow::anyhow!("缺少字段 profileId"))?;
            Ok(json!(archive_samples_list(&state.db, &profile_id)?))
        }
        "archives:samples:create" => {
            let res = archive_sample_create(&state.db, payload)?;
            if let Some(id) = res.get("id").and_then(|v| v.as_str()) {
                emit_data_changed(state, "archives", "sample-create", Some(id));
            }
            Ok(res)
        }
        "archives:samples:update" => {
            let res = archive_sample_update(&state.db, payload)?;
            if let Some(id) = res.get("id").and_then(|v| v.as_str()) {
                emit_data_changed(state, "archives", "sample-update", Some(id));
            }
            Ok(res)
        }
        "archives:samples:delete" => {
            let id = extract_id(payload, &["id", "sampleId"])
                .ok_or_else(|| anyhow::anyhow!("缺少字段 id/sampleId"))?;
            let res = archive_sample_delete(&state.db, &id)?;
            emit_data_changed(state, "archives", "sample-delete", Some(&id));
            Ok(res)
        }
        _ => Err(anyhow::anyhow!("archives 通道未实现: {channel}")),
    }
}

fn archives_list(db: &Db) -> anyhow::Result<Vec<Value>> {
    let rows = db.query_all_json(
        "SELECT id, name, platform, goal, domain, audience, tone_tags, created_at, updated_at \
         FROM archive_profiles ORDER BY updated_at DESC",
        &[],
    )?;
    Ok(rows.into_iter().map(decorate_profile).collect())
}

fn archives_create(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let name = need_str(payload, "name")?;
    let platform = opt_str(payload, "platform").unwrap_or("");
    let goal = opt_str(payload, "goal").unwrap_or("");
    let domain = opt_str(payload, "domain").unwrap_or("");
    let audience = opt_str(payload, "audience").unwrap_or("");
    let tone_tags = opt_str_array(payload, "toneTags");
    if is_dry_run(payload) {
        return Ok(dry_run_preview(json!({
            "id": null,
            "name": name,
            "platform": platform,
            "goal": goal,
            "domain": domain,
            "audience": audience,
            "tone_tags": tone_tags,
        })));
    }
    let now = now_ts();
    let id = gen_id("archive");
    db.execute_json(
        "INSERT INTO archive_profiles \
         (id, name, platform, goal, domain, audience, tone_tags, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            json!(id),
            json!(name),
            json!(platform),
            json!(goal),
            json!(domain),
            json!(audience),
            json!(serde_json::to_string(&tone_tags)?),
            json!(now),
            json!(now),
        ],
    )?;
    Ok(json!({
        "id": id,
        "name": name,
        "platform": platform,
        "goal": goal,
        "domain": domain,
        "audience": audience,
        "tone_tags": tone_tags,
        "created_at": now,
        "updated_at": now,
    }))
}

fn archives_update(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = need_str(payload, "id")?;
    let name = need_str(payload, "name")?;
    let platform = opt_str(payload, "platform").unwrap_or("");
    let goal = opt_str(payload, "goal").unwrap_or("");
    let domain = opt_str(payload, "domain").unwrap_or("");
    let audience = opt_str(payload, "audience").unwrap_or("");
    let tone_tags = opt_str_array(payload, "toneTags");
    if is_dry_run(payload) {
        return Ok(dry_run_preview(json!({ "id": id, "updated": true })));
    }
    let now = now_ts();
    let created_at = db
        .query_one_json(
            "SELECT created_at FROM archive_profiles WHERE id = ?",
            &[json!(id)],
        )?
        .and_then(|r| r.get("created_at").and_then(|v| v.as_i64()))
        .unwrap_or(now);
    db.execute_json(
        "UPDATE archive_profiles \
         SET name = ?, platform = ?, goal = ?, domain = ?, audience = ?, tone_tags = ?, updated_at = ? \
         WHERE id = ?",
        &[
            json!(name),
            json!(platform),
            json!(goal),
            json!(domain),
            json!(audience),
            json!(serde_json::to_string(&tone_tags)?),
            json!(now),
            json!(id),
        ],
    )?;
    Ok(json!({
        "id": id,
        "name": name,
        "platform": platform,
        "goal": goal,
        "domain": domain,
        "audience": audience,
        "tone_tags": tone_tags,
        "created_at": created_at,
        "updated_at": now,
    }))
}

fn archives_delete(db: &Db, id: &str) -> anyhow::Result<Value> {
    if is_dry_run_from(id).is_none() {
        // 非 dry 场景：正常删除（ CASCADE 会带走 samples）。
    }
    db.execute_json("DELETE FROM archive_profiles WHERE id = ?", &[json!(id)])?;
    Ok(json!({ "success": true }))
}

fn archive_samples_list(db: &Db, profile_id: &str) -> anyhow::Result<Vec<Value>> {
    let rows = db.query_all_json(
        "SELECT id, profile_id, title, content, excerpt, tags, images, platform, source_url, \
         sample_date, is_featured, created_at \
         FROM archive_samples WHERE profile_id = ? ORDER BY created_at DESC",
        &[json!(profile_id)],
    )?;
    Ok(rows.into_iter().map(decorate_sample).collect())
}

fn archive_sample_create(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let profile_id = need_str(payload, "profileId")?;
    let title = opt_str(payload, "title").unwrap_or("");
    let content = opt_str(payload, "content").unwrap_or("");
    let platform = opt_str(payload, "platform").unwrap_or("");
    let source_url = opt_str(payload, "sourceUrl").unwrap_or("");
    let sample_date = opt_str(payload, "sampleDate")
        .map(String::from)
        .unwrap_or_else(today_date_string);
    let is_featured = payload
        .get("isFeatured")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let tags = {
        let t = opt_str_array(payload, "tags");
        if t.is_empty() {
            extract_tags_from_text(title, content)
        } else {
            t
        }
    };
    let excerpt = build_excerpt(content);
    if is_dry_run(payload) {
        return Ok(dry_run_preview(json!({
            "profile_id": profile_id,
            "title": title,
            "excerpt": excerpt,
        })));
    }
    let now = now_ts();
    let id = gen_id("sample");
    db.execute_json(
        "INSERT INTO archive_samples \
         (id, profile_id, title, content, excerpt, tags, images, platform, source_url, sample_date, is_featured, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        &[
            json!(id),
            json!(profile_id),
            json!(title),
            json!(content),
            json!(excerpt),
            json!(serde_json::to_string(&tags)?),
            json!("[]"),
            json!(platform),
            json!(source_url),
            json!(sample_date),
            json!(if is_featured { 1 } else { 0 }),
            json!(now),
        ],
    )?;
    Ok(json!({
        "id": id,
        "profile_id": profile_id,
        "title": title,
        "content": content,
        "excerpt": excerpt,
        "tags": tags,
        "images": [],
        "platform": platform,
        "source_url": source_url,
        "sample_date": sample_date,
        "is_featured": if is_featured { 1 } else { 0 },
        "created_at": now,
    }))
}

fn archive_sample_update(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let id = need_str(payload, "id")?;
    let profile_id = need_str(payload, "profileId")?;
    let title = opt_str(payload, "title").unwrap_or("");
    let content = opt_str(payload, "content").unwrap_or("");
    let platform = opt_str(payload, "platform").unwrap_or("");
    let source_url = opt_str(payload, "sourceUrl").unwrap_or("");
    let sample_date = opt_str(payload, "sampleDate")
        .map(String::from)
        .unwrap_or_else(today_date_string);
    let is_featured = payload
        .get("isFeatured")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let tags = {
        let t = opt_str_array(payload, "tags");
        if t.is_empty() {
            extract_tags_from_text(title, content)
        } else {
            t
        }
    };
    let excerpt = build_excerpt(content);
    // 保留已有 images（读旧值）。
    let images = archive_samples_list(db, profile_id)?
        .into_iter()
        .find(|s| s.get("id").and_then(|v| v.as_str()) == Some(id))
        .and_then(|s| s.get("images").cloned())
        .unwrap_or_else(|| json!([]));
    if is_dry_run(payload) {
        return Ok(dry_run_preview(json!({ "id": id, "updated": true })));
    }
    db.execute_json(
        "UPDATE archive_samples \
         SET title = ?, content = ?, excerpt = ?, tags = ?, images = ?, platform = ?, \
         source_url = ?, sample_date = ?, is_featured = ? WHERE id = ?",
        &[
            json!(title),
            json!(content),
            json!(excerpt),
            json!(serde_json::to_string(&tags)?),
            json!(images_to_string(&images)?),
            json!(platform),
            json!(source_url),
            json!(sample_date),
            json!(if is_featured { 1 } else { 0 }),
            json!(id),
        ],
    )?;
    let created_at = db
        .query_one_json(
            "SELECT created_at FROM archive_samples WHERE id = ?",
            &[json!(id)],
        )?
        .and_then(|r| r.get("created_at").and_then(|v| v.as_i64()))
        .unwrap_or_else(now_ts);
    Ok(json!({
        "id": id,
        "profile_id": profile_id,
        "title": title,
        "content": content,
        "excerpt": excerpt,
        "tags": tags,
        "images": images,
        "platform": platform,
        "source_url": source_url,
        "sample_date": sample_date,
        "is_featured": if is_featured { 1 } else { 0 },
        "created_at": created_at,
    }))
}

fn archive_sample_delete(db: &Db, id: &str) -> anyhow::Result<Value> {
    db.execute_json("DELETE FROM archive_samples WHERE id = ?", &[json!(id)])?;
    Ok(json!({ "success": true }))
}

// =============================================================================
// manuscripts（fs）
// =============================================================================

async fn manuscripts_dispatch(
    channel: &str,
    payload: &Value,
    root: &Path,
    state: &AppState,
) -> anyhow::Result<Value> {
    match channel {
        "manuscripts:list" => {
            ensure_dir(root).await;
            Ok(json!(build_tree(root, root).await))
        }
        "manuscripts:read" => {
            let rel = extract_id(payload, &["filePath", "path", "id"])
                .ok_or_else(|| anyhow::anyhow!("缺少字段 filePath"))?;
            manuscripts_read(root, &rel).await
        }
        "manuscripts:save" => {
            let rel = need_str(payload, "path")?.to_string();
            let content = opt_str(payload, "content").unwrap_or("").to_string();
            let metadata = payload.get("metadata").cloned().unwrap_or(json!({}));
            let dry = is_dry_run(payload);
            let res = manuscripts_save(root, &rel, &content, &metadata, dry).await?;
            if !dry {
                emit_data_changed(state, "manuscripts", "save", Some(&rel));
            }
            Ok(res)
        }
        "manuscripts:delete" => {
            let rel = extract_id(payload, &["filePath", "path", "id"])
                .ok_or_else(|| anyhow::anyhow!("缺少字段 filePath"))?;
            let dry = is_dry_run(payload);
            let res = manuscripts_delete(root, &rel, dry).await?;
            if !dry {
                emit_data_changed(state, "manuscripts", "delete", Some(&rel));
            }
            Ok(res)
        }
        "manuscripts:rename" => {
            let old = need_str(payload, "oldPath")?.to_string();
            let new_name = need_str(payload, "newName")?.to_string();
            let dry = is_dry_run(payload);
            manuscripts_rename(root, &old, &new_name, dry).await
        }
        "manuscripts:move" => {
            let source = need_str(payload, "sourcePath")?.to_string();
            let target_dir = need_str(payload, "targetDir")?.to_string();
            let dry = is_dry_run(payload);
            manuscripts_move(root, &source, &target_dir, dry).await
        }
        "manuscripts:create-folder" => {
            let parent = opt_str(payload, "parentPath").unwrap_or("").to_string();
            let name = need_str(payload, "name")?.to_string();
            let dry = is_dry_run(payload);
            manuscripts_create_folder(root, &parent, &name, dry).await
        }
        "manuscripts:create-file" => {
            let parent = opt_str(payload, "parentPath").unwrap_or("").to_string();
            let name = need_str(payload, "name")?.to_string();
            let content = opt_str(payload, "content").unwrap_or("").to_string();
            let _title = opt_str(payload, "title").unwrap_or("").to_string();
            let dry = is_dry_run(payload);
            manuscripts_create_file(root, &parent, &name, &content, dry).await
        }
        "manuscripts:get-layout" => manuscripts_get_layout(root).await,
        "manuscripts:save-layout" => {
            let layout = payload
                .get("layout")
                .filter(|v| v.is_object())
                .unwrap_or(payload)
                .clone();
            let dry = is_dry_run(payload);
            manuscripts_save_layout(root, &layout, dry).await
        }
        "manuscripts:format-wechat" => {
            let content = opt_str(payload, "content").unwrap_or("");
            if content.trim().is_empty() {
                return Ok(json!({ "success": false, "error": "content is required" }));
            }
            // 占位：返回原文（待接入微信排版引擎）。
            Ok(json!({
                "success": true,
                "content": content,
                "html": content,
                "title": opt_str(payload, "title").unwrap_or("")
            }))
        }
        "manuscripts:get-package-state" => {
            let rel = extract_id(payload, &["filePath", "path", "id"])
                .ok_or_else(|| anyhow::anyhow!("缺少字段 filePath"))?;
            manuscripts_get_package_state(root, &rel).await
        }
        _ => Err(anyhow::anyhow!("manuscripts 通道未实现: {channel}")),
    }
}

async fn manuscripts_read(root: &Path, rel: &str) -> anyhow::Result<Value> {
    let full = resolve_within(root, rel)?;
    match tokio::fs::metadata(&full).await {
        Ok(meta) if meta.is_dir() => {
            // 稿件包：尝试读 script.md。
            let script = full.join("script.md");
            match tokio::fs::read_to_string(&script).await {
                Ok(raw) => {
                    let (metadata, content) = parse_frontmatter(&raw);
                    Ok(json!({ "content": content, "metadata": metadata }))
                }
                Err(_) => Ok(json!({ "content": "", "metadata": {} })),
            }
        }
        Ok(_) => {
            let raw = tokio::fs::read_to_string(&full).await.unwrap_or_default();
            let (metadata, content) = parse_frontmatter(&raw);
            Ok(json!({ "content": content, "metadata": metadata }))
        }
        Err(_) => Ok(json!({ "content": "", "metadata": {} })),
    }
}

async fn manuscripts_save(
    root: &Path,
    rel: &str,
    content: &str,
    metadata: &Value,
    dry: bool,
) -> anyhow::Result<Value> {
    let full = resolve_within(root, rel)?;
    if dry {
        return Ok(dry_run_preview(json!({ "path": rel })));
    }
    if let Some(parent) = full.parent() {
        tokio::fs::create_dir_all(parent).await.ok();
    }
    let mut meta = metadata.clone();
    if let Some(obj) = meta.as_object_mut() {
        let now = now_ts();
        obj.entry("updatedAt").or_insert(json!(now));
        obj.entry("createdAt").or_insert(json!(now));
    }
    let body = stringify_frontmatter(content, &meta);
    tokio::fs::write(&full, body).await?;
    Ok(json!({ "success": true }))
}

async fn manuscripts_create_folder(
    root: &Path,
    parent: &str,
    name: &str,
    dry: bool,
) -> anyhow::Result<Value> {
    let rel = join_rel(parent, name);
    let full = resolve_within(root, &rel)?;
    if dry {
        return Ok(dry_run_preview(json!({ "path": rel })));
    }
    tokio::fs::create_dir_all(&full).await?;
    Ok(json!({ "success": true, "path": rel }))
}

async fn manuscripts_create_file(
    root: &Path,
    parent: &str,
    name: &str,
    content: &str,
    dry: bool,
) -> anyhow::Result<Value> {
    let file_name = ensure_manuscript_file_name(name);
    let rel = join_rel(parent, &file_name);
    let full = resolve_within(root, &rel)?;
    if dry {
        return Ok(dry_run_preview(json!({ "path": rel })));
    }
    if tokio::fs::metadata(&full).await.is_ok() {
        return Ok(json!({ "success": false, "error": "文件已存在" }));
    }
    if let Some(p) = full.parent() {
        tokio::fs::create_dir_all(p).await.ok();
    }
    if is_manuscript_package_name(&file_name) {
        // 稿件包：建目录 + script.md。
        tokio::fs::create_dir_all(&full).await?;
        tokio::fs::write(full.join("script.md"), content).await?;
    } else {
        tokio::fs::write(&full, content).await?;
    }
    Ok(json!({ "success": true, "path": rel }))
}

async fn manuscripts_delete(root: &Path, rel: &str, dry: bool) -> anyhow::Result<Value> {
    let full = resolve_within(root, rel)?;
    if dry {
        return Ok(dry_run_preview(json!({ "path": rel })));
    }
    match tokio::fs::metadata(&full).await {
        Ok(m) if m.is_dir() => {
            tokio::fs::remove_dir_all(&full).await?;
        }
        Ok(_) => {
            tokio::fs::remove_file(&full).await?;
        }
        Err(_) => {}
    }
    Ok(json!({ "success": true }))
}

async fn manuscripts_rename(
    root: &Path,
    old_rel: &str,
    new_name: &str,
    dry: bool,
) -> anyhow::Result<Value> {
    let old_full = resolve_within(root, old_rel)?;
    let parent = old_full
        .parent()
        .ok_or_else(|| anyhow::anyhow!("无效路径"))?;
    let new_full = resolve_within(root, &join_rel(&rel_of(root, parent), new_name))?;
    if dry {
        return Ok(dry_run_preview(
            json!({ "oldPath": old_rel, "newName": new_name }),
        ));
    }
    tokio::fs::rename(&old_full, &new_full).await?;
    let new_rel = rel_of(root, &new_full);
    Ok(json!({ "success": true, "newPath": new_rel }))
}

async fn manuscripts_move(
    root: &Path,
    source_rel: &str,
    target_dir: &str,
    dry: bool,
) -> anyhow::Result<Value> {
    let source_full = resolve_within(root, source_rel)?;
    let file_name = source_full
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("无效源路径"))?
        .to_string();
    let target_rel = join_rel(target_dir, &file_name);
    let target_full = resolve_within(root, &target_rel)?;
    if dry {
        return Ok(dry_run_preview(
            json!({ "sourcePath": source_rel, "targetDir": target_dir }),
        ));
    }
    if let Some(p) = target_full.parent() {
        tokio::fs::create_dir_all(p).await.ok();
    }
    tokio::fs::rename(&source_full, &target_full).await?;
    Ok(json!({ "success": true, "newPath": rel_of(root, &target_full) }))
}

async fn manuscripts_get_layout(root: &Path) -> anyhow::Result<Value> {
    ensure_dir(root).await;
    let layout_path = root.join("layout.json");
    match tokio::fs::read_to_string(&layout_path).await {
        Ok(raw) => Ok(serde_json::from_str(&raw).unwrap_or(json!({}))),
        Err(_) => Ok(json!({})),
    }
}

async fn manuscripts_save_layout(root: &Path, layout: &Value, dry: bool) -> anyhow::Result<Value> {
    if dry {
        return Ok(dry_run_preview(json!({ "layout": layout })));
    }
    ensure_dir(root).await;
    let layout_path = root.join("layout.json");
    tokio::fs::write(&layout_path, serde_json::to_string_pretty(layout)?).await?;
    Ok(json!({ "success": true }))
}

async fn manuscripts_get_package_state(root: &Path, rel: &str) -> anyhow::Result<Value> {
    let full = resolve_within(root, rel)?;
    match tokio::fs::metadata(&full).await {
        Ok(m)
            if m.is_dir()
                && full
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(is_manuscript_package_name)
                    .unwrap_or(false) =>
        {
            let manifest = tokio::fs::read_to_string(full.join("manifest.json"))
                .await
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .unwrap_or(json!({}));
            Ok(json!({ "success": true, "state": { "path": rel, "manifest": manifest } }))
        }
        Ok(_) => Ok(json!({ "success": false, "error": "Not a manuscript package" })),
        Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
    }
}

/// 递归构建稿件文件树（目录优先、字母序）。
async fn build_tree(root: &Path, dir: &Path) -> Vec<Value> {
    let mut read = match tokio::fs::read_dir(dir).await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut entries: Vec<(String, bool)> = Vec::new();
    while let Ok(Some(e)) = read.next_entry().await {
        let name = e.file_name().to_string_lossy().to_string();
        let is_dir = e.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        entries.push((name, is_dir));
    }
    entries.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(&b.0),
    });
    let mut out = Vec::new();
    for (name, is_dir) in entries {
        let full = dir.join(&name);
        let rel = rel_of(root, &full);
        if is_dir {
            let children = Box::pin(build_tree(root, &full)).await;
            out.push(json!({
                "name": name,
                "path": rel,
                "isDirectory": true,
                "children": children,
            }));
        } else if is_supported_manuscript_file(&name) {
            out.push(json!({
                "name": name,
                "path": rel,
                "isDirectory": false,
                "draftType": draft_type_from_name(&name),
                "title": null,
                "status": null,
            }));
        }
    }
    out
}

async fn ensure_dir(path: &Path) {
    let _ = tokio::fs::create_dir_all(path).await;
}

// =============================================================================
// frontmatter（简化版）
// =============================================================================

/// 解析 `---\n<yaml>\n---\n<body>`，返回 (metadata, body)。无 frontmatter 时 metadata 为 `{}`。
fn parse_frontmatter(raw: &str) -> (Value, String) {
    let lines: Vec<&str> = raw.split('\n').collect();
    if lines.is_empty() || lines[0].trim() != "---" {
        return (json!({}), raw.to_string());
    }
    let end = (1..lines.len()).find(|&i| {
        let t = lines[i].trim();
        t == "---" || t == "..."
    });
    let end = match end {
        Some(e) => e,
        None => return (json!({}), raw.to_string()),
    };
    let mut obj = serde_json::Map::new();
    for line in &lines[1..end] {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            if key.is_empty() {
                continue;
            }
            obj.insert(key, parse_scalar(v.trim()));
        }
    }
    let body = lines[end + 1..]
        .join("\n")
        .trim_start_matches('\n')
        .to_string();
    (Value::Object(obj), body)
}

fn parse_scalar(s: &str) -> Value {
    if s.is_empty() {
        return Value::Null;
    }
    if let Ok(n) = s.parse::<i64>() {
        return json!(n);
    }
    if let Ok(f) = s.parse::<f64>() {
        return json!(f);
    }
    if s == "true" {
        return json!(true);
    }
    if s == "false" {
        return json!(false);
    }
    if s == "null" {
        return Value::Null;
    }
    if s.starts_with('[') || s.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            return v;
        }
    }
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return Value::String(s[1..s.len() - 1].to_string());
    }
    Value::String(s.to_string())
}

/// 把 (body, metadata) 组装为带 frontmatter 的字符串。
fn stringify_frontmatter(body: &str, metadata: &Value) -> String {
    let mut out = String::from("---\n");
    if let Some(obj) = metadata.as_object() {
        for (k, v) in obj {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(&scalar_to_yaml(v));
            out.push('\n');
        }
    }
    out.push_str("---\n\n");
    out.push_str(body);
    out
}

fn scalar_to_yaml(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if s.is_empty()
                || s.contains(':')
                || s.contains('\n')
                || s.contains('#')
                || s.starts_with(' ')
                || s.starts_with('[')
                || s.starts_with('{')
            {
                format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                s.clone()
            }
        }
        other => other.to_string(),
    }
}

// =============================================================================
// 通用助手
// =============================================================================

const SUPPORTED_EXTENSIONS: &[&str] = &[".md", ".redarticle", ".redpost", ".redvideo", ".redaudio"];
const PACKAGE_EXTENSIONS: &[&str] = &[".redarticle", ".redpost", ".redvideo", ".redaudio"];

fn is_supported_manuscript_file(name: &str) -> bool {
    SUPPORTED_EXTENSIONS.iter().any(|e| name.ends_with(e))
}

fn is_manuscript_package_name(name: &str) -> bool {
    PACKAGE_EXTENSIONS.iter().any(|e| name.ends_with(e))
}

fn draft_type_from_name(name: &str) -> &'static str {
    if name.ends_with(".redarticle") {
        "longform"
    } else if name.ends_with(".redpost") {
        "richpost"
    } else if name.ends_with(".redvideo") {
        "video"
    } else if name.ends_with(".redaudio") {
        "audio"
    } else {
        "unknown"
    }
}

fn ensure_manuscript_file_name(name: &str) -> String {
    if is_supported_manuscript_file(name) {
        name.to_string()
    } else {
        format!("{name}.md")
    }
}

/// manuscripts 根目录与 Beav `getWorkspacePaths().manuscripts` 保持一致。
fn manuscripts_root(settings: &Value) -> PathBuf {
    crate::workspace::resolve(settings).manuscripts
}

fn active_space_id(settings: &Value) -> String {
    settings
        .get("active_space_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("default")
        .to_string()
}

/// 解析 `user_memories`/`archive_*` 行，把 `tags`/`tone_tags` JSON 字符串还原为数组。
fn decorate_memory(mut row: Value) -> Value {
    if let Some(map) = row.as_object_mut() {
        let tags = parse_array_field(map, "tags");
        map.insert("tags".into(), json!(tags));
    }
    row
}

fn decorate_profile(mut row: Value) -> Value {
    if let Some(map) = row.as_object_mut() {
        let tags = parse_array_field(map, "tone_tags");
        map.insert("tone_tags".into(), json!(tags));
    }
    row
}

fn decorate_sample(mut row: Value) -> Value {
    if let Some(map) = row.as_object_mut() {
        let tags = parse_array_field(map, "tags");
        let images = parse_array_field(map, "images");
        let featured = map
            .get("is_featured")
            .and_then(|v| v.as_i64())
            .map(|i| if i != 0 { 1 } else { 0 })
            .unwrap_or(0);
        map.insert("tags".into(), json!(tags));
        map.insert("images".into(), json!(images));
        map.insert("is_featured".into(), json!(featured));
    }
    row
}

fn parse_array_field(map: &serde_json::Map<String, Value>, col: &str) -> Vec<String> {
    match map.get(col) {
        Some(Value::String(s)) => serde_json::from_str::<Vec<String>>(s).unwrap_or_default(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => Vec::new(),
    }
}

fn normalize_type(t: &str) -> &str {
    match t {
        "preference" | "fact" => t,
        _ => "general",
    }
}

/// 从文本抽取前若干 `#标签`/关键词作为 tags（移植 `extractTagsFromText` 的简化版）。
fn extract_tags_from_text(title: &str, content: &str) -> Vec<String> {
    let mut set: Vec<String> = Vec::new();
    for text in [title, content] {
        for tok in text.split(|c: char| c.is_whitespace() || c == '\n') {
            let t = tok.trim();
            if let Some(rest) = t.strip_prefix('#') {
                let tag = rest.trim_matches(|c: char| !c.is_alphanumeric() && c != '-');
                if !tag.is_empty() && !set.iter().any(|x| x == tag) {
                    set.push(tag.to_string());
                }
            }
            if set.len() >= 6 {
                return set;
            }
        }
    }
    set
}

fn build_excerpt(content: &str) -> String {
    content
        .replace('\n', " ")
        .chars()
        .take(140)
        .collect::<String>()
        .trim()
        .to_string()
}

fn images_to_string(images: &Value) -> anyhow::Result<String> {
    match images {
        Value::Array(_) => Ok(serde_json::to_string(images)?),
        Value::String(s) => Ok(s.clone()),
        _ => Ok("[]".into()),
    }
}

/// 安全拼接：把 `rel` 限定在 `root` 之内，拒绝绝对路径/越界 `..`。
fn resolve_within(root: &Path, rel: &str) -> anyhow::Result<PathBuf> {
    if rel.contains('\0') {
        anyhow::bail!("非法路径: {rel}");
    }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        anyhow::bail!("非法路径（绝对路径）: {rel}");
    }
    let joined = root.join(rel_path);
    let mut norm = PathBuf::new();
    for comp in joined.components() {
        use std::path::Component;
        match comp {
            Component::ParentDir => {
                norm.pop();
            }
            Component::CurDir => {}
            other => norm.push(other.as_os_str()),
        }
    }
    if !norm.starts_with(root) {
        anyhow::bail!("路径越界: {rel}");
    }
    Ok(norm)
}

fn join_rel(parent: &str, name: &str) -> String {
    let parent = parent.trim_start_matches('/').trim_end_matches('/');
    if parent.is_empty() {
        name.trim_start_matches('/').to_string()
    } else {
        format!("{parent}/{}", name.trim_start_matches('/'))
    }
}

/// `full` 相对 `root` 的 posix 风格路径。
fn rel_of(root: &Path, full: &Path) -> String {
    full.strip_prefix(root)
        .unwrap_or(full)
        .to_string_lossy()
        .replace('\\', "/")
}

fn opt_str<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(|v| v.as_str())
}

fn need_str<'a>(payload: &'a Value, key: &str) -> anyhow::Result<&'a str> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("缺少字段 {key}"))
}

fn opt_str_array(payload: &Value, key: &str) -> Vec<String> {
    match payload.get(key) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn opt_str_array_owned(v: &Value) -> Vec<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        Value::String(s) => serde_json::from_str(s).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// 取 ID：payload 可能是裸字符串，或对象里的某个 key。
fn extract_id(payload: &Value, keys: &[&str]) -> Option<String> {
    if let Some(s) = payload.as_str() {
        return Some(s.to_string());
    }
    for k in keys {
        if let Some(s) = payload.get(*k).and_then(|v| v.as_str()) {
            return Some(s.to_string());
        }
    }
    None
}

fn is_dry_run(payload: &Value) -> bool {
    payload
        .get("dryRun")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 占位：`is_dry_run_from` 仅用于消除 `archives_delete` 中未使用变量告警的语义占位。
fn is_dry_run_from(_id: &str) -> Option<()> {
    None
}

fn dry_run_preview(payload: Value) -> Value {
    let mut p = payload;
    if let Some(obj) = p.as_object_mut() {
        obj.insert("success".into(), json!(true));
        obj.insert("dryRun".into(), json!(true));
        obj.insert("skipped".into(), json!(true));
    } else {
        p = json!({ "success": true, "dryRun": true, "skipped": true, "data": p });
    }
    p
}

fn emit_data_changed(state: &AppState, scope: &str, action: &str, entity_id: Option<&str>) {
    let mut payload = serde_json::Map::new();
    payload.insert("scope".into(), json!(scope));
    payload.insert("action".into(), json!(action));
    if let Some(id) = entity_id {
        payload.insert("entityId".into(), json!(id));
    }
    state.emitter.emit("data:changed", Value::Object(payload));
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn next_seq() -> u64 {
    SEQ.fetch_add(1, Ordering::Relaxed)
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn gen_id(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{}_{}_{:x}{:x}", prefix, now_ts(), nanos, next_seq())
}

/// `YYYY-MM-DD`（不依赖 chrono clock feature）。
fn today_date_string() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let days = secs.div_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// days since 1970-01-01 → (year, month, day)。Howard Hinnant 算法。
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// =============================================================================
// tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let space = "default";
        // add
        let m1 = memory_add(&db, space, "用户喜欢深色主题", "preference", &["ui".into()]).unwrap();
        let id1 = m1["id"].as_str().unwrap().to_string();
        memory_add(&db, space, "项目用 Rust 写", "fact", &[]).unwrap();

        // list（按 created_at DESC，顺序不固定，按 id 定位 m1）
        let list = memory_list(&db, space).unwrap();
        assert_eq!(list.len(), 2);
        let m1_row = list
            .iter()
            .find(|m| m["id"].as_str() == Some(id1.as_str()))
            .unwrap();
        // tags 应被还原为数组（含 ui）
        assert!(m1_row["tags"]
            .as_array()
            .is_some_and(|a| a.iter().any(|t| t == "ui")));

        // update
        memory_update(
            &db,
            space,
            &id1,
            Some("用户喜欢浅色主题"),
            None,
            Some(&["ui".into(), "theme".into()]),
        )
        .unwrap();
        let after = memory_list(&db, space)
            .unwrap()
            .into_iter()
            .find(|m| m["id"].as_str() == Some(id1.as_str()))
            .unwrap();
        assert_eq!(after["content"], "用户喜欢浅色主题");

        // search
        let hits = memory_search(&db, space, "rust", false, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["content"], "项目用 Rust 写");

        // delete
        memory_delete(&db, space, &id1).unwrap();
        assert_eq!(memory_list(&db, space).unwrap().len(), 1);
    }

    #[test]
    fn archive_sample_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let profile = archives_create(
            &db,
            &json!({ "name": "小红书账号", "platform": "xiaohongshu", "toneTags": ["种草"] }),
        )
        .unwrap();
        let pid = profile["id"].as_str().unwrap().to_string();
        assert_eq!(profile["tone_tags"], json!(["种草"]));

        // sample create（无显式 tags → 从 # 标签抽取）
        let sample = archive_sample_create(
            &db,
            &json!({
                "profileId": pid,
                "title": "爆款笔记",
                "content": "正文 #种草 #好物",
            }),
        )
        .unwrap();
        let sid = sample["id"].as_str().unwrap().to_string();
        let tags = sample["tags"].as_array().unwrap();
        assert!(tags.iter().any(|t| t.as_str() == Some("种草")));

        // list
        let samples = archive_samples_list(&db, &pid).unwrap();
        assert_eq!(samples.len(), 1);

        // update
        archive_sample_update(
            &db,
            &json!({
                "id": sid,
                "profileId": pid,
                "title": "改过",
                "content": "新内容",
                "isFeatured": true,
            }),
        )
        .unwrap();
        let updated = archive_samples_list(&db, &pid).unwrap()[0].clone();
        assert_eq!(updated["title"], "改过");
        assert_eq!(updated["is_featured"], 1);

        // delete
        archive_sample_delete(&db, &sid).unwrap();
        assert!(archive_samples_list(&db, &pid).unwrap().is_empty());
    }

    #[tokio::test]
    async fn manuscripts_save_read_list() {
        let root = unique_temp_dir();
        // save
        let saved = manuscripts_save(
            &root,
            "notes/a.md",
            "正文内容",
            &json!({ "title": "A" }),
            false,
        )
        .await
        .unwrap();
        assert_eq!(saved["success"], json!(true));

        // read 回来：content 与 metadata 往返一致
        let read = manuscripts_read(&root, "notes/a.md").await.unwrap();
        assert_eq!(read["content"], "正文内容");
        assert_eq!(read["metadata"]["title"], "A");

        // list 含该文件节点
        let tree = build_tree(&root, &root).await;
        let names: Vec<&str> = tree
            .iter()
            .filter_map(|n| {
                if n["isDirectory"] == true {
                    None
                } else {
                    n["name"].as_str()
                }
            })
            .collect();
        // notes/a.md 在子目录，顶层应出现 notes 目录
        assert!(tree
            .iter()
            .any(|n| n["name"] == "notes" && n["isDirectory"] == true));
        let _ = names;

        // dryRun 不落盘
        let before = manuscripts_save(&root, "dry.md", "x", &json!({}), true)
            .await
            .unwrap();
        assert_eq!(before["dryRun"], json!(true));
        assert!(manuscripts_read(&root, "dry.md").await.unwrap()["content"].as_str() == Some(""));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn frontmatter_roundtrip() {
        let body = "hello world";
        let meta = json!({ "title": "T", "count": 5i64, "flag": true });
        let s = stringify_frontmatter(body, &meta);
        assert!(s.starts_with("---\n"));
        let (m2, b2) = parse_frontmatter(&s);
        assert_eq!(b2, body);
        assert_eq!(m2["title"], "T");
        assert_eq!(m2["count"], 5);
        assert_eq!(m2["flag"], true);
    }

    #[test]
    fn resolve_within_rejects_traversal() {
        let root = unique_temp_dir();
        assert!(resolve_within(&root, "a/b.md").is_ok());
        assert!(resolve_within(&root, "../escape.md").is_err());
        assert!(resolve_within(&root, "/etc/passwd").is_err());
        assert!(resolve_within(&root, "a/../../escape.md").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    fn unique_temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("yy_content_test_{}", gen_id("d")));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
