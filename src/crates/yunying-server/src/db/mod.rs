//! SQLite 持久层（Repository），替代 Beav 的 `better-sqlite3` `electron/db.ts`。
//!
//! [`Db`] 持有 `parking_lot::Mutex<rusqlite::Connection>`，复用原 `redconvert.db` 文件
//! 与 schema（见 [`schema::CORE_SCHEMA`]）。v1 实现聊天接 goose 所需的核心仓库：
//! [`SettingsRepo`]、[`SpacesRepo`]、[`ChatRepo`]；其余表（knowledge/manuscripts/wander 等）
//! 后续按需补全。

pub mod schema;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// 数据库句柄。`Arc` 共享给 IPC handler / axum state。
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

impl Db {
    /// 打开（或创建）`redconvert.db`，执行 schema 迁移，确保单例 settings 行。
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(schema::CORE_SCHEMA)?;
        ensure_settings_columns(&conn)?;
        ensure_chat_message_columns(&conn)?;
        conn.execute_batch(schema::ENSURE_SETTINGS_ROW)?;
        // 确保 default space 存在。
        ensure_default_space(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 内存库（测试用）。
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(schema::CORE_SCHEMA)?;
        ensure_settings_columns(&conn)?;
        ensure_chat_message_columns(&conn)?;
        conn.execute_batch(schema::ENSURE_SETTINGS_ROW)?;
        ensure_default_space(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn settings(&self) -> SettingsRepo {
        SettingsRepo { db: self.clone() }
    }
    pub fn spaces(&self) -> SpacesRepo {
        SpacesRepo { db: self.clone() }
    }
    pub fn chat(&self) -> ChatRepo {
        ChatRepo { db: self.clone() }
    }

    /// 通用查询：返回每行为 JSON 对象（列名→值）。供命名空间 handler 灵活读写，无需逐表建 Repo。
    pub fn query_all_json(&self, sql: &str, params: &[Value]) -> anyhow::Result<Vec<Value>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(sql)?;
        let sql_params: Vec<rusqlite::types::Value> = params.iter().map(json_to_sql).collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(sql_params.iter()), row_to_json)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// 通用单行查询。
    pub fn query_one_json(&self, sql: &str, params: &[Value]) -> anyhow::Result<Option<Value>> {
        Ok(self.query_all_json(sql, params)?.into_iter().next())
    }

    /// 通用执行（INSERT/UPDATE/DELETE），返回受影响行数。
    pub fn execute_json(&self, sql: &str, params: &[Value]) -> anyhow::Result<usize> {
        let conn = self.conn.lock();
        let sql_params: Vec<rusqlite::types::Value> = params.iter().map(json_to_sql).collect();
        Ok(conn.execute(sql, rusqlite::params_from_iter(sql_params.iter()))?)
    }

    /// 在同一 SQLite 事务中依次执行多条参数化语句。
    ///
    /// 任一语句失败时全部回滚；返回值与输入顺序一致，供调用方检查 CAS 影响行数。
    pub fn execute_transaction_json(
        &self,
        statements: &[(String, Vec<Value>)],
    ) -> anyhow::Result<Vec<usize>> {
        let mut conn = self.conn.lock();
        let transaction = conn.transaction()?;
        let mut affected = Vec::with_capacity(statements.len());
        for (sql, params) in statements {
            let sql_params: Vec<rusqlite::types::Value> = params.iter().map(json_to_sql).collect();
            affected.push(transaction.execute(sql, rusqlite::params_from_iter(sql_params.iter()))?);
        }
        transaction.commit()?;
        Ok(affected)
    }
}

fn ensure_settings_columns(conn: &Connection) -> anyhow::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(settings)")?;
    let existing = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    for (name, sql_type) in [
        ("chat_max_tokens_default", "INTEGER"),
        ("chat_max_tokens_deepseek", "INTEGER"),
    ] {
        if !existing.contains(name) {
            conn.execute(
                &format!("ALTER TABLE settings ADD COLUMN {name} {sql_type}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn ensure_chat_message_columns(conn: &Connection) -> anyhow::Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(chat_messages)")?;
    let existing = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
    for (name, sql_type) in [("display_content", "TEXT"), ("attachment", "TEXT")] {
        if !existing.contains(name) {
            conn.execute(
                &format!("ALTER TABLE chat_messages ADD COLUMN {name} {sql_type}"),
                [],
            )?;
        }
    }
    Ok(())
}

/// serde_json::Value → rusqlite SQL 值。
fn json_to_sql(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        Value::Null => S::Null,
        Value::Bool(b) => S::Integer(*b as i64),
        Value::Number(n) => n
            .as_i64()
            .map(S::Integer)
            .or_else(|| n.as_f64().map(S::Real))
            .unwrap_or(S::Null),
        Value::String(s) => S::Text(s.clone()),
        other => S::Text(other.to_string()),
    }
}

/// 一行 → JSON 对象。
fn row_to_json(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    let count = row.as_ref().column_count();
    let mut obj = serde_json::Map::new();
    for i in 0..count {
        let name = row.as_ref().column_name(i)?.to_string();
        let val: rusqlite::types::Value = row.get(i)?;
        obj.insert(name, sql_value_to_json(val));
    }
    Ok(Value::Object(obj))
}

fn ensure_default_space(conn: &Connection) -> anyhow::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO spaces (id, name, created_at, updated_at) VALUES ('default', '默认空间', ?1, ?1)",
        rusqlite::params![now_ts()],
    )?;
    Ok(())
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// rusqlite SQL 值 → serde_json::Value。
fn sql_value_to_json(v: rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as V;
    match v {
        V::Null => Value::Null,
        V::Integer(i) => json!(i),
        V::Real(f) => json!(f),
        V::Text(s) => Value::String(s),
        V::Blob(b) => Value::String(format!("<blob {} bytes>", b.len())),
    }
}

// ---------------------------------------------------------------------------
// Settings（单例行，id=1）
// ---------------------------------------------------------------------------

pub struct SettingsRepo {
    db: Db,
}

impl SettingsRepo {
    /// 读取全部 settings 列为 JSON 对象（列名→值）。
    pub fn get(&self) -> anyhow::Result<Value> {
        let conn = self.db.conn.lock();
        let mut stmt = conn.prepare("SELECT * FROM settings WHERE id = 1")?;
        let mut rows = stmt.query([])?;
        let row = rows
            .next()?
            .ok_or_else(|| anyhow::anyhow!("settings 行不存在"))?;
        let col_count = row.as_ref().column_count();
        let mut obj = serde_json::Map::new();
        for i in 0..col_count {
            let name = row.as_ref().column_name(i)?.to_string();
            let val: rusqlite::types::Value = row.get(i)?;
            obj.insert(name, sql_value_to_json(val));
        }
        Ok(Value::Object(obj))
    }

    /// 按 JSON 对象的部分字段更新 settings（白名单列，防注入）。
    /// 仅更新对象中存在且属于已知列的键；值统一以 TEXT 存（与原 Beav 一致）。
    pub fn save(&self, patch: &Value) -> anyhow::Result<()> {
        let obj = patch
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("settings patch 必须是对象"))?;
        let allowed: std::collections::HashSet<&str> = SETTINGS_COLUMNS.iter().copied().collect();
        let mut sets: Vec<String> = Vec::new();
        let mut vals: Vec<String> = Vec::new();
        for (k, v) in obj {
            if !allowed.contains(k.as_str()) {
                continue;
            }
            sets.push(format!("{k} = ?{n}", n = sets.len() + 1));
            vals.push(match v {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                other => other.to_string(),
            });
        }
        if sets.is_empty() {
            return Ok(());
        }
        let sql = format!("UPDATE settings SET {} WHERE id = 1", sets.join(", "));
        let conn = self.db.conn.lock();
        // &String 实现 ToSql；params_from_iter 持有 vals 的迭代器。
        conn.execute(&sql, rusqlite::params_from_iter(vals.iter()))?;
        Ok(())
    }
}

/// settings 表已知可写列白名单。
const SETTINGS_COLUMNS: &[&str] = &[
    "api_endpoint",
    "api_key",
    "model_name",
    "workspace_dir",
    "active_space_id",
    "ai_sources_json",
    "default_ai_source_id",
    "image_provider",
    "image_endpoint",
    "image_api_key",
    "image_model",
    "video_endpoint",
    "video_api_key",
    "video_model",
    "image_provider_template",
    "image_aspect_ratio",
    "image_size",
    "image_quality",
    "mcp_servers_json",
    "social_tools_json",
    "proxy_enabled",
    "proxy_url",
    "role_mapping",
    "chat_max_tokens_default",
    "chat_max_tokens_deepseek",
    "transcription_model",
    "transcription_endpoint",
    "transcription_key",
];

// ---------------------------------------------------------------------------
// Spaces
// ---------------------------------------------------------------------------

pub struct SpacesRepo {
    db: Db,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl SpacesRepo {
    pub fn list(&self) -> anyhow::Result<Vec<Space>> {
        let conn = self.db.conn.lock();
        let mut stmt = conn
            .prepare("SELECT id, name, created_at, updated_at FROM spaces ORDER BY created_at")?;
        let rows = stmt.query_map([], |r| {
            Ok(Space {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn create(&self, id: &str, name: &str) -> anyhow::Result<Space> {
        let ts = now_ts();
        let conn = self.db.conn.lock();
        conn.execute(
            "INSERT INTO spaces (id, name, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            rusqlite::params![id, name, ts],
        )?;
        Ok(Space {
            id: id.into(),
            name: name.into(),
            created_at: ts,
            updated_at: ts,
        })
    }

    pub fn rename(&self, id: &str, name: &str) -> anyhow::Result<()> {
        let conn = self.db.conn.lock();
        conn.execute(
            "UPDATE spaces SET name = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![name, now_ts(), id],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Chat sessions / messages
// ---------------------------------------------------------------------------

pub struct ChatRepo {
    db: Db,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSession {
    pub id: String,
    pub title: Option<String>,
    #[serde(serialize_with = "serialize_timestamp_millis")]
    pub created_at: i64,
    #[serde(serialize_with = "serialize_timestamp_millis")]
    pub updated_at: i64,
    pub metadata: Option<String>,
}

fn serialize_timestamp_millis<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*value)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    serializer.serialize_str(&timestamp)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub display_content: Option<String>,
    pub attachment: Option<String>,
    pub tool_calls: Option<String>,
    pub tool_call_id: Option<String>,
    pub timestamp: i64,
}

impl ChatRepo {
    pub fn create_session(
        &self,
        id: &str,
        title: Option<&str>,
        metadata: Option<&str>,
    ) -> anyhow::Result<ChatSession> {
        let ts = now_ts();
        let conn = self.db.conn.lock();
        conn.execute(
            "INSERT INTO chat_sessions (id, title, created_at, updated_at, metadata) VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![id, title, ts, metadata],
        )?;
        Ok(ChatSession {
            id: id.into(),
            title: title.map(String::from),
            created_at: ts,
            updated_at: ts,
            metadata: metadata.map(String::from),
        })
    }

    pub fn list_sessions(&self) -> anyhow::Result<Vec<ChatSession>> {
        let conn = self.db.conn.lock();
        let mut stmt = conn.prepare("SELECT id, title, created_at, updated_at, metadata FROM chat_sessions ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |r| {
            Ok(ChatSession {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                metadata: r.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_session(&self, id: &str) -> anyhow::Result<Option<ChatSession>> {
        let conn = self.db.conn.lock();
        let s = conn
            .query_row(
                "SELECT id, title, created_at, updated_at, metadata FROM chat_sessions WHERE id = ?1",
                rusqlite::params![id],
                |r| {
                    Ok(ChatSession {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        created_at: r.get(2)?,
                        updated_at: r.get(3)?,
                        metadata: r.get(4)?,
                    })
                },
            )
            .optional()?;
        Ok(s)
    }

    pub fn update_session_metadata(&self, id: &str, metadata: &str) -> anyhow::Result<()> {
        let conn = self.db.conn.lock();
        conn.execute(
            "UPDATE chat_sessions SET metadata = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![metadata, now_ts(), id],
        )?;
        Ok(())
    }

    pub fn add_message(&self, msg: &ChatMessage) -> anyhow::Result<()> {
        let conn = self.db.conn.lock();
        conn.execute(
            "INSERT INTO chat_messages (id, session_id, role, content, display_content, attachment, tool_calls, tool_call_id, timestamp) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![msg.id, msg.session_id, msg.role, msg.content, msg.display_content, msg.attachment, msg.tool_calls, msg.tool_call_id, msg.timestamp],
        )?;
        conn.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![msg.timestamp, msg.session_id],
        )?;
        Ok(())
    }

    pub fn upsert_message(&self, msg: &ChatMessage) -> anyhow::Result<()> {
        let conn = self.db.conn.lock();
        conn.execute(
            "INSERT INTO chat_messages (id, session_id, role, content, display_content, attachment, tool_calls, tool_call_id, timestamp) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT(id) DO UPDATE SET \
               content = excluded.content, \
               display_content = excluded.display_content, \
               attachment = excluded.attachment, \
               tool_calls = excluded.tool_calls, \
               tool_call_id = excluded.tool_call_id, \
               timestamp = excluded.timestamp",
            rusqlite::params![msg.id, msg.session_id, msg.role, msg.content, msg.display_content, msg.attachment, msg.tool_calls, msg.tool_call_id, msg.timestamp],
        )?;
        conn.execute(
            "UPDATE chat_sessions SET updated_at = ?1 WHERE id = ?2",
            rusqlite::params![msg.timestamp, msg.session_id],
        )?;
        Ok(())
    }

    pub fn get_messages(&self, session_id: &str) -> anyhow::Result<Vec<ChatMessage>> {
        let conn = self.db.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, role, content, display_content, attachment, tool_calls, tool_call_id, timestamp \
             FROM chat_messages WHERE session_id = ?1 ORDER BY timestamp ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id], |r| {
            Ok(ChatMessage {
                id: r.get(0)?,
                session_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                display_content: r.get(4)?,
                attachment: r.get(5)?,
                tool_calls: r.get(6)?,
                tool_call_id: r.get(7)?,
                timestamp: r.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn delete_session(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.db.conn.lock();
        conn.execute(
            "DELETE FROM chat_sessions WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_get_save_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let s = db.settings();
        let initial = s.get().unwrap();
        assert_eq!(initial["id"], json!(1));

        s.save(&json!({ "model_name": "gpt-5.5", "api_endpoint": "https://x" }))
            .unwrap();
        s.save(&json!({ "chat_max_tokens_default": 128000 }))
            .unwrap();
        let after = s.get().unwrap();
        assert_eq!(after["model_name"], json!("gpt-5.5"));
        assert_eq!(after["api_endpoint"], json!("https://x"));
        assert_eq!(after["chat_max_tokens_default"], json!(128000));
    }

    #[test]
    fn spaces_list_contains_default() {
        let db = Db::open_in_memory().unwrap();
        let list = db.spaces().list().unwrap();
        assert!(list.iter().any(|s| s.id == "default"));
    }

    #[test]
    fn execute_transaction_json_rolls_back_every_statement_on_error() {
        let db = Db::open_in_memory().unwrap();
        db.execute_json("CREATE TABLE transaction_probe (id TEXT PRIMARY KEY)", &[])
            .unwrap();
        let result = db.execute_transaction_json(&[
            (
                "INSERT INTO transaction_probe (id) VALUES (?1)".into(),
                vec![json!("first")],
            ),
            (
                "INSERT INTO missing_transaction_table (id) VALUES (?1)".into(),
                vec![json!("second")],
            ),
        ]);
        assert!(result.is_err());
        let rows = db
            .query_all_json("SELECT id FROM transaction_probe", &[])
            .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn chat_session_messages_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        let chat = db.chat();
        chat.create_session("s1", Some("标题"), None).unwrap();
        let ts = now_ts();
        chat.add_message(&ChatMessage {
            id: "m1".into(),
            session_id: "s1".into(),
            role: "user".into(),
            content: "你好".into(),
            display_content: Some("你好".into()),
            attachment: Some("{\"name\":\"参考图.png\"}".into()),
            tool_calls: None,
            tool_call_id: None,
            timestamp: ts,
        })
        .unwrap();
        chat.add_message(&ChatMessage {
            id: "m2".into(),
            session_id: "s1".into(),
            role: "assistant".into(),
            content: "你好！".into(),
            display_content: None,
            attachment: None,
            tool_calls: None,
            tool_call_id: None,
            timestamp: ts + 1,
        })
        .unwrap();
        let msgs = chat.get_messages("s1").unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].display_content.as_deref(), Some("你好"));
        assert_eq!(
            msgs[0].attachment.as_deref(),
            Some("{\"name\":\"参考图.png\"}")
        );
        assert_eq!(msgs[1].content, "你好！");

        chat.upsert_message(&ChatMessage {
            id: "m2".into(),
            session_id: "s1".into(),
            role: "assistant".into(),
            content: "你好！这是流式更新。".into(),
            display_content: None,
            attachment: None,
            tool_calls: Some("[{\"name\":\"demo\"}]".into()),
            tool_call_id: None,
            timestamp: ts + 2,
        })
        .unwrap();
        let updated = chat.get_messages("s1").unwrap();
        assert_eq!(updated.len(), 2, "流式草稿更新不应插入重复消息");
        assert_eq!(updated[1].content, "你好！这是流式更新。");
        assert_eq!(
            updated[1].tool_calls.as_deref(),
            Some("[{\"name\":\"demo\"}]")
        );

        // 删除会话级联删消息
        chat.delete_session("s1").unwrap();
        assert!(chat.get_messages("s1").unwrap().is_empty());
    }
}
