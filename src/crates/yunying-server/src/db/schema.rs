//! SQLite schema，1:1 移植自 `Beav/desktop/electron/db.ts` 的 `initDb()`。
//!
//! 数据库文件 `redconvert.db`（与原 Beav 兼容，可在 userData 目录复用）。
//! v1 包含聊天接 goose 所需的核心表；manuscripts/knowledge/wander 等扩展表后续补全。

/// 建表 SQL（`CREATE TABLE IF NOT EXISTS`，幂等）。
pub const CORE_SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  api_endpoint TEXT,
  api_key TEXT,
  model_name TEXT,
  workspace_dir TEXT,
  active_space_id TEXT,
  ai_sources_json TEXT,
  default_ai_source_id TEXT,
  image_provider TEXT,
  image_endpoint TEXT,
  image_api_key TEXT,
  image_model TEXT,
  video_endpoint TEXT,
  video_api_key TEXT,
  video_model TEXT,
  image_provider_template TEXT,
  image_aspect_ratio TEXT,
  image_size TEXT,
  image_quality TEXT,
  mcp_servers_json TEXT,
  social_tools_json TEXT,
  proxy_enabled INTEGER,
  proxy_url TEXT,
  role_mapping TEXT,
  chat_max_tokens_default INTEGER,
  chat_max_tokens_deepseek INTEGER,
  transcription_model TEXT,
  transcription_endpoint TEXT,
  transcription_key TEXT
);

CREATE TABLE IF NOT EXISTS spaces (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS chat_sessions (
  id TEXT PRIMARY KEY,
  title TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  metadata TEXT
);

CREATE TABLE IF NOT EXISTS chat_messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  display_content TEXT,
  attachment TEXT,
  tool_calls TEXT,
  tool_call_id TEXT,
  timestamp INTEGER NOT NULL,
  FOREIGN KEY (session_id) REFERENCES chat_sessions(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_chat_messages_session_id ON chat_messages(session_id);
CREATE INDEX IF NOT EXISTS idx_chat_messages_timestamp ON chat_messages(timestamp);

CREATE TABLE IF NOT EXISTS session_transcript_records (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  record_type TEXT NOT NULL,
  role TEXT,
  content TEXT,
  payload_json TEXT,
  created_at INTEGER NOT NULL,
  FOREIGN KEY (session_id) REFERENCES chat_sessions(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_session_transcript_records_session
  ON session_transcript_records(session_id, created_at ASC);

CREATE TABLE IF NOT EXISTS session_tool_results (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  call_id TEXT NOT NULL,
  tool_name TEXT NOT NULL,
  command TEXT,
  success INTEGER NOT NULL DEFAULT 1,
  result_text TEXT,
  summary_text TEXT,
  payload_json TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  FOREIGN KEY (session_id) REFERENCES chat_sessions(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_session_tool_results_session
  ON session_tool_results(session_id, created_at ASC);

CREATE TABLE IF NOT EXISTS user_memories (
  id TEXT PRIMARY KEY,
  space_id TEXT NOT NULL DEFAULT 'default',
  content TEXT NOT NULL,
  type TEXT DEFAULT 'general',
  tags TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_accessed INTEGER
);
CREATE INDEX IF NOT EXISTS idx_memories_created_at ON user_memories(created_at);

CREATE TABLE IF NOT EXISTS agent_tasks (
  id TEXT PRIMARY KEY,
  task_type TEXT NOT NULL,
  status TEXT NOT NULL,
  runtime_mode TEXT NOT NULL,
  owner_session_id TEXT,
  intent TEXT,
  role_id TEXT,
  goal TEXT,
  current_node TEXT,
  route_json TEXT,
  graph_json TEXT,
  artifacts_json TEXT,
  checkpoints_json TEXT,
  metadata_json TEXT,
  last_error TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  started_at INTEGER,
  completed_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_agent_tasks_status_updated ON agent_tasks(status, updated_at DESC);

CREATE TABLE IF NOT EXISTS agent_task_traces (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  node_id TEXT,
  event_type TEXT NOT NULL,
  payload_json TEXT,
  created_at INTEGER NOT NULL,
  FOREIGN KEY (task_id) REFERENCES agent_tasks(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_agent_task_traces_task ON agent_task_traces(task_id, created_at ASC);

CREATE TABLE IF NOT EXISTS archive_profiles (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  platform TEXT,
  goal TEXT,
  domain TEXT,
  audience TEXT,
  tone_tags TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS archive_samples (
  id TEXT PRIMARY KEY,
  profile_id TEXT NOT NULL,
  title TEXT,
  content TEXT,
  excerpt TEXT,
  tags TEXT,
  images TEXT,
  platform TEXT,
  source_url TEXT,
  sample_date TEXT,
  is_featured INTEGER DEFAULT 0,
  created_at INTEGER NOT NULL,
  FOREIGN KEY (profile_id) REFERENCES archive_profiles(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_archive_samples_profile_id ON archive_samples(profile_id);
CREATE INDEX IF NOT EXISTS idx_archive_samples_created_at ON archive_samples(created_at);

CREATE TABLE IF NOT EXISTS file_index_lanes (
  id TEXT PRIMARY KEY,
  scope_id TEXT NOT NULL,
  lane TEXT NOT NULL,
  status TEXT DEFAULT 'idle',
  done INTEGER DEFAULT 0,
  total INTEGER DEFAULT 0,
  failed INTEGER DEFAULT 0,
  metadata_only INTEGER DEFAULT 0,
  last_updated_at TEXT,
  next_retry_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_file_index_lanes_scope ON file_index_lanes(scope_id);

CREATE TABLE IF NOT EXISTS file_index_events (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  details TEXT,
  created_at TEXT DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_file_index_events_source ON file_index_events(source_id);

CREATE TABLE IF NOT EXISTS manuscript_embeddings (
  file_path TEXT PRIMARY KEY,
  content_hash TEXT NOT NULL,
  embedding BLOB NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS manuscript_similarity_cache (
  manuscript_id TEXT PRIMARY KEY,
  content_hash TEXT NOT NULL,
  knowledge_version INTEGER NOT NULL,
  sorted_ids TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS document_knowledge_index (
  space_id TEXT NOT NULL,
  source_id TEXT NOT NULL,
  absolute_path TEXT NOT NULL,
  relative_path TEXT NOT NULL,
  title TEXT,
  file_size INTEGER NOT NULL DEFAULT 0,
  mtime_ms INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (space_id, source_id, absolute_path)
);
CREATE INDEX IF NOT EXISTS idx_doc_index_space_source ON document_knowledge_index(space_id, source_id);

CREATE TABLE IF NOT EXISTS wander_history (
  id TEXT PRIMARY KEY,
  space_id TEXT NOT NULL DEFAULT 'default',
  items TEXT NOT NULL,
  result TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_wander_history_created_at ON wander_history(created_at);

-- 账号池：统一管理 mediacrawler(采集) + socialconnect(发布) 的平台账号绑定。
-- UI（设置页）经 ipc social-tools:* 通道读写；operations 工具读此处拿 cookie。
CREATE TABLE IF NOT EXISTS platform_accounts (
  id TEXT PRIMARY KEY,
  platform TEXT NOT NULL,
  profile TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'both',
  cookies TEXT,
  status TEXT NOT NULL DEFAULT 'idle',
  qr_image TEXT,
  error_message TEXT,
  last_login_at INTEGER,
  last_check_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_platform_accounts_platform ON platform_accounts(platform);
CREATE UNIQUE INDEX IF NOT EXISTS idx_platform_accounts_platform_profile ON platform_accounts(platform, profile);
"#;

/// 在 settings 表插入单例空行（id=1，active_space_id='default'），保证 get_settings 总有数据。
pub const ENSURE_SETTINGS_ROW: &str =
    "INSERT OR IGNORE INTO settings (id, active_space_id) VALUES (1, 'default');";
