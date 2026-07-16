//! Knowledge 命名空间 IPC 通道（替代 Beav `ipcMain.handle('knowledge:*')`）。
//!
//! 覆盖三类知识来源：
//! - 小红书笔记 / 视频笔记（`knowledge_vectors` 元数据）；
//! - YouTube 视频笔记（原 Electron 为文件系统存储，本实现返回空/graceful，待 workspace root）；
//! - 文档知识源（`document_knowledge_index`，登记/删除/列表）。
//!
//! 索引进度统计走 `file_index_lanes`（done/total/failed）。
//!
//! 由 [`super::dispatch_invoke`] 按前缀 `knowledge` 路由到 [`invoke`]。
//! 写操作（add-files / add-folder / delete / rebuild-catalog）默认 dry-run，
//! payload 提供 `dryRun:false` 或 `confirm:true` 时才真正落库（见 [`should_execute`]）。
//!
//! 真实转录（STT）与向量/embedding 计算依赖外部服务，此处只做 accept + 事件推送，
//! 标注 `TODO`（见 [`run_transcribe_background`] / [`run_retry_youtube_subtitle_background`]）。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use super::AppState;
use super::EventEmitter;
use crate::db::Db;

/// 双向通道分发。按 channel 全名 match；未知通道返回 `Err`。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    // knowledge_vectors 表尚未在 Rust schema 中（document_knowledge_index / file_index_lanes 已有），
    // 这里幂等建表，保证 handler 与单测可直接 SQL。生产环境若 schema 已建则跳过。
    ensure_knowledge_vectors_table(&state.db)?;

    match channel {
        "knowledge:list" => list_impl(&state.db),
        "knowledge:list-youtube" => Ok(list_youtube_impl()),
        "knowledge:docs:list" => docs_list_impl(&state.db),
        "knowledge:list-page" => list_page_impl(&state.db, &payload),
        "knowledge:get-item-detail" => get_item_detail_impl(&state.db, &payload),
        "knowledge:get-index-status" => get_index_status_impl(&state.db),
        "knowledge:get-file-index-dashboard" => get_file_index_dashboard_impl(&state.db),
        "knowledge:read-youtube-subtitle" => Ok(read_youtube_subtitle_impl(&payload)),
        "knowledge:delete" => delete_impl(&state.db, &payload),

        // 写操作
        "knowledge:docs:add-files" => {
            let res = docs_add_files_impl(&state.db, &payload)?;
            if res
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                state.emitter.emit("knowledge:docs-updated", json!({}));
            }
            Ok(res)
        }
        "knowledge:docs:add-folder" => {
            let res = docs_add_folder_impl(&state.db, &payload).await?;
            if res
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                state.emitter.emit("knowledge:docs-updated", json!({}));
            }
            Ok(res)
        }
        "knowledge:docs:delete-source" => {
            let res = docs_delete_source_impl(&state.db, &payload)?;
            if res
                .get("success")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                state.emitter.emit("knowledge:docs-updated", json!({}));
            }
            Ok(res)
        }
        "knowledge:rebuild-catalog" => rebuild_catalog_impl(&state.db, &payload),
        "knowledge:open-index-root" => open_index_root_impl(&payload),

        // accept + 后台任务（emit knowledge:note-updated / youtube-video-updated）
        "knowledge:transcribe" => {
            let res = transcribe_impl(state.emitter.as_ref(), &payload)?;
            if should_execute(&payload) {
                if let Some(note_id) = note_id_of(&payload) {
                    tokio::spawn(run_transcribe_background(
                        state.db.clone(),
                        state.emitter.clone(),
                        note_id,
                    ));
                }
            }
            Ok(res)
        }
        "knowledge:retry-youtube-subtitle" => {
            let res = retry_youtube_subtitle_impl(state.emitter.as_ref(), &payload)?;
            if should_execute(&payload) {
                if let Some(vid) = youtube_id_of(&payload) {
                    tokio::spawn(run_retry_youtube_subtitle_background(
                        state.emitter.clone(),
                        vid,
                    ));
                }
            }
            Ok(res)
        }

        other => Err(anyhow::anyhow!("knowledge 通道未实现: {other}")),
    }
}

// ============================================================================
// 读取通道
// ============================================================================

/// `knowledge:list` —— 查 `knowledge_vectors` 去重 source_id，元数据来自 metadata JSON。
fn list_impl(db: &Db) -> anyhow::Result<Value> {
    let rows = db.query_all_json(
        "SELECT source_id, source_type, metadata, MAX(created_at) AS created_at \
         FROM knowledge_vectors GROUP BY source_id ORDER BY created_at DESC",
        &[],
    )?;
    let items: Vec<Value> = rows.iter().map(catalog_item_from_vector_row).collect();
    Ok(json!(items))
}

/// `knowledge:list-youtube` —— YouTube 笔记原为文件系统存储，无 DB 表；返回空（待 workspace root）。
fn list_youtube_impl() -> Value {
    // TODO: 接入 workspace 知识库根目录后，从 youtube/<id>/meta.json 读取。
    json!([])
}

/// `knowledge:docs:list` —— 按 source_id 聚合 `document_knowledge_index`，返回文档源视图。
fn docs_list_impl(db: &Db) -> anyhow::Result<Value> {
    let rows = db.query_all_json(
        "SELECT source_id, COUNT(*) AS file_count, MAX(updated_at) AS updated_at, \
         MAX(title) AS title FROM document_knowledge_index \
         GROUP BY source_id ORDER BY updated_at DESC",
        &[],
    )?;
    let items: Vec<Value> = rows.iter().map(document_source_view).collect();
    Ok(json!(items))
}

/// `knowledge:list-page` —— 分页返回全部目录条目（向量条目 + 文档源），支持 kind/query 过滤。
///
/// payload 兼容两种形态：`{cursor,limit,kind,query}` 或 `{payload:{...}}`（对齐原 renderer）。
fn list_page_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let inner = payload.get("payload").unwrap_or(payload);

    let query = inner
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    let kind = inner
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let limit = inner
        .get("limit")
        .and_then(|v| v.as_i64())
        .unwrap_or(200)
        .clamp(1, 500) as usize;
    let cursor = inner
        .get("cursor")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0) as usize;

    // 聚合两类来源
    let mut items: Vec<Value> = Vec::new();
    let vrows = db.query_all_json(
        "SELECT source_id, source_type, metadata, MAX(created_at) AS created_at \
         FROM knowledge_vectors GROUP BY source_id",
        &[],
    )?;
    for r in &vrows {
        items.push(catalog_item_from_vector_row(r));
    }
    let drows = db.query_all_json(
        "SELECT source_id, COUNT(*) AS file_count, MAX(updated_at) AS updated_at \
         FROM document_knowledge_index GROUP BY source_id",
        &[],
    )?;
    for r in &drows {
        items.push(document_source_view(r));
    }

    // 过滤
    let filtered: Vec<Value> = items
        .into_iter()
        .filter(|it| {
            if !kind.is_empty() && it.get("kind").and_then(|v| v.as_str()) != Some(kind.as_str()) {
                return false;
            }
            if query.is_empty() {
                return true;
            }
            let hay = serde_json::to_string(it).unwrap_or_default().to_lowercase();
            hay.contains(&query)
        })
        .collect();

    let total = filtered.len();
    let mut kind_counts: Map<String, Value> = Map::new();
    for it in &filtered {
        let k = it
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let entry = kind_counts.entry(k).or_insert(json!(0));
        if let Some(n) = entry.as_i64() {
            *entry = json!(n + 1);
        }
    }

    let page: Vec<Value> = filtered.into_iter().skip(cursor).take(limit).collect();
    let next_cursor = if cursor + limit < total {
        Some((cursor + limit).to_string())
    } else {
        None
    };

    Ok(json!({
        "items": page,
        "nextCursor": next_cursor,
        "total": total,
        "kindCounts": kind_counts,
    }))
}

/// `knowledge:get-item-detail` —— 按 kind 返回单条详情。
///
/// - `redbook-note` → `knowledge_vectors` 元数据；
/// - `document-source` → `document_knowledge_index` 汇总 + 文件清单；
/// - `youtube-video` → 文件系统存储，暂返回 null（待 workspace root）。
fn get_item_detail_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let inner = payload.get("payload").unwrap_or(payload);
    let item_id = inner
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let kind = inner
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if item_id.is_empty() || kind.is_empty() {
        return Ok(Value::Null);
    }

    match kind.as_str() {
        "redbook-note" => {
            let row = db.query_one_json(
                "SELECT source_id, source_type, metadata, MAX(created_at) AS created_at \
                 FROM knowledge_vectors WHERE source_id = ? GROUP BY source_id LIMIT 1",
                &[json!(item_id)],
            )?;
            Ok(row
                .as_ref()
                .map(catalog_item_from_vector_row)
                .unwrap_or(Value::Null))
        }
        "document-source" => {
            let summary = db.query_one_json(
                "SELECT source_id, COUNT(*) AS file_count, MAX(updated_at) AS updated_at \
                 FROM document_knowledge_index WHERE source_id = ? GROUP BY source_id",
                &[json!(item_id)],
            )?;
            let Some(summary) = summary else {
                return Ok(Value::Null);
            };
            let files = db.query_all_json(
                "SELECT absolute_path, relative_path, title, file_size, mtime_ms, updated_at \
                 FROM document_knowledge_index WHERE source_id = ? ORDER BY relative_path ASC",
                &[json!(item_id)],
            )?;
            let mut view = document_source_view(&summary);
            if let Some(obj) = view.as_object_mut() {
                obj.insert("files".into(), json!(files));
            }
            Ok(view)
        }
        "youtube-video" => {
            // TODO: 文件系统 youtube/<id>/meta.json
            Ok(Value::Null)
        }
        _ => Ok(Value::Null),
    }
}

/// `knowledge:get-index-status` —— 统计 `file_index_lanes` 的 done/total/failed/running。
fn get_index_status_impl(db: &Db) -> anyhow::Result<Value> {
    let row = db.query_one_json(
        "SELECT \
            COALESCE(SUM(done),0) AS done, \
            COALESCE(SUM(total),0) AS total, \
            COALESCE(SUM(failed),0) AS failed, \
            COALESCE(SUM(CASE WHEN status IN ('running','processing') THEN 1 ELSE 0 END),0) AS running, \
            MAX(last_updated_at) AS last_updated_at \
         FROM file_index_lanes",
        &[],
    )?;
    let row = row.unwrap_or_else(|| json!({ "done": 0, "total": 0, "failed": 0, "running": 0, "last_updated_at": Value::Null }));
    let done = row.get("done").and_then(|v| v.as_i64()).unwrap_or(0);
    let total = row.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
    let failed = row.get("failed").and_then(|v| v.as_i64()).unwrap_or(0);
    let running = row.get("running").and_then(|v| v.as_i64()).unwrap_or(0);
    let pending = (total - done - failed).max(0);
    Ok(json!({
        "indexedCount": done,
        "pendingCount": pending,
        "failedCount": failed,
        "lastIndexedAt": row.get("last_updated_at").cloned().unwrap_or(Value::Null),
        "isBuilding": running > 0,
        "lastError": Value::Null,
    }))
}

/// `knowledge:get-file-index-dashboard` —— 汇总 lanes + 按 scope_id 分组的进度。
fn get_file_index_dashboard_impl(db: &Db) -> anyhow::Result<Value> {
    let lanes = db.query_all_json("SELECT * FROM file_index_lanes", &[])?;

    let mut indexed: i64 = 0;
    let mut total_files: i64 = 0;
    let mut failed_files: i64 = 0;
    let mut running = false;
    let mut last_updated_at: Option<Value> = None;
    let mut scopes_map: BTreeMap<String, Vec<Value>> = BTreeMap::new();

    for lane in &lanes {
        let done = lane.get("done").and_then(|v| v.as_i64()).unwrap_or(0);
        let total = lane.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
        let failed = lane.get("failed").and_then(|v| v.as_i64()).unwrap_or(0);
        indexed += done;
        total_files += total;
        failed_files += failed;
        let status = lane.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status == "running" || status == "processing" {
            running = true;
        }
        if let Some(t) = lane.get("last_updated_at") {
            if !t.is_null() {
                last_updated_at = Some(t.clone());
            }
        }
        let scope_id = lane
            .get("scope_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        scopes_map
            .entry(scope_id)
            .or_default()
            .push(lane_view(lane));
    }

    let scopes: Vec<Value> = scopes_map
        .into_iter()
        .map(|(scope_id, scope_lanes)| {
            let file_count: i64 = scope_lanes
                .iter()
                .map(|l| l["total"].as_i64().unwrap_or(0))
                .sum();
            let failed_count: i64 = scope_lanes
                .iter()
                .map(|l| l["failed"].as_i64().unwrap_or(0))
                .sum();
            let is_indexing = scope_lanes.iter().any(|l| {
                l["status"].as_str() == Some("running")
                    || l["status"].as_str() == Some("processing")
            });
            json!({
                "scopeId": scope_id,
                "name": scope_id,
                "scopeType": "knowledge",
                "ownerId": "",
                "ownerName": "",
                "fileCount": file_count,
                "status": if is_indexing { "indexing" } else { "idle" },
                "failedCount": failed_count,
                "lanes": scope_lanes,
            })
        })
        .collect();

    Ok(json!({
        "overall": {
            "status": if running { "indexing" } else { "idle" },
            "indexedFiles": indexed,
            "totalFiles": total_files,
            "failedFiles": failed_files,
            "lastIndexedAt": last_updated_at,
        },
        "lanes": lanes.iter().map(lane_view).collect::<Vec<_>>(),
        "scopes": scopes,
    }))
}

/// `knowledge:read-youtube-subtitle` —— 字幕存于文件系统；无 workspace 时返回空（graceful）。
fn read_youtube_subtitle_impl(payload: &Value) -> Value {
    let _video_id = youtube_id_of(payload);
    // TODO: 读取 workspace youtube/<id>/<subtitleFile>。
    json!({ "success": true, "subtitleContent": "", "hasSubtitle": false })
}

// ============================================================================
// 写通道（默认 dry-run）
// ============================================================================

/// `knowledge:delete` —— 删除某笔记（按 source_id 清理 `knowledge_vectors`）。
fn delete_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let Some(note_id) = note_id_of(payload) else {
        return Ok(json!({ "success": false, "error": "缺少 noteId" }));
    };
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "noteId": note_id }));
    }
    let removed = db.execute_json(
        "DELETE FROM knowledge_vectors WHERE source_id = ?",
        &[json!(note_id)],
    )?;
    Ok(json!({ "success": true, "noteId": note_id, "removed": removed }))
}

/// `knowledge:docs:add-files` —— 把文件登记进 `document_knowledge_index`。
///
/// payload: `{ spaceId?, paths?: string[], files?: [string | { sourceId?, absolutePath|path, ... }] }`。
fn docs_add_files_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let space_id = payload
        .get("spaceId")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let files = payload
        .get("files")
        .or_else(|| payload.get("paths"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if files.is_empty() {
        return Ok(json!({ "success": true, "added": 0, "canceled": true }));
    }
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "added": files.len() }));
    }

    let now = now_ms();
    let mut added = 0usize;
    for f in &files {
        let string_path = f.as_str();
        let abs = string_path
            .or_else(|| f.get("absolutePath").and_then(|v| v.as_str()))
            .or_else(|| f.get("path").and_then(|v| v.as_str()))
            .unwrap_or("");
        let path = std::path::PathBuf::from(abs);
        if abs.is_empty() || (string_path.is_some() && !path.is_file()) {
            continue;
        }
        let source_id = f
            .get("sourceId")
            .or_else(|| f.get("source_id"))
            .and_then(|v| v.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("doc-file-{}", uuid::Uuid::new_v4().simple()));
        let rel = f
            .get("relativePath")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or(abs)
                    .to_string()
            });
        let title = f
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("未命名文件")
                    .to_string()
            });
        let metadata = std::fs::metadata(&path).ok();
        let size = f
            .get("fileSize")
            .and_then(|v| v.as_i64())
            .or_else(|| metadata.as_ref().map(|value| value.len() as i64))
            .unwrap_or(0);
        let mtime = f
            .get("mtimeMs")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                metadata
                    .as_ref()
                    .and_then(|value| value.modified().ok())
                    .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|value| value.as_millis() as i64)
            })
            .unwrap_or(0);
        db.execute_json(
            "INSERT OR REPLACE INTO document_knowledge_index \
             (space_id, source_id, absolute_path, relative_path, title, file_size, mtime_ms, updated_at) \
             VALUES (?,?,?,?,?,?,?,?)",
            &[
                json!(space_id),
                json!(source_id),
                json!(abs),
                json!(rel),
                json!(title),
                json!(size),
                json!(mtime),
                json!(now),
            ],
        )?;
        added += 1;
    }
    Ok(json!({ "success": true, "added": added }))
}

/// `knowledge:docs:add-folder` —— 扫描目录（顶层文件）登记为文档源。
///
/// payload: `{ spaceId?, sourceId?, path|folder }`。未提供 sourceId 时自动生成，递归登记文件。
async fn docs_add_folder_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let space_id = payload
        .get("spaceId")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let source_id = payload
        .get("sourceId")
        .or_else(|| payload.get("source_id"))
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("doc-folder-{}", uuid::Uuid::new_v4().simple()));
    let folder = payload
        .get("path")
        .or_else(|| payload.get("folder"))
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from);
    let Some(folder) = folder else {
        return Ok(json!({ "success": false, "error": "缺少 path" }));
    };
    if !should_execute(payload) {
        return Ok(
            json!({ "success": true, "dryRun": true, "path": folder.display().to_string() }),
        );
    }

    if !folder.is_dir() {
        return Ok(json!({ "success": false, "error": "所选路径不是文件夹" }));
    }
    let now = now_ms();
    let mut added = 0usize;
    let mut pending = vec![folder.clone()];
    while let Some(directory) = pending.pop() {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(error) if directory == folder => {
                return Ok(json!({ "success": false, "error": format!("无法读取目录: {error}") }));
            }
            Err(_) => continue,
        };
        while let Some(entry) = entries.next_entry().await? {
            let file_type = match entry.file_type().await {
                Ok(value) => value,
                Err(_) => continue,
            };
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let relative = path
                .strip_prefix(&folder)
                .map(|value| value.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| name.clone());
            let abs = path.display().to_string();
            let meta = tokio::fs::metadata(&path).await.ok();
            let size = meta.as_ref().map(|m| m.len() as i64).unwrap_or(0);
            let mtime = meta
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            db.execute_json(
                "INSERT OR REPLACE INTO document_knowledge_index \
                 (space_id, source_id, absolute_path, relative_path, title, file_size, mtime_ms, updated_at) \
                 VALUES (?,?,?,?,?,?,?,?)",
                &[
                    json!(space_id),
                    json!(source_id),
                    json!(abs),
                    json!(relative),
                    json!(name),
                    json!(size),
                    json!(mtime),
                    json!(now),
                ],
            )?;
            added += 1;
        }
    }
    Ok(json!({ "success": true, "added": added, "sourceId": source_id }))
}

/// `knowledge:docs:delete-source` —— 删除某文档源的全部登记。
fn docs_delete_source_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let source_id = payload
        .get("sourceId")
        .or_else(|| payload.get("source_id"))
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
        .or_else(|| payload.as_str())
        .unwrap_or("");
    if source_id.is_empty() {
        return Ok(json!({ "success": false, "error": "文档源不存在" }));
    }
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "sourceId": source_id }));
    }
    let removed = db.execute_json(
        "DELETE FROM document_knowledge_index WHERE source_id = ?",
        &[json!(source_id)],
    )?;
    Ok(json!({ "success": true, "sourceId": source_id, "removed": removed }))
}

/// `knowledge:rebuild-catalog` —— 重置 `file_index_lanes` 状态（status=idle, done=0, failed=0）。
fn rebuild_catalog_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true }));
    }
    let reset = db.execute_json(
        "UPDATE file_index_lanes SET status='idle', done=0, failed=0, last_updated_at=NULL",
        &[],
    )?;
    Ok(json!({ "success": true, "reset": reset }))
}

/// `knowledge:open-index-root` —— 用系统命令打开目录（macOS `open` / Windows `explorer` / Linux `xdg-open`）。
///
/// payload: `{ path, dryRun? }`。未提供 `path` 时返回错误（无 workspace 概念）。
fn open_index_root_impl(payload: &Value) -> anyhow::Result<Value> {
    let Some(path) = payload.get("path").and_then(|v| v.as_str()) else {
        return Ok(json!({ "success": false, "error": "未指定 index root（payload.path）" }));
    };
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "path": path }));
    }
    let (cmd, prefix_args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![])
    } else if cfg!(target_os = "windows") {
        ("explorer", vec![])
    } else {
        ("xdg-open", vec![])
    };
    let mut command = std::process::Command::new(cmd);
    command.args(&prefix_args).arg(path);
    match command.status() {
        Ok(status) if status.success() => Ok(json!({ "success": true, "path": path })),
        Ok(status) => Ok(
            json!({ "success": false, "error": format!("退出码 {:?}", status.code()), "path": path }),
        ),
        Err(e) => Ok(json!({ "success": false, "error": e.to_string() })),
    }
}

// ============================================================================
// accept + 后台任务通道（emit 事件；真实转录/向量依赖外部服务，标 TODO）
// ============================================================================

/// `knowledge:transcribe` —— 立即返回 accepted 并 emit `knowledge:note-updated`(processing)；
/// 真实转录由后台任务承担（见 [`run_transcribe_background`]）。
fn transcribe_impl(emitter: &dyn EventEmitter, payload: &Value) -> anyhow::Result<Value> {
    let Some(note_id) = note_id_of(payload) else {
        return Ok(json!({ "success": false, "error": "缺少 noteId" }));
    };
    emitter.emit(
        "knowledge:note-updated",
        json!({ "noteId": note_id, "hasTranscript": false, "transcriptionStatus": "processing" }),
    );
    Ok(json!({ "success": true, "accepted": true, "noteId": note_id }))
}

/// `knowledge:retry-youtube-subtitle` —— 立即返回 accepted 并 emit `knowledge:youtube-video-updated`(processing)；
/// 真实字幕重抓由后台任务承担（见 [`run_retry_youtube_subtitle_background`]）。
fn retry_youtube_subtitle_impl(
    emitter: &dyn EventEmitter,
    payload: &Value,
) -> anyhow::Result<Value> {
    let Some(video_id) = youtube_id_of(payload) else {
        return Ok(json!({ "success": false, "error": "缺少 videoId" }));
    };
    emitter.emit(
        "knowledge:youtube-video-updated",
        json!({ "noteId": video_id, "status": "processing" }),
    );
    Ok(json!({ "success": true, "accepted": true, "videoId": video_id }))
}

/// 后台转录占位：TODO 接入 STT（whisper/云转录）+ embedding 服务，写入 `knowledge_vectors`。
///
/// 当前未配置后端，emit failed 事件（对齐前端 transcriptionStatus）。
async fn run_transcribe_background(_db: Db, emitter: Arc<dyn EventEmitter>, note_id: String) {
    // TODO: 读取视频 -> STT 转录 -> 写 transcript -> 计算 embedding -> INSERT knowledge_vectors。
    emitter.emit(
        "knowledge:note-updated",
        json!({
            "noteId": note_id,
            "hasTranscript": false,
            "transcriptionStatus": "failed",
            "error": "transcription backend not configured (TODO: STT/embedding service)"
        }),
    );
}

/// 后台字幕重抓占位：TODO 接入字幕下载服务（yt-dlp/字幕队列）。
async fn run_retry_youtube_subtitle_background(emitter: Arc<dyn EventEmitter>, video_id: String) {
    // TODO: 重新下载字幕 -> 写 subtitleFile -> 生成摘要 -> emit completed。
    emitter.emit(
        "knowledge:youtube-video-updated",
        json!({
            "noteId": video_id,
            "status": "completed",
            "hasSubtitle": false,
            "error": "subtitle backend not configured (TODO: subtitle service)"
        }),
    );
}

// ============================================================================
// 视图 / 工具
// ============================================================================

/// 从 `knowledge_vectors` 聚合行构造目录条目：metadata JSON 字段并入顶层。
fn catalog_item_from_vector_row(row: &Value) -> Value {
    let source_id = row
        .get("source_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let source_type = row
        .get("source_type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let created_at = row.get("created_at").cloned().unwrap_or(Value::Null);

    let mut item = Map::new();
    item.insert("id".into(), json!(source_id.clone()));
    item.insert("sourceId".into(), json!(source_id));
    item.insert("kind".into(), json!(source_type));
    item.insert(
        "sourceType".into(),
        json!(row.get("source_type").cloned().unwrap_or(Value::Null)),
    );
    item.insert("createdAt".into(), created_at);

    // metadata JSON 字段并入（不覆盖已设的键）
    if let Some(meta_str) = row.get("metadata").and_then(|v| v.as_str()) {
        if let Ok(Value::Object(meta)) = serde_json::from_str::<Value>(meta_str) {
            for (k, v) in meta {
                item.entry(k).or_insert(v);
            }
        }
    }
    Value::Object(item)
}

/// 从 `document_knowledge_index` 聚合行构造文档源视图。
fn document_source_view(row: &Value) -> Value {
    let source_id = row.get("source_id").and_then(|v| v.as_str()).unwrap_or("");
    json!({
        "id": source_id,
        "sourceId": source_id,
        "kind": "document-source",
        "title": source_id,
        "name": source_id,
        "fileCount": row.get("file_count").and_then(|v| v.as_i64()).unwrap_or(0),
        "status": "completed",
        "updatedAt": row.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0),
    })
}

/// 单条 lane 的视图（对齐原 renderer lane 字段）。
fn lane_view(lane: &Value) -> Value {
    json!({
        "lane": lane.get("lane").cloned().unwrap_or(Value::Null),
        "label": lane.get("lane").cloned().unwrap_or(Value::Null),
        "status": lane.get("status").cloned().unwrap_or(Value::Null),
        "done": lane.get("done").and_then(|v| v.as_i64()).unwrap_or(0),
        "total": lane.get("total").and_then(|v| v.as_i64()).unwrap_or(0),
        "failed": lane.get("failed").and_then(|v| v.as_i64()).unwrap_or(0),
        "metadataOnly": lane.get("metadata_only").and_then(|v| v.as_i64()).unwrap_or(0),
        "lastUpdatedAt": lane.get("last_updated_at").cloned().unwrap_or(Value::Null),
        "nextRetryAt": lane.get("next_retry_at").cloned().unwrap_or(Value::Null),
    })
}

/// 取笔记 id：`{noteId}` / `{id}` / 裸字符串。
fn note_id_of(payload: &Value) -> Option<String> {
    payload
        .get("noteId")
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
        .or_else(|| payload.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 取 YouTube 视频 id：`{videoId}` / `{noteId}` / `{id}` / 裸字符串。
fn youtube_id_of(payload: &Value) -> Option<String> {
    payload
        .get("videoId")
        .or_else(|| payload.get("noteId"))
        .or_else(|| payload.get("id"))
        .and_then(|v| v.as_str())
        .or_else(|| payload.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 是否真正执行写/真实操作。
///
/// 规则（对齐「写/真实操作默认 dry_run，payload 有 dryRun/confirm 时尊重」）：
/// - 非对象 payload（裸字符串/数字等位置参数）→ 默认执行；
/// - `dryRun:false` → 执行；`dryRun:true` → 仅 dry-run；
/// - `confirm:true` → 执行；
/// - 其余（无字段）→ 默认 dry-run。
fn should_execute(payload: &Value) -> bool {
    let obj = match payload {
        Value::Object(m) => m,
        _ => return true,
    };
    if let Some(d) = obj.get("dryRun").and_then(|v| v.as_bool()) {
        return !d;
    }
    if let Some(c) = obj.get("confirm").and_then(|v| v.as_bool()) {
        return c;
    }
    false
}

/// 幂等建 `knowledge_vectors` 表（Rust schema 尚未包含此表；IF NOT EXISTS 对已建表无害）。
fn ensure_knowledge_vectors_table(db: &Db) -> anyhow::Result<()> {
    db.execute_json(
        "CREATE TABLE IF NOT EXISTS knowledge_vectors (\
            id TEXT PRIMARY KEY,\
            source_id TEXT NOT NULL,\
            source_type TEXT NOT NULL,\
            chunk_index INTEGER NOT NULL DEFAULT 0,\
            content TEXT NOT NULL DEFAULT '',\
            embedding BLOB NOT NULL DEFAULT x'',\
            metadata TEXT,\
            content_hash TEXT,\
            created_at TEXT DEFAULT CURRENT_TIMESTAMP\
         )",
        &[],
    )?;
    db.execute_json(
        "CREATE INDEX IF NOT EXISTS idx_vectors_source ON knowledge_vectors(source_id)",
        &[],
    )?;
    Ok(())
}

/// 当前毫秒时间戳。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{NoopEmitter, RecordingEmitter};

    /// 插入一条 `knowledge_vectors` 测试行（embedding 走默认空 blob，metadata 为 JSON 字符串）。
    fn seed_vector(db: &Db, id: &str, source_id: &str, source_type: &str, metadata: Value) {
        db.execute_json(
            "INSERT INTO knowledge_vectors (id, source_id, source_type, metadata) VALUES (?,?,?,?)",
            &[
                json!(id),
                json!(source_id),
                json!(source_type),
                json!(metadata.to_string()),
            ],
        )
        .unwrap();
    }

    #[test]
    fn docs_add_files_then_list() {
        let db = Db::open_in_memory().unwrap();
        let payload = json!({
            "confirm": true,
            "files": [
                { "sourceId": "src-a", "absolutePath": "/tmp/a.md", "title": "A" },
                { "sourceId": "src-a", "absolutePath": "/tmp/b.md", "title": "B" },
                { "sourceId": "src-b", "absolutePath": "/tmp/c.md" },
            ],
        });
        let res = docs_add_files_impl(&db, &payload).unwrap();
        assert_eq!(res["added"], json!(3));

        let listed = docs_list_impl(&db).unwrap();
        let items = listed.as_array().unwrap();
        assert_eq!(items.len(), 2, "应按 source_id 去重为 2 个文档源");
        let by_src: std::collections::HashMap<&str, i64> = items
            .iter()
            .map(|v| {
                (
                    v["sourceId"].as_str().unwrap(),
                    v["fileCount"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(by_src["src-a"], 2);
        assert_eq!(by_src["src-b"], 1);
    }

    #[test]
    fn docs_add_files_accepts_picker_paths_and_generates_metadata() {
        let db = Db::open_in_memory().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("picker-note.md");
        std::fs::write(&file, "picker content").unwrap();

        let result = docs_add_files_impl(
            &db,
            &json!({ "confirm": true, "paths": [file.to_string_lossy()] }),
        )
        .unwrap();
        assert_eq!(result["added"], json!(1));

        let rows = db
            .query_all_json(
                "SELECT source_id, relative_path, title, file_size FROM document_knowledge_index",
                &[],
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0]["source_id"]
            .as_str()
            .unwrap()
            .starts_with("doc-file-"));
        assert_eq!(rows[0]["relative_path"], json!("picker-note.md"));
        assert_eq!(rows[0]["title"], json!("picker-note.md"));
        assert!(rows[0]["file_size"].as_i64().unwrap() > 0);
    }

    #[test]
    fn docs_add_files_dry_run_does_not_write() {
        let db = Db::open_in_memory().unwrap();
        // 无 confirm / dryRun:false → 默认 dry-run
        let payload = json!({ "files": [{ "sourceId": "s", "absolutePath": "/x" }] });
        let res = docs_add_files_impl(&db, &payload).unwrap();
        assert_eq!(res["dryRun"], json!(true));
        let listed = docs_list_impl(&db).unwrap();
        assert!(listed.as_array().unwrap().is_empty(), "dry-run 不应落库");
    }

    #[test]
    fn index_status_counts_lanes() {
        let db = Db::open_in_memory().unwrap();
        db.execute_json(
            "INSERT INTO file_index_lanes (id, scope_id, lane, status, done, total, failed) \
             VALUES ('l1','scope-1','vectors','running',5,10,1)",
            &[],
        )
        .unwrap();
        db.execute_json(
            "INSERT INTO file_index_lanes (id, scope_id, lane, status, done, total, failed) \
             VALUES ('l2','scope-1','docs','idle',3,3,0)",
            &[],
        )
        .unwrap();
        let status = get_index_status_impl(&db).unwrap();
        assert_eq!(status["indexedCount"], json!(8)); // 5 + 3
        assert_eq!(status["failedCount"], json!(1));
        assert_eq!(status["isBuilding"], json!(true)); // l1 running
    }

    #[test]
    fn rebuild_catalog_resets_lanes() {
        let db = Db::open_in_memory().unwrap();
        db.execute_json(
            "INSERT INTO file_index_lanes (id, scope_id, lane, status, done, total, failed) \
             VALUES ('l1','s','vectors','running',7,10,2)",
            &[],
        )
        .unwrap();
        let res = rebuild_catalog_impl(&db, &json!({ "confirm": true })).unwrap();
        assert_eq!(res["reset"], json!(1));
        let row = db
            .query_one_json(
                "SELECT done, failed, status FROM file_index_lanes WHERE id='l1'",
                &[],
            )
            .unwrap()
            .unwrap();
        assert_eq!(row["done"], json!(0));
        assert_eq!(row["failed"], json!(0));
        assert_eq!(row["status"], json!("idle"));
    }

    #[test]
    fn list_dedupes_by_source_id_and_merges_metadata() {
        let db = Db::open_in_memory().unwrap();
        ensure_knowledge_vectors_table(&db).unwrap();
        seed_vector(
            &db,
            "v1",
            "note-x",
            "redbook-note",
            json!({ "title": "笔记一", "author": "张三" }),
        );
        seed_vector(
            &db,
            "v2",
            "note-x",
            "redbook-note",
            json!({ "title": "笔记一", "tags": ["t1"] }),
        );
        seed_vector(
            &db,
            "v3",
            "note-y",
            "redbook-note",
            json!({ "title": "笔记二" }),
        );

        let items = list_impl(&db).unwrap();
        let arr = items.as_array().unwrap();
        assert_eq!(arr.len(), 2, "按 source_id 去重");
        let x = arr.iter().find(|v| v["id"] == "note-x").unwrap();
        assert_eq!(x["kind"], json!("redbook-note"));
        assert_eq!(x["title"], json!("笔记一"));
        assert_eq!(x["author"], json!("张三"));
    }

    #[test]
    fn list_page_filters_by_kind_and_query() {
        let db = Db::open_in_memory().unwrap();
        ensure_knowledge_vectors_table(&db).unwrap();
        seed_vector(
            &db,
            "v1",
            "n1",
            "redbook-note",
            json!({ "title": "Rust 笔记" }),
        );
        seed_vector(
            &db,
            "v2",
            "n2",
            "redbook-note",
            json!({ "title": "Go 语言" }),
        );
        // document source
        db.execute_json(
            "INSERT INTO document_knowledge_index (space_id, source_id, absolute_path, relative_path, updated_at) \
             VALUES ('default','doc-1','/d.md','d.md',1)",
            &[],
        )
        .unwrap();

        // kind 过滤
        let docs = list_page_impl(
            &db,
            &json!({ "payload": { "kind": "document-source", "limit": 10 } }),
        )
        .unwrap();
        assert_eq!(docs["total"], json!(1));
        assert_eq!(docs["kindCounts"]["document-source"], json!(1));

        // query 过滤（匹配 title）
        let rust = list_page_impl(&db, &json!({ "query": "rust" })).unwrap();
        let titles: Vec<&str> = rust["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["title"].as_str().unwrap_or(""))
            .collect();
        assert!(titles.iter().any(|t| t.contains("Rust")));
        assert!(!titles.iter().any(|t| t.contains("Go 语言")));
    }

    #[tokio::test]
    async fn docs_add_folder_scans_tempdir() {
        let db = Db::open_in_memory().unwrap();
        let dir = std::env::temp_dir().join(format!("kv-add-folder-{}", now_ms()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("a.md"), "hello").await.unwrap();
        tokio::fs::write(dir.join("b.md"), "world").await.unwrap();
        // 文件夹导入会递归登记子目录文件。
        tokio::fs::create_dir_all(dir.join("sub")).await.unwrap();
        tokio::fs::write(dir.join("sub/c.md"), "skip")
            .await
            .unwrap();

        let payload =
            json!({ "confirm": true, "sourceId": "folder-1", "path": dir.display().to_string() });
        let res = docs_add_folder_impl(&db, &payload).await.unwrap();
        assert_eq!(res["success"], json!(true));
        assert_eq!(res["added"], json!(3), "应递归登记 3 个文件");

        let rows = db
            .query_all_json(
                "SELECT relative_path FROM document_knowledge_index WHERE source_id='folder-1' ORDER BY relative_path",
                &[],
            )
            .unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().any(|row| row["relative_path"] == "sub/c.md"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transcribe_emits_processing_and_returns_accepted() {
        let emitter = RecordingEmitter::new();
        let res = transcribe_impl(&emitter, &json!({ "noteId": "n1" })).unwrap();
        assert_eq!(res["accepted"], json!(true));
        let events = emitter.events.blocking_lock();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "knowledge:note-updated");
        assert_eq!(events[0].1["transcriptionStatus"], json!("processing"));
    }

    #[test]
    fn get_item_detail_document_source_includes_files() {
        let db = Db::open_in_memory().unwrap();
        db.execute_json(
            "INSERT INTO document_knowledge_index (space_id, source_id, absolute_path, relative_path, title, updated_at) \
             VALUES ('default','doc-1','/a.md','a.md','A',1)",
            &[],
        )
        .unwrap();
        db.execute_json(
            "INSERT INTO document_knowledge_index (space_id, source_id, absolute_path, relative_path, title, updated_at) \
             VALUES ('default','doc-1','/b.md','b.md','B',2)",
            &[],
        )
        .unwrap();
        let detail = get_item_detail_impl(
            &db,
            &json!({ "payload": { "itemId": "doc-1", "kind": "document-source" } }),
        )
        .unwrap();
        assert_eq!(detail["kind"], json!("document-source"));
        assert_eq!(detail["fileCount"], json!(2));
        assert_eq!(detail["files"].as_array().unwrap().len(), 2);
    }

    #[test]
    #[ignore] // 后台任务真实依赖 STT/embedding 服务
    fn transcribe_background_emits_failed_when_unconfigured() {
        let rec = std::sync::Arc::new(RecordingEmitter::new());
        let emitter_dyn: std::sync::Arc<dyn EventEmitter> = rec.clone();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            run_transcribe_background(db_helper(), emitter_dyn, "n1".into()).await;
        });
        let events = rec.events.blocking_lock();
        let last = events.last().unwrap();
        assert_eq!(last.1["transcriptionStatus"], json!("failed"));
    }

    #[test]
    #[ignore] // 需真实 OS 打开命令
    fn open_index_root_dry_run_preview() {
        let res = open_index_root_impl(&json!({ "path": "/tmp", "dryRun": true })).unwrap();
        assert_eq!(res["dryRun"], json!(true));
        assert_eq!(res["path"], json!("/tmp"));
    }

    #[test]
    fn noop_emitter_satisfies_trait() {
        let _e: &dyn EventEmitter = &NoopEmitter;
    }

    /// 为 ignore 测试提供一个独立内存库。
    fn db_helper() -> Db {
        Db::open_in_memory().unwrap()
    }
}
