//! `data` 命名空间：向量 / 索引 / 相似度 / YouTube 二进制 / AI 源元数据。
//!
//! 迁移自 Beav `desktop/electron/main.ts`：
//! - `embedding:*` / `similarity:*` —— 稿件 embedding 缓存与相似度排序缓存（`manuscript_embeddings` /
//!   `manuscript_similarity_cache` 真实读写）；`compute` / `rebuild-all` 需 embedding 服务，标 `#[ignore]`。
//! - `indexing:*` —— 文件索引队列（`file_index_lanes` / `file_index_events`）；`get-stats` /
//!   `remove-item` / `clear-queue` 真实落库，`rebuild-all` / `rebuild-advisor` 扫描工作区目录（需 fs，`#[ignore]`）。
//! - `youtube:*` —— yt-dlp 二进制管理（子进程），环境依赖，标 `#[ignore]`。
//! - `ai:fetch-models` / `ai:test-connection` —— 真实 HTTP 拉取 provider 模型列表；
//!   `ai:detect-protocol` 为纯启发式（与 Beav `detectAiProtocol` 对齐）；`ai:roles:list` 为内置常量。
//!
//! 写操作默认 dry_run（`payload.dryRun===true` 或 `confirm` 缺省时），见 [`should_execute`]。
//! DB 走 [`Db::query_all_json`] / [`Db::execute_json`]；embedding 以 JSON 数组串存入 `BLOB` 列
//! （SQLite BLOB 亲和保留存储类，可经 JSON 助手往返）。

use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use crate::db::Db;
use crate::ipc::AppState;

/// 知识库版本号（内存态，对齐 Beav `knowledgeVersion = Date.now()`，进程内单调）。
/// 每次 `indexing:rebuild-all` 真实执行后递增；`similarity:get-knowledge-version` 读取。
static KNOWLEDGE_VERSION: AtomicI64 = AtomicI64::new(0);
/// 测试 / lane id 生成用的自增计数。
static ID_SEQ: AtomicU64 = AtomicU64::new(0);

/// `data` 命名空间的双向通道分发。未知通道返回 `Err`。
///
/// 由 `yunying_server::ipc::dispatch_invoke` 在命名空间前缀为
/// `embedding` / `similarity` / `indexing` / `youtube` / `ai` 时路由到此。
pub async fn invoke(channel: &str, payload: Value, state: &AppState) -> anyhow::Result<Value> {
    match channel {
        // ---------------- embedding ----------------
        "embedding:compute" => embedding_compute_impl(&payload, state).await,
        "embedding:get-sorted-sources" => {
            Ok(embedding_get_sorted_sources_impl(&state.db, &payload)?)
        }
        "embedding:rebuild-all" => embedding_rebuild_all_impl(&state.db, &payload), // 需服务，结构完整
        "embedding:get-status" => Ok(embedding_get_status_impl(&state.db)?),
        "embedding:get-manuscript-cache" => {
            Ok(embedding_get_manuscript_cache_impl(&state.db, &payload)?)
        }
        "embedding:save-manuscript-cache" => {
            Ok(embedding_save_manuscript_cache_impl(&state.db, &payload)?)
        }

        // ---------------- similarity ----------------
        "similarity:get-cache" => Ok(similarity_get_cache_impl(&state.db, &payload)?),
        "similarity:save-cache" => Ok(similarity_save_cache_impl(&state.db, &payload)?),
        "similarity:get-knowledge-version" => Ok(json!(KNOWLEDGE_VERSION.load(Ordering::Relaxed))),

        // ---------------- indexing ----------------
        "indexing:get-stats" => Ok(indexing_get_stats_impl(&state.db)?),
        "indexing:remove-item" => Ok(indexing_remove_item_impl(&state.db, &payload)?),
        "indexing:clear-queue" => Ok(indexing_clear_queue_impl(&state.db, &payload)?),
        "indexing:rebuild-all" => indexing_rebuild_all_impl(&state.db, &payload).await,
        "indexing:rebuild-advisor" => indexing_rebuild_advisor_impl(&state.db, &payload).await,

        // ---------------- youtube (yt-dlp binary) ----------------
        "youtube:check-ytdlp" => youtube_check_ytdlp_impl().await,
        "youtube:install" => youtube_install_impl(state).await,
        "youtube:update" => youtube_update_impl().await,

        // ---------------- ai source ----------------
        "ai:fetch-models" => ai_fetch_models_impl(&payload).await,
        "ai:roles:list" => Ok(ai_roles_list_impl()),
        "ai:detect-protocol" => {
            Ok(json!({ "success": true, "protocol": detect_ai_protocol(&payload) }))
        }
        "ai:test-connection" => ai_test_connection_impl(&payload).await,

        other => Err(anyhow::anyhow!("data 通道未实现: {other}")),
    }
}

// ---------------------------------------------------------------------------
// embedding
// ---------------------------------------------------------------------------

/// `embedding:compute` —— 调 embedding 服务把文本转向量。需真实 embedding 服务（HTTP/本地模型），
/// 此处为结构完整占位：返回 `success:false` 并说明服务未接入。`#[ignore]` 测试覆盖。
async fn embedding_compute_impl(payload: &Value, _state: &AppState) -> anyhow::Result<Value> {
    let text = payload
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
    // TODO: 接入 embedding 服务（state.goose 或独立 embedding 客户端）：embed_query(text) -> Vec<f32>。
    Ok(json!({
        "success": false,
        "error": "embedding service not configured",
        "text": text,
    }))
}

/// `embedding:get-sorted-sources` —— 给定查询向量，按余弦相似度排序所有 `knowledge_vectors` 的 source_id
/// （每个 source 取最大分），复刻 Beav `getSimilaritySortedSourceIds`。仅统计当前活动空间的向量。
fn embedding_get_sorted_sources_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    ensure_knowledge_vectors_table(db)?;
    let query = payload_embedding_vec(payload);
    let active = active_space_id(db);

    let rows = db.query_all_json(
        "SELECT source_id, embedding, metadata FROM knowledge_vectors",
        &[],
    )?;

    // source_id -> 最大相似度
    let mut best: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
    for row in &rows {
        let source_id = match row.get("source_id").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let metadata = parse_metadata(row.get("metadata").and_then(|v| v.as_str()));
        if !is_vector_in_space(&metadata, &active) {
            continue;
        }
        let raw = row.get("embedding").and_then(|v| v.as_str()).unwrap_or("");
        let Some(vec) = decode_embedding_f32(raw) else {
            continue; // 空 blob / 未嵌入的行跳过
        };
        if vec.len() != query.len() {
            continue; // 维度不一致跳过
        }
        let score = cosine(&query, &vec);
        let entry = best.entry(source_id).or_insert(0.0);
        if score > *entry {
            *entry = score;
        }
    }

    let mut sorted: Vec<(String, f32)> = best.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let out = sorted
        .into_iter()
        .map(|(source_id, score)| json!({ "sourceId": source_id, "score": score }))
        .collect::<Vec<_>>();
    Ok(json!({ "success": true, "sorted": out }))
}

/// `embedding:rebuild-all` —— 全量重建知识库索引。需 embedding 服务才能真正重算向量；
/// 这里统计 `knowledge_vectors` 条目数并（真实执行时）清空 lanes，返回 `queued` 计数。`#[ignore]` 测试覆盖。
fn embedding_rebuild_all_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    ensure_knowledge_vectors_table(db)?;
    let rows = db.query_all_json("SELECT DISTINCT source_id FROM knowledge_vectors", &[])?;
    let queued = rows.len() as i64;
    if should_execute(payload) {
        // 清空索引队列 lane（真实）；真正重嵌需 embedding 服务，见 TODO。
        let _ = db.execute_json("DELETE FROM file_index_lanes", &[]);
        bump_knowledge_version();
    }
    Ok(json!({ "success": true, "queued": queued, "dryRun": !should_execute(payload) }))
}

/// `embedding:get-status` —— 索引状态（聚合 `knowledge_vectors` 与 `file_index_lanes`）。
fn embedding_get_status_impl(db: &Db) -> anyhow::Result<Value> {
    ensure_knowledge_vectors_table(db)?;
    let (total_vectors, total_documents) = vector_counts(db)?;
    let lanes = db.query_all_json("SELECT * FROM file_index_lanes", &[])?;
    let is_indexing = lanes
        .iter()
        .any(|l| l.get("status").and_then(|v| v.as_str()) == Some("running"));
    Ok(json!({
        "isIndexing": is_indexing,
        "totalQueueLength": lanes.len(),
        "activeItems": [],
        "queuedItems": [],
        "processedCount": 0,
        "totalStats": {
            "vectors": total_vectors,
            "documents": total_documents,
        },
    }))
}

/// `embedding:get-manuscript-cache` —— 取稿件缓存向量（`manuscript_embeddings`）。
fn embedding_get_manuscript_cache_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let file_path = flex_str(payload, "filePath");
    if file_path.is_empty() {
        return Ok(json!({ "success": false, "error": "filePath is required" }));
    }
    let row = db.query_one_json(
        "SELECT content_hash, embedding FROM manuscript_embeddings WHERE file_path = ?",
        &[json!(file_path)],
    )?;
    let Some(row) = row else {
        return Ok(json!({ "success": true, "cached": Value::Null }));
    };
    let content_hash = row
        .get("content_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let raw = row.get("embedding").and_then(|v| v.as_str()).unwrap_or("");
    let vec = decode_embedding_f64(raw).unwrap_or_default();
    Ok(json!({
        "success": true,
        "cached": {
            "contentHash": content_hash,
            "embedding": vec,
        },
    }))
}

/// `embedding:save-manuscript-cache` —— 存稿件缓存向量。写操作默认 dry_run。
fn embedding_save_manuscript_cache_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let file_path = flex_str(payload, "filePath");
    let content_hash = flex_str(payload, "contentHash");
    let embedding = payload
        .get("embedding")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if file_path.is_empty() {
        return Ok(json!({ "success": false, "error": "filePath is required" }));
    }
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true }));
    }
    let encoded = encode_embedding(&embedding)?;
    db.execute_json(
        "INSERT INTO manuscript_embeddings (file_path, content_hash, embedding, created_at) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(file_path) DO UPDATE SET content_hash = ?2, embedding = ?3, created_at = ?4",
        &[
            json!(file_path),
            json!(content_hash),
            json!(encoded),
            json!(now_ms()),
        ],
    )?;
    Ok(json!({ "success": true }))
}

// ---------------------------------------------------------------------------
// similarity
// ---------------------------------------------------------------------------

/// `similarity:get-cache` —— 取稿件相似度排序缓存，附带当前知识库版本。
fn similarity_get_cache_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let manuscript_id = flex_str(payload, "manuscriptId");
    if manuscript_id.is_empty() {
        return Ok(json!({ "success": false, "error": "manuscriptId is required" }));
    }
    let row = db.query_one_json(
        "SELECT manuscript_id, content_hash, knowledge_version, sorted_ids, created_at \
         FROM manuscript_similarity_cache WHERE manuscript_id = ?",
        &[json!(manuscript_id)],
    )?;
    let cache = row.map(|r| {
        let sorted_ids: Value = r
            .get("sorted_ids")
            .and_then(|v| v.as_str())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Value::Array(vec![]));
        json!({
            "manuscriptId": r.get("manuscript_id").and_then(|v| v.as_str()).unwrap_or(""),
            "contentHash": r.get("content_hash").and_then(|v| v.as_str()).unwrap_or(""),
            "knowledgeVersion": r.get("knowledge_version").and_then(|v| v.as_i64()).unwrap_or(0),
            "sortedIds": sorted_ids,
            "createdAt": r.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0),
        })
    });
    Ok(json!({
        "success": true,
        "cache": cache,
        "currentKnowledgeVersion": KNOWLEDGE_VERSION.load(Ordering::Relaxed),
    }))
}

/// `similarity:save-cache` —— 存稿件相似度排序缓存。写操作默认 dry_run。
fn similarity_save_cache_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let manuscript_id = flex_str(payload, "manuscriptId");
    let content_hash = flex_str(payload, "contentHash");
    let knowledge_version = payload
        .get("knowledgeVersion")
        .and_then(|v| v.as_i64())
        .or_else(|| payload.get("knowledge_version").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    let sorted_ids = payload
        .get("sortedIds")
        .or_else(|| payload.get("sorted_ids"))
        .cloned()
        .unwrap_or(Value::Array(vec![]));
    if manuscript_id.is_empty() {
        return Ok(json!({ "success": false, "error": "manuscriptId is required" }));
    }
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true }));
    }
    let sorted_ids_str = serde_json::to_string(&sorted_ids)?;
    db.execute_json(
        "INSERT INTO manuscript_similarity_cache \
         (manuscript_id, content_hash, knowledge_version, sorted_ids, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(manuscript_id) DO UPDATE SET \
           content_hash = ?2, knowledge_version = ?3, sorted_ids = ?4, created_at = ?5",
        &[
            json!(manuscript_id),
            json!(content_hash),
            json!(knowledge_version),
            json!(sorted_ids_str),
            json!(now_ms()),
        ],
    )?;
    Ok(json!({ "success": true }))
}

// ---------------------------------------------------------------------------
// indexing
// ---------------------------------------------------------------------------

/// `indexing:get-stats` —— 索引统计：向量数 / 文档数 / lane 计数与进度汇总。
fn indexing_get_stats_impl(db: &Db) -> anyhow::Result<Value> {
    ensure_knowledge_vectors_table(db)?;
    let (total_vectors, total_documents) = vector_counts(db)?;
    let lanes = db.query_all_json("SELECT * FROM file_index_lanes", &[])?;
    let lane_total: i64 = lanes.len() as i64;
    let lane_done: i64 = lanes
        .iter()
        .map(|l| l.get("done").and_then(|v| v.as_i64()).unwrap_or(0))
        .sum();
    let lane_sum_total: i64 = lanes
        .iter()
        .map(|l| l.get("total").and_then(|v| v.as_i64()).unwrap_or(0))
        .sum();
    let lane_failed: i64 = lanes
        .iter()
        .map(|l| l.get("failed").and_then(|v| v.as_i64()).unwrap_or(0))
        .sum();
    let is_indexing = lanes
        .iter()
        .any(|l| l.get("status").and_then(|v| v.as_str()) == Some("running"));
    Ok(json!({
        "isIndexing": is_indexing,
        "totalQueueLength": lane_total,
        "processedCount": lane_done,
        "totalStats": {
            "vectors": total_vectors,
            "documents": total_documents,
        },
        "lanes": {
            "count": lane_total,
            "done": lane_done,
            "total": lane_sum_total,
            "failed": lane_failed,
        },
    }))
}

/// `indexing:remove-item` —— 从索引队列移除某项（删 `file_index_lanes` 中匹配 id 的行及其事件）。
/// 写操作默认 dry_run。
fn indexing_remove_item_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let item_id = flex_str(payload, "itemId");
    if item_id.is_empty() {
        return Ok(json!({ "success": false, "error": "itemId is required" }));
    }
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "itemId": item_id }));
    }
    let n1 = db.execute_json(
        "DELETE FROM file_index_lanes WHERE id = ?",
        &[json!(item_id)],
    )?;
    let n2 = db.execute_json(
        "DELETE FROM file_index_events WHERE source_id = ?",
        &[json!(item_id)],
    )?;
    Ok(json!({ "success": true, "removed": n1, "eventsRemoved": n2 }))
}

/// `indexing:clear-queue` —— 清空未完成的索引 lane（status 非 done/completed/succeeded）。
/// 写操作默认 dry_run。
fn indexing_clear_queue_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true }));
    }
    let n = db.execute_json(
        "DELETE FROM file_index_lanes \
         WHERE status IS NULL OR status NOT IN ('done','completed','succeeded')",
        &[],
    )?;
    Ok(json!({ "success": true, "cleared": n }))
}

/// `indexing:rebuild-all` —— 清空并重建索引：清 lane（+可选清向量），扫描工作区知识目录，
/// 重建聚合 lane。需要文件系统与 embedding 服务（真正重嵌在此未接入），`#[ignore]` 测试覆盖。
async fn indexing_rebuild_all_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "items": 0 }));
    }
    // 1) 清空 lanes（与 Beav clearAndRebuild 对齐；向量清理由 embedding 服务负责，默认不动）
    let _ = db.execute_json("DELETE FROM file_index_lanes", &[]);
    if payload
        .get("clearVectors")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        ensure_knowledge_vectors_table(db)?;
        let _ = db.execute_json("DELETE FROM knowledge_vectors", &[]);
    }

    // 2) 扫描工作区知识目录，统计可索引条目
    let workspace = workspace_dir(db);
    let total = scan_all_knowledge_items(&workspace).await;

    // 3) 写一条聚合 "rebuild" lane
    let lane_id = next_id("lane");
    db.execute_json(
        "INSERT INTO file_index_lanes \
         (id, scope_id, lane, status, done, total, failed, metadata_only, last_updated_at, next_retry_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
        &[
            json!(lane_id),
            json!("__rebuild__"),
            json!("queue"),
            json!("queued"),
            json!(0i64),
            json!(total),
            json!(0i64),
            json!(0i64),
            json!(now_ms().to_string()),
        ],
    )?;
    bump_knowledge_version();
    Ok(json!({ "success": true, "items": total, "lane": lane_id }))
}

/// `indexing:rebuild-advisor` —— 重建单个 advisor 的知识索引（扫描其 knowledge 目录）。
async fn indexing_rebuild_advisor_impl(db: &Db, payload: &Value) -> anyhow::Result<Value> {
    let advisor_id = flex_str(payload, "advisorId");
    if advisor_id.is_empty() {
        return Ok(json!({ "success": false, "error": "advisorId is required" }));
    }
    if !should_execute(payload) {
        return Ok(json!({ "success": true, "dryRun": true, "items": 0 }));
    }
    let workspace = workspace_dir(db);
    let knowledge_dir = workspace
        .join("advisors")
        .join(&advisor_id)
        .join("knowledge");
    let total = count_advisor_knowledge_items(&knowledge_dir).await;

    let lane_id = next_id("lane");
    db.execute_json(
        "INSERT INTO file_index_lanes \
         (id, scope_id, lane, status, done, total, failed, metadata_only, last_updated_at, next_retry_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
        &[
            json!(lane_id),
            json!(format!("advisor:{advisor_id}")),
            json!("queue"),
            json!("queued"),
            json!(0i64),
            json!(total),
            json!(0i64),
            json!(0i64),
            json!(now_ms().to_string()),
        ],
    )?;
    Ok(json!({ "success": true, "items": total, "advisorId": advisor_id, "lane": lane_id }))
}

// ---------------------------------------------------------------------------
// youtube (yt-dlp binary management) —— 环境依赖，标 #[ignore]
// ---------------------------------------------------------------------------

/// `youtube:check-ytdlp` —— 检测 yt-dlp 是否可用（本地 bin 或 PATH），返回 `{installed, version, path}`。
async fn youtube_check_ytdlp_impl() -> anyhow::Result<Value> {
    // 1) 本地安装元信息（~/.yunying/bin 或 workspace bin）
    if let Some(meta) = read_local_ytdlp_meta() {
        return Ok(json!({
            "installed": true,
            "version": meta.0,
            "path": meta.1,
        }));
    }
    // 2) PATH 中尝试 `yt-dlp --version`
    let probed = tokio::task::spawn_blocking(|| {
        std::process::Command::new("yt-dlp")
            .arg("--version")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
    })
    .await
    .ok()
    .flatten();
    if let Some(version) = probed {
        return Ok(json!({ "installed": true, "version": version, "path": "yt-dlp" }));
    }
    Ok(json!({ "installed": false }))
}

/// `youtube:install` —— 安装 yt-dlp（下载二进制 / pip install）。需网络与系统权限，占位：
/// 发出 `youtube:install-progress` 事件并返回结构化结果。`#[ignore]` 测试覆盖。
async fn youtube_install_impl(state: &AppState) -> anyhow::Result<Value> {
    state.emitter.emit(
        "youtube:install-progress",
        json!({ "stage": "start", "message": "installing yt-dlp" }),
    );
    // TODO: 下载 https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos 到本地 bin，
    //       或 `pip install -U yt-dlp`。此处仅占位。
    let installed = install_ytdlp_best_effort().await;
    state.emitter.emit(
        "youtube:install-progress",
        json!({ "stage": "done", "success": installed }),
    );
    Ok(json!({ "success": installed }))
}

/// `youtube:update` —— 更新 yt-dlp（`yt-dlp -U`）。环境依赖，占位。
async fn youtube_update_impl() -> anyhow::Result<Value> {
    let updated = tokio::task::spawn_blocking(|| {
        std::process::Command::new("yt-dlp")
            .arg("-U")
            .output()
            .ok()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false);
    Ok(json!({ "success": updated }))
}

/// 占位安装实现：尝试 `pip install -U yt-dlp`，失败返回 false（不 panic）。
async fn install_ytdlp_best_effort() -> bool {
    tokio::task::spawn_blocking(|| {
        std::process::Command::new("pip3")
            .args(["install", "-U", "yt-dlp"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

/// 读取本地 yt-dlp 安装元信息（`~/.yunying/bin/yt-dlp` 或 `yt-dlp.json`）。
fn read_local_ytdlp_meta() -> Option<(String, String)> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let bin = home.join(".yunying/bin/yt-dlp");
    if bin.exists() {
        let meta = std::fs::read_to_string(home.join(".yunying/bin/yt-dlp.json")).ok();
        let version = meta
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| {
                v.get("version")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "已安装".to_string());
        return Some((version, bin.to_string_lossy().into_owned()));
    }
    None
}

// ---------------------------------------------------------------------------
// ai source
// ---------------------------------------------------------------------------

/// `ai:fetch-models` —— 拉取 provider 模型列表。保持旧 Electron 契约：成功时直接返回数组。
async fn ai_fetch_models_impl(payload: &Value) -> anyhow::Result<Value> {
    let protocol = detect_ai_protocol(payload);
    let base_url = resolve_ai_base_url(payload, &protocol)?;
    let api_key = payload
        .get("apiKey")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    if api_key.is_empty() && !allow_empty_ai_api_key(payload, &base_url, &protocol) {
        return Err(anyhow::anyhow!(
            "请先填写 API Key（本地 OpenAI 兼容服务可留空）"
        ));
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| anyhow::anyhow!("创建 HTTP 客户端失败: {e}"))?;

    let mut last_error = String::new();
    for endpoint in ai_models_endpoints(&protocol, &base_url) {
        let mut request = client.get(&endpoint).header("accept", "application/json");
        match protocol.as_str() {
            "anthropic" => {
                request = request
                    .header("anthropic-version", "2023-06-01")
                    .header("x-api-key", api_key);
            }
            "gemini" => {
                request = request.query(&[("key", api_key)]);
                if !api_key.is_empty() {
                    request = request.header("x-goog-api-key", api_key);
                }
            }
            _ => {
                if !api_key.is_empty() {
                    request = request.header("authorization", format!("Bearer {api_key}"));
                }
            }
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                last_error = format!("{endpoint}: {error}");
                continue;
            }
        };
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("读取模型列表响应失败: {e}"))?;
        if !status.is_success() {
            last_error = format!("{endpoint}: HTTP {status}: {}", truncate_http_body(&body));
            continue;
        }

        match serde_json::from_str::<Value>(&body) {
            Ok(value) => {
                return Ok(Value::Array(extract_ai_model_descriptors(
                    &protocol, &value,
                )))
            }
            Err(error) => {
                last_error = format!("{endpoint}: 响应不是有效 JSON: {error}");
            }
        }
    }
    Err(anyhow::anyhow!("模型列表请求失败: {last_error}"))
}

/// `ai:test-connection` —— 测试 AI 源连通性。
async fn ai_test_connection_impl(payload: &Value) -> anyhow::Result<Value> {
    let protocol = detect_ai_protocol(payload);
    let base_url = payload
        .get("baseURL")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match ai_fetch_models_impl(payload).await {
        Ok(models) => {
            let count = models.as_array().map_or(0, Vec::len);
            Ok(json!({
                "success": true,
                "protocol": protocol,
                "models": models,
                "message": format!("连接成功，返回 {count} 个模型"),
                "baseURL": base_url,
            }))
        }
        Err(e) => Ok(json!({
            "success": false,
            "protocol": protocol,
            "models": [],
            "message": e.to_string(),
            "baseURL": base_url,
        })),
    }
}

/// 后台刷新默认 AI 源中已选模型的元数据。只合并当前已经添加/选中的模型，
/// 不会把远端完整模型列表直接塞进用户配置。
pub async fn refresh_default_ai_source_model_metadata(db: &Db) -> anyhow::Result<Value> {
    let settings = db.settings().get()?;
    let raw = settings
        .get("ai_sources_json")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return Ok(json!({ "updated": false, "reason": "no_ai_sources" }));
    }
    let mut sources = serde_json::from_str::<Value>(raw)?;
    let source_items = sources
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("ai_sources_json 不是数组"))?;
    if source_items.is_empty() {
        return Ok(json!({ "updated": false, "reason": "no_ai_sources" }));
    }
    let default_id = settings
        .get("default_ai_source_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let source_index = source_items
        .iter()
        .position(|source| {
            !default_id.is_empty()
                && source.get("id").and_then(Value::as_str).map(str::trim) == Some(default_id)
        })
        .unwrap_or(0);
    let source = source_items
        .get_mut(source_index)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| anyhow::anyhow!("默认 AI 源不是对象"))?;
    let base_url = source
        .get("baseURL")
        .or_else(|| source.get("base_url"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| settings.get("api_endpoint").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let api_key = source
        .get("apiKey")
        .or_else(|| source.get("api_key"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or_else(|| settings.get("api_key").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let protocol = source
        .get("protocol")
        .and_then(Value::as_str)
        .unwrap_or("openai")
        .trim()
        .to_string();
    let preset_id = source
        .get("presetId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let fetched = ai_fetch_models_impl(&json!({
        "baseURL": base_url,
        "apiKey": api_key,
        "protocol": protocol,
        "presetId": preset_id,
    }))
    .await?;
    let fetched_items = fetched.as_array().cloned().unwrap_or_default();
    let fetched_by_id = fetched_items
        .into_iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.trim().to_string();
            (!id.is_empty()).then_some((id, item))
        })
        .collect::<std::collections::HashMap<_, _>>();

    let mut selected_ids = Vec::new();
    if let Some(model) = source
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        selected_ids.push(model.to_string());
    }
    if let Some(models) = source.get("models").and_then(Value::as_array) {
        for model in models.iter().filter_map(Value::as_str).map(str::trim) {
            if !model.is_empty() && !selected_ids.iter().any(|existing| existing == model) {
                selected_ids.push(model.to_string());
            }
        }
    }
    if selected_ids.is_empty() {
        if let Some(model) = settings
            .get("model_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            selected_ids.push(model.to_string());
            source.insert("model".into(), json!(model));
            source.insert("models".into(), json!([model]));
        }
    }
    if selected_ids.is_empty() {
        return Ok(json!({ "updated": false, "reason": "no_selected_models" }));
    }

    let existing_meta = source
        .get("modelsMeta")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let existing_by_id = existing_meta
        .into_iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.trim().to_string();
            (!id.is_empty()).then_some((id, item))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut merged_meta = Vec::new();
    for id in &selected_ids {
        let mut merged = existing_by_id
            .get(id)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        merged.insert("id".into(), json!(id));
        if let Some(fetched) = fetched_by_id.get(id).and_then(Value::as_object) {
            for (key, value) in fetched {
                merged.insert(key.clone(), value.clone());
            }
        }
        merged_meta.push(Value::Object(merged));
    }
    source.insert("modelsMeta".into(), Value::Array(merged_meta.clone()));
    let source_id = source.get("id").cloned().unwrap_or(Value::Null);
    let active_model = source
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let active = merged_meta
        .iter()
        .find(|item| {
            item.get("id").and_then(Value::as_str).map(str::trim) == Some(active_model.as_str())
        })
        .cloned();
    let serialized_sources = serde_json::to_string(&sources)?;
    db.settings().save(&json!({
        "ai_sources_json": serialized_sources,
    }))?;
    Ok(json!({
        "updated": true,
        "sourceId": source_id,
        "modelCount": merged_meta.len(),
        "activeModel": active,
    }))
}

fn resolve_ai_base_url(payload: &Value, protocol: &str) -> anyhow::Result<String> {
    let base = payload
        .get("baseURL")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if !base.is_empty() {
        return Ok(trim_trailing_slashes(base));
    }
    match protocol {
        "anthropic" => Ok("https://api.anthropic.com/v1".into()),
        "gemini" => Ok("https://generativelanguage.googleapis.com/v1beta".into()),
        "openai" => Ok("https://api.openai.com/v1".into()),
        _ => Err(anyhow::anyhow!("请先填写 Endpoint")),
    }
}

fn allow_empty_ai_api_key(payload: &Value, base_url: &str, protocol: &str) -> bool {
    if protocol != "openai" {
        return false;
    }
    let base = base_url.to_lowercase();
    if base.contains("localhost")
        || base.contains("127.0.0.1")
        || base.contains("0.0.0.0")
        || base.contains("[::1]")
        || base.contains("://::1")
    {
        return true;
    }
    let preset = payload
        .get("presetId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    [
        "local",
        "ollama",
        "lmstudio",
        "lm-studio",
        "vllm",
        "localai",
    ]
    .iter()
    .any(|hint| preset.contains(hint))
}

fn ai_models_endpoints(protocol: &str, base_url: &str) -> Vec<String> {
    let base = trim_trailing_slashes(base_url.trim());
    let lower = base.to_lowercase();
    let endpoints = match protocol {
        "anthropic" => {
            if lower.ends_with("/models") {
                vec![base]
            } else if lower.ends_with("/v1") {
                vec![format!("{base}/models")]
            } else {
                vec![format!("{base}/v1/models")]
            }
        }
        "gemini" => {
            if lower.ends_with("/models") {
                vec![base]
            } else if lower.ends_with("/v1") || lower.ends_with("/v1beta") {
                vec![format!("{base}/models")]
            } else {
                vec![format!("{base}/v1beta/models")]
            }
        }
        _ => {
            if lower.ends_with("/models") {
                vec![base]
            } else if lower.ends_with("/chat/completions") {
                vec![format!(
                    "{}/models",
                    base.trim_end_matches("/chat/completions")
                )]
            } else if lower.ends_with("/responses") {
                vec![format!("{}/models", base.trim_end_matches("/responses"))]
            } else if lower.ends_with("/v1") || lower.ends_with("/openai") {
                vec![format!("{base}/models")]
            } else {
                vec![format!("{base}/v1/models"), format!("{base}/models")]
            }
        }
    };
    let mut seen = std::collections::HashSet::new();
    endpoints
        .into_iter()
        .filter(|endpoint| seen.insert(endpoint.clone()))
        .collect()
}

fn extract_ai_model_descriptors(protocol: &str, value: &Value) -> Vec<Value> {
    let candidates = ["data", "models", "items", "results"];
    let Some(items) = value.as_array().or_else(|| {
        candidates
            .iter()
            .find_map(|key| value.get(*key).and_then(|v| v.as_array()))
    }) else {
        return Vec::new();
    };

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for item in items {
        let Some(id) = extract_ai_model_id(protocol, item) else {
            continue;
        };
        if !seen.insert(id.clone()) {
            continue;
        }
        let mut descriptor = serde_json::Map::new();
        descriptor.insert("id".into(), json!(id));
        if let Some(capabilities) = item.get("capabilities").and_then(|v| v.as_array()) {
            descriptor.insert("capabilities".into(), Value::Array(capabilities.clone()));
        }
        if let Some(display_name) = item
            .get("displayName")
            .or_else(|| item.get("display_name"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            descriptor.insert("displayName".into(), json!(display_name.trim()));
        }
        enrich_model_descriptor_from_payload(&mut descriptor, item);
        enrich_model_descriptor_from_canonical(protocol, &mut descriptor);
        out.push(Value::Object(descriptor));
    }
    out
}

fn enrich_model_descriptor_from_payload(
    descriptor: &mut serde_json::Map<String, Value>,
    item: &Value,
) {
    if let Some(value) = model_metadata_usize(
        item,
        &[
            "contextLimit",
            "context_limit",
            "contextLength",
            "context_length",
            "contextWindow",
            "context_window",
            "maxModelLen",
            "max_model_len",
            "maxInputTokens",
            "max_input_tokens",
            "inputTokenLimit",
            "input_token_limit",
        ],
    ) {
        descriptor.insert("contextLimit".into(), json!(value));
    }
    if let Some(value) = model_metadata_usize(
        item,
        &[
            "maxOutputTokens",
            "max_output_tokens",
            "outputTokenLimit",
            "output_token_limit",
            "maxCompletionTokens",
            "max_completion_tokens",
        ],
    ) {
        descriptor.insert("maxOutputTokens".into(), json!(value));
    }
    if let Some(value) = model_metadata_bool(
        item,
        &["reasoning", "supportsReasoning", "supports_reasoning"],
    ) {
        descriptor.insert("reasoning".into(), json!(value));
    }
    if let Some(value) = model_metadata_bool(
        item,
        &["toolCall", "tool_call", "supportsTools", "supports_tools"],
    ) {
        descriptor.insert("supportsTools".into(), json!(value));
    }
}

fn enrich_model_descriptor_from_canonical(
    protocol: &str,
    descriptor: &mut serde_json::Map<String, Value>,
) {
    let Some(model) = descriptor.get("id").and_then(Value::as_str) else {
        return;
    };
    let provider = match protocol {
        "anthropic" => "anthropic",
        "gemini" => "google",
        _ => "openai",
    };
    let Some(canonical) = goose::providers::canonical::maybe_get_canonical_model(provider, model)
    else {
        return;
    };
    descriptor
        .entry("contextLimit")
        .or_insert_with(|| json!(canonical.limit.context));
    if let Some(output) = canonical.limit.output {
        descriptor
            .entry("maxOutputTokens")
            .or_insert_with(|| json!(output));
    }
    if let Some(reasoning) = canonical.reasoning {
        descriptor
            .entry("reasoning")
            .or_insert_with(|| json!(reasoning));
    }
    descriptor
        .entry("supportsTools")
        .or_insert_with(|| json!(canonical.tool_call));
    if !canonical.modalities.input.is_empty() {
        descriptor.entry("inputModalities").or_insert_with(|| {
            serde_json::to_value(&canonical.modalities.input).unwrap_or_default()
        });
    }
    if !canonical.modalities.output.is_empty() {
        descriptor.entry("outputModalities").or_insert_with(|| {
            serde_json::to_value(&canonical.modalities.output).unwrap_or_default()
        });
    }
}

fn model_metadata_usize(value: &Value, keys: &[&str]) -> Option<usize> {
    for container in model_metadata_containers(value) {
        for key in keys {
            let Some(candidate) = container.get(*key) else {
                continue;
            };
            let parsed = candidate.as_u64().or_else(|| {
                candidate
                    .as_str()
                    .and_then(|text| text.trim().parse::<u64>().ok())
            });
            if let Some(parsed) = parsed.filter(|number| *number > 0) {
                if let Ok(parsed) = usize::try_from(parsed) {
                    return Some(parsed);
                }
            }
        }
    }
    None
}

fn model_metadata_bool(value: &Value, keys: &[&str]) -> Option<bool> {
    for container in model_metadata_containers(value) {
        for key in keys {
            if let Some(candidate) = container.get(*key) {
                if let Some(value) = candidate.as_bool() {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn model_metadata_containers(value: &Value) -> Vec<&serde_json::Map<String, Value>> {
    let Some(root) = value.as_object() else {
        return Vec::new();
    };
    let mut containers = vec![root];
    for key in [
        "model_info",
        "modelInfo",
        "metadata",
        "limits",
        "capabilities",
    ] {
        if let Some(nested) = root.get(key).and_then(Value::as_object) {
            containers.push(nested);
        }
    }
    containers
}

fn extract_ai_model_id(protocol: &str, item: &Value) -> Option<String> {
    if let Some(s) = item.as_str().map(str::trim).filter(|s| !s.is_empty()) {
        return Some(normalize_ai_model_id(protocol, s));
    }
    let obj = item.as_object()?;
    for key in ["id", "name", "model", "displayName", "display_name"] {
        if let Some(s) = obj
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return Some(normalize_ai_model_id(protocol, s));
        }
    }
    None
}

fn normalize_ai_model_id(protocol: &str, raw: &str) -> String {
    if protocol == "gemini" {
        raw.strip_prefix("models/").unwrap_or(raw).to_string()
    } else {
        raw.to_string()
    }
}

fn truncate_http_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= 300 {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(300).collect();
    out.push_str("...");
    out
}

/// `ai:roles:list` —— 内置角色清单（对齐 Beav `core/ai/roleRegistry.ts` 的 6 个 RoleSpec）。
fn ai_roles_list_impl() -> Value {
    json!([
        {
            "roleId": "planner",
            "purpose": "负责拆解目标、确定阶段顺序、把任务转成明确执行步骤。",
            "systemPrompt": "你是任务规划者，优先澄清目标、阶段、依赖和落盘动作，不要直接跳到模糊回答。",
            "allowedToolPack": "redclaw",
            "inputSchema": "目标、上下文、约束、历史项目状态",
            "outputSchema": "阶段计划、执行建议、关键依赖、保存策略",
            "handoffContract": "把任务拆成可执行步骤，并给出下一角色所需最小输入。",
            "artifactTypes": ["plan", "task-outline"],
        },
        {
            "roleId": "researcher",
            "purpose": "负责检索知识、提取证据、整理素材、形成研究摘要。",
            "systemPrompt": "你是研究代理，优先检索证据、阅读素材、提炼事实，不要在证据不足时强行下结论。",
            "allowedToolPack": "knowledge",
            "inputSchema": "问题、知识来源、素材、已有假设",
            "outputSchema": "证据摘要、引用来源、结论边界、待验证点",
            "handoffContract": "输出给写作者或评审时，必须包含证据、结论和不确定项。",
            "artifactTypes": ["research-note", "evidence-summary"],
        },
        {
            "roleId": "copywriter",
            "purpose": "负责产出标题、正文、发布话术、完整稿件和成品文案。",
            "systemPrompt": "你是写作代理，目标是生成可直接交付和落盘的内容，而不是停留在聊天草稿。",
            "allowedToolPack": "redclaw",
            "inputSchema": "目标、受众、策略、素材、证据",
            "outputSchema": "完整稿件、标题包、标签、发布建议",
            "handoffContract": "完成正文后必须准备保存路径或项目归档信息。",
            "artifactTypes": ["manuscript", "title-pack", "copy-pack"],
        },
        {
            "roleId": "image-director",
            "purpose": "负责封面、配图、海报、图片策略和视觉执行指令。",
            "systemPrompt": "你是图像策略代理，负责把目标转成可执行的配图/封面方案，并推动真实出图或落盘。",
            "allowedToolPack": "redclaw",
            "inputSchema": "内容目标、风格要求、参考素材、输出形式",
            "outputSchema": "封面策略、图片提示词、视觉结构、保存方案",
            "handoffContract": "给执行层的输出必须是可以直接生成或保存的结构化内容。",
            "artifactTypes": ["image-plan", "cover-plan", "image-pack"],
        },
        {
            "roleId": "reviewer",
            "purpose": "负责校验结果是否符合需求、是否保存、是否存在幻觉或遗漏。",
            "systemPrompt": "你是质量评审代理，优先检查结果是否满足需求、是否真实落盘、是否存在伪成功。",
            "allowedToolPack": "redclaw",
            "inputSchema": "目标、执行结果、工具回执、产物路径",
            "outputSchema": "评审结论、问题列表、修正建议",
            "handoffContract": "如果结果不满足交付条件，明确指出缺口并阻止宣称成功。",
            "artifactTypes": ["review-report"],
        },
        {
            "roleId": "ops-coordinator",
            "purpose": "负责后台任务、自动化、记忆维护和持续执行任务的推进。",
            "systemPrompt": "你是运行协调代理，负责长任务推进、自动化配置、状态检查、恢复和后台维护。",
            "allowedToolPack": "redclaw",
            "inputSchema": "任务目标、调度需求、运行状态、失败原因",
            "outputSchema": "调度动作、运行状态、恢复策略、维护结论",
            "handoffContract": "输出必须明确包含下一步执行条件与当前状态。",
            "artifactTypes": ["automation-config", "ops-report"],
        },
    ])
}

// ---------------------------------------------------------------------------
// 共享助手
// ---------------------------------------------------------------------------

/// 当前毫秒时间戳（`std::time::SystemTime`）。
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 幂等建 `knowledge_vectors` 表（Rust schema 尚未包含此表；与 knowledge.rs 对齐）。
fn ensure_knowledge_vectors_table(db: &Db) -> anyhow::Result<()> {
    db.execute_json(
        "CREATE TABLE IF NOT EXISTS knowledge_vectors (\
            id TEXT PRIMARY KEY,\
            source_id TEXT NOT NULL,\
            source_type TEXT NOT NULL DEFAULT 'file',\
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

/// 是否真正执行写操作（对齐 knowledge.rs 规则）：
/// 非对象 payload（裸位置参数）→ 执行；`dryRun:true` → 否；`confirm:true` → 是；其余默认否。
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

/// 当前活动空间 id（settings.active_space_id，缺省 `default`）。
fn active_space_id(db: &Db) -> String {
    db.settings()
        .get()
        .ok()
        .and_then(|s| {
            s.get("active_space_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string())
}

/// 工作区目录（settings.workspace_dir，缺省 `~/.yunying/workspace`）。
fn workspace_dir(db: &Db) -> std::path::PathBuf {
    let from_settings = db
        .settings()
        .get()
        .ok()
        .and_then(|s| {
            s.get("workspace_dir")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .filter(|s| !s.is_empty());
    if let Some(dir) = from_settings {
        return std::path::PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".yunying/workspace")
}

/// `knowledge_vectors` 总行数与去重 source_id 数。
fn vector_counts(db: &Db) -> anyhow::Result<(i64, i64)> {
    let total = db
        .query_one_json("SELECT COUNT(*) AS c FROM knowledge_vectors", &[])?
        .and_then(|r| r.get("c").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    let docs = db
        .query_one_json(
            "SELECT COUNT(DISTINCT source_id) AS c FROM knowledge_vectors",
            &[],
        )?
        .and_then(|r| r.get("c").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    Ok((total, docs))
}

/// 解析 metadata JSON 字符串为对象（失败返回 Null）。
fn parse_metadata(raw: Option<&str>) -> Value {
    raw.and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(|v| v.is_object())
        .unwrap_or(Value::Null)
}

/// 向量是否属于当前空间（对齐 Beav `isVectorInSpace`）：metadata.spaceId 缺省 → 仅 default 空间。
fn is_vector_in_space(metadata: &Value, active: &str) -> bool {
    match metadata.get("spaceId").and_then(|v| v.as_str()) {
        None => active == "default",
        Some(s) => s == active,
    }
}

/// 余弦相似度（维度不一致或零向量返回 0）。
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// 从 payload 提取查询向量（兼容裸数组 `{embedding:[...]}` 与 `[...]`）。返回 f32。
fn payload_embedding_vec(payload: &Value) -> Vec<f32> {
    let arr = payload
        .get("embedding")
        .and_then(|v| v.as_array())
        .or_else(|| payload.as_array());
    arr.map(|a| {
        a.iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect()
    })
    .unwrap_or_default()
}

/// 把 JSON 数组（数字）编码为 JSON 字符串（存入 `BLOB` 列）。空数组返回 `[]`。
fn encode_embedding(arr: &[Value]) -> anyhow::Result<String> {
    let floats: Vec<f32> = arr
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect();
    Ok(serde_json::to_string(&floats)?)
}

/// 把存储的 embedding 串解析为 `Vec<f32>`（非 JSON 数组 / 空 blob 返回 None）。
fn decode_embedding_f32(raw: &str) -> Option<Vec<f32>> {
    let t = raw.trim();
    if !t.starts_with('[') {
        return None;
    }
    let parsed: Vec<f64> = serde_json::from_str(t).ok()?;
    let v: Vec<f32> = parsed.into_iter().map(|x| x as f32).collect();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 把存储的 embedding 串解析为 `Vec<f64>`（用于返回给前端）。
fn decode_embedding_f64(raw: &str) -> Option<Vec<f64>> {
    let t = raw.trim();
    if !t.starts_with('[') {
        return None;
    }
    serde_json::from_str::<Vec<f64>>(t).ok()
}

/// 灵活取字符串字段：兼容裸字符串 payload 与 `{<key>: "..."}`。
fn flex_str(payload: &Value, key: &str) -> String {
    payload
        .as_str()
        .map(|s| s.to_string())
        .or_else(|| {
            payload
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// 自增 id 生成（lane/event）。
fn next_id(prefix: &str) -> String {
    let n = ID_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{n}-{}", now_ms())
}

/// 递增并返回知识库版本。
fn bump_knowledge_version() -> i64 {
    let v = now_ms();
    KNOWLEDGE_VERSION.store(v, Ordering::Relaxed);
    v
}

/// 扫描工作区所有知识条目（redbook / youtube / advisors），返回可索引总数。
async fn scan_all_knowledge_items(workspace: &std::path::Path) -> i64 {
    let mut total: i64 = 0;
    // (1) 知识红书：knowledge/redbook/*/meta.json
    total += count_meta_dirs(&workspace.join("knowledge/redbook")).await;
    // (2) 知识 YouTube：knowledge/youtube/*/meta.json
    total += count_meta_dirs(&workspace.join("knowledge/youtube")).await;
    // (3) Advisors：advisors/<id>/knowledge 下的 txt/md + config.json 的 videos
    let advisors_dir = workspace.join("advisors");
    if let Ok(mut entries) = tokio::fs::read_dir(&advisors_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                total += count_advisor_knowledge_items(
                    &advisors_dir.join(entry.file_name()).join("knowledge"),
                )
                .await;
            }
        }
    }
    total
}

/// 统计某目录下含 `meta.json` 的子目录数。
async fn count_meta_dirs(dir: &std::path::Path) -> i64 {
    let mut count: i64 = 0;
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return 0;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false)
            && tokio::fs::try_exists(entry.path().join("meta.json"))
                .await
                .unwrap_or(false)
        {
            count += 1;
        }
    }
    count
}

/// 统计 advisor knowledge 目录下的可索引条目（txt/md 文件 + config.json 中成功的视频字幕）。
async fn count_advisor_knowledge_items(knowledge_dir: &std::path::Path) -> i64 {
    let mut count: i64 = 0;
    if let Ok(mut entries) = tokio::fs::read_dir(knowledge_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Some(name) = entry.file_name().to_str() {
                if name.ends_with(".txt") || name.ends_with(".md") {
                    count += 1;
                }
            }
        }
    }
    // config.json 中 status==success 且有 subtitleFile 的视频
    let config_path = knowledge_dir.parent().map(|p| p.join("config.json"));
    if let Some(Some(cp)) = config_path.map(|p| if p.exists() { Some(p) } else { None }) {
        if let Ok(raw) = tokio::fs::read_to_string(&cp).await {
            if let Ok(cfg) = serde_json::from_str::<Value>(&raw) {
                if let Some(videos) = cfg.get("videos").and_then(|v| v.as_array()) {
                    count += videos
                        .iter()
                        .filter(|v| {
                            v.get("status").and_then(|x| x.as_str()) == Some("success")
                                && v.get("subtitleFile").is_some()
                        })
                        .count() as i64;
                }
            }
        }
    }
    count
}

/// AI 协议启发式（对齐 Beav `detectAiProtocol`）：显式 > presetId > baseURL host/path 提示 > openai。
fn detect_ai_protocol(payload: &Value) -> String {
    let protocol = payload
        .get("protocol")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if protocol == "anthropic" || protocol == "gemini" || protocol == "openai" {
        return protocol;
    }
    let preset = payload
        .get("presetId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if preset == "anthropic" {
        return "anthropic".into();
    }
    if preset == "gemini" || preset == "google" {
        return "gemini".into();
    }
    let base = trim_trailing_slashes(
        &payload
            .get("baseURL")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_lowercase(),
    );
    if base.contains("/anthropic") || base.contains("anthropic.com") {
        return "anthropic".into();
    }
    // Gemini OpenAI 兼容端点视为 openai
    if base.contains("/openai") || base.contains("/compatible-mode") {
        return "openai".into();
    }
    let gemini_hints = [
        "generativelanguage.googleapis.com",
        "aiplatform.googleapis.com",
        "googleapis.com",
    ];
    if gemini_hints.iter().any(|h| base.contains(h)) && !base.contains("compatible-mode") {
        return "gemini".into();
    }
    "openai".into()
}

/// 去除尾部斜杠。
fn trim_trailing_slashes(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{AppState, NoopEmitter};
    use std::sync::Arc;

    /// 测试用 AppState（内存库 + NoopEmitter）。
    fn test_state() -> AppState {
        let db = Db::open_in_memory().unwrap();
        AppState {
            redclaw_scheduler: crate::ipc::redclaw_runner::RedClawScheduler::inactive(db.clone()),
            db,
            goose: crate::goose_bridge::GooseBridge::default(),
            emitter: Arc::new(NoopEmitter),
            login: Arc::new(crate::login::LoginService::new(Arc::new(
                crate::login::StubLoginDriver,
            ))),
        }
    }

    #[test]
    fn manuscript_embedding_cache_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        // 缺省 dry_run → 不落库
        let save_dry = embedding_save_manuscript_cache_impl(
            &db,
            &json!({
                "filePath": "/a.md",
                "contentHash": "hash1",
                "embedding": [0.1, 0.2, 0.3],
            }),
        )
        .unwrap();
        assert_eq!(save_dry["dryRun"], json!(true));
        assert!(
            embedding_get_manuscript_cache_impl(&db, &json!("/a.md")).unwrap()["cached"].is_null()
        );

        // confirm:true → 真正写入
        let save = embedding_save_manuscript_cache_impl(
            &db,
            &json!({
                "filePath": "/a.md",
                "contentHash": "hash1",
                "embedding": [0.1, 0.2, 0.3],
                "confirm": true,
            }),
        )
        .unwrap();
        assert_eq!(save["success"], json!(true));

        let got = embedding_get_manuscript_cache_impl(&db, &json!("/a.md")).unwrap();
        assert_eq!(got["success"], json!(true));
        assert_eq!(got["cached"]["contentHash"], json!("hash1"));
        let emb = got["cached"]["embedding"].as_array().unwrap();
        assert_eq!(emb.len(), 3);
        assert!((emb[0].as_f64().unwrap() - 0.1).abs() < 1e-5);
    }

    #[test]
    fn similarity_cache_and_knowledge_version() {
        let db = Db::open_in_memory().unwrap();
        let v0 = KNOWLEDGE_VERSION.load(Ordering::Relaxed);
        let _ = bump_knowledge_version();
        assert!(KNOWLEDGE_VERSION.load(Ordering::Relaxed) >= v0);

        let save = similarity_save_cache_impl(
            &db,
            &json!({
                "manuscriptId": "m1",
                "contentHash": "h",
                "knowledgeVersion": 42,
                "sortedIds": ["s1", "s2"],
                "confirm": true,
            }),
        )
        .unwrap();
        assert_eq!(save["success"], json!(true));

        let got = similarity_get_cache_impl(&db, &json!("m1")).unwrap();
        assert_eq!(got["success"], json!(true));
        assert_eq!(got["cache"]["knowledgeVersion"], json!(42));
        assert_eq!(got["cache"]["sortedIds"], json!(["s1", "s2"]));
        assert_eq!(got["cache"]["sortedIds"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn sorted_sources_cosine_ranking() {
        let db = Db::open_in_memory().unwrap();
        ensure_knowledge_vectors_table(&db).unwrap();
        // 两个 source，A 与查询几乎相同，B 正交
        let a = serde_json::to_string(&vec![1.0f32, 0.0, 0.0]).unwrap();
        let b = serde_json::to_string(&vec![0.0f32, 1.0, 0.0]).unwrap();
        db.execute_json(
            "INSERT INTO knowledge_vectors (id, source_id, source_type, embedding, metadata) \
             VALUES ('v1','srcA','file',?1,'{}')",
            &[json!(a)],
        )
        .unwrap();
        db.execute_json(
            "INSERT INTO knowledge_vectors (id, source_id, source_type, embedding, metadata) \
             VALUES ('v2','srcB','file',?1,'{}')",
            &[json!(b)],
        )
        .unwrap();

        let res = embedding_get_sorted_sources_impl(&db, &json!({ "embedding": [1.0, 0.0, 0.0] }))
            .unwrap();
        let sorted = res["sorted"].as_array().unwrap();
        assert_eq!(sorted.len(), 2);
        assert_eq!(sorted[0]["sourceId"], json!("srcA"));
        assert!(sorted[0]["score"].as_f64().unwrap() > 0.99);
        assert!(sorted[0]["score"].as_f64().unwrap() > sorted[1]["score"].as_f64().unwrap());
    }

    #[test]
    fn indexing_stats_remove_clear() {
        let db = Db::open_in_memory().unwrap();
        // 插入 2 条 lane，一条 done 一条 queued
        db.execute_json(
            "INSERT INTO file_index_lanes (id, scope_id, lane, status, done, total, failed) \
             VALUES ('l1','s1','queue','done',3,3,0)",
            &[],
        )
        .unwrap();
        db.execute_json(
            "INSERT INTO file_index_lanes (id, scope_id, lane, status, done, total, failed) \
             VALUES ('l2','s1','queue','queued',0,5,0)",
            &[],
        )
        .unwrap();
        let stats = indexing_get_stats_impl(&db).unwrap();
        assert_eq!(stats["lanes"]["count"], json!(2));
        assert_eq!(stats["lanes"]["total"], json!(8));

        // remove-item（confirm）
        let rm =
            indexing_remove_item_impl(&db, &json!({ "itemId": "l2", "confirm": true })).unwrap();
        assert_eq!(rm["removed"], json!(1));
        assert_eq!(
            indexing_get_stats_impl(&db).unwrap()["lanes"]["count"],
            json!(1)
        );

        // clear-queue（confirm）— 剩余 l1 为 done，不被清
        let cleared = indexing_clear_queue_impl(&db, &json!({ "confirm": true })).unwrap();
        assert_eq!(cleared["cleared"], json!(0));
    }

    #[test]
    fn detect_protocol_variants() {
        assert_eq!(
            detect_ai_protocol(&json!({ "protocol": "anthropic" })),
            "anthropic"
        );
        assert_eq!(
            detect_ai_protocol(&json!({ "protocol": "gemini" })),
            "gemini"
        );
        assert_eq!(
            detect_ai_protocol(&json!({ "presetId": "google" })),
            "gemini"
        );
        assert_eq!(
            detect_ai_protocol(&json!({ "baseURL": "https://api.anthropic.com/v1" })),
            "anthropic"
        );
        assert_eq!(
            detect_ai_protocol(
                &json!({ "baseURL": "https://generativelanguage.googleapis.com/v1beta" })
            ),
            "gemini"
        );
        // Gemini OpenAI 兼容端点 → openai
        assert_eq!(
            detect_ai_protocol(
                &json!({ "baseURL": "https://generativelanguage.googleapis.com/v1beta/openai" })
            ),
            "openai"
        );
        // 默认 openai
        assert_eq!(
            detect_ai_protocol(&json!({ "baseURL": "https://api.example.com/v1" })),
            "openai"
        );
    }

    #[test]
    fn openai_models_use_v1_first_and_include_canonical_limits() {
        assert_eq!(
            ai_models_endpoints("openai", "https://api.example.com"),
            vec![
                "https://api.example.com/v1/models".to_string(),
                "https://api.example.com/models".to_string(),
            ]
        );
        let models =
            extract_ai_model_descriptors("openai", &json!({ "data": [{ "id": "gpt-5.5" }] }));
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["contextLimit"], json!(1_050_000));
        assert_eq!(models[0]["maxOutputTokens"], json!(128_000));
        assert_eq!(models[0]["reasoning"], json!(true));
    }

    #[test]
    fn roles_list_returns_six_specs() {
        let roles = ai_roles_list_impl();
        let arr = roles.as_array().unwrap();
        assert_eq!(arr.len(), 6);
        assert!(arr
            .iter()
            .all(|r| r.get("roleId").is_some() && r.get("systemPrompt").is_some()));
    }

    #[test]
    fn should_execute_rules() {
        assert!(should_execute(&json!("裸字符串")));
        assert!(!should_execute(&json!({ "dryRun": true })));
        assert!(should_execute(&json!({ "dryRun": false })));
        assert!(should_execute(&json!({ "confirm": true })));
        assert!(!should_execute(&json!({})));
    }

    // ---- 需真实网络 / 子进程 / 文件系统布局的用例 ----

    #[tokio::test]
    #[ignore = "需 embedding 服务"]
    async fn embedding_compute_is_stub() {
        let state = test_state();
        let res = invoke("embedding:compute", json!("hello"), &state)
            .await
            .unwrap();
        assert_eq!(res["success"], json!(false));
    }

    #[tokio::test]
    #[ignore = "需真实工作区目录与 embedding 服务"]
    async fn rebuild_all_creates_lane() {
        let state = test_state();
        let res = invoke("indexing:rebuild-all", json!({ "confirm": true }), &state)
            .await
            .unwrap();
        assert_eq!(res["success"], json!(true));
        assert!(res.get("lane").is_some());
    }

    #[tokio::test]
    #[ignore = "依赖环境中的 yt-dlp / pip"]
    async fn youtube_check_ytdlp_shape() {
        let res = youtube_check_ytdlp_impl().await.unwrap();
        assert!(res.get("installed").is_some());
    }

    #[tokio::test]
    #[ignore = "需真实 HTTP 客户端"]
    async fn ai_fetch_models_needs_http() {
        let res = ai_fetch_models_impl(&json!({ "baseURL": "https://api.example.com/v1" })).await;
        assert!(res.is_err());
    }
}
