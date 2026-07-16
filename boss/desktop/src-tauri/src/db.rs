use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Local, Utc};
use directories::ProjectDirs;
use rusqlite::{params, Connection};

use crate::error::{BossError, BossResult};

#[derive(Clone)]
pub struct BossCore {
    pub(crate) connection: Arc<Mutex<Connection>>,
    pub(crate) data_dir: Arc<PathBuf>,
}

impl BossCore {
    pub fn open(path: impl AsRef<Path>) -> BossResult<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        init_database(&mut connection)?;
        let data_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            data_dir: Arc::new(data_dir),
        })
    }

    pub fn open_default() -> BossResult<Self> {
        let path = database_path()?;
        Self::open(path)
    }

    pub fn data_dir(&self) -> &Path {
        self.data_dir.as_ref()
    }

    pub(crate) fn lock(&self) -> BossResult<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| BossError::State("数据库连接锁已损坏".to_string()))
    }
}

pub fn database_path() -> BossResult<PathBuf> {
    if let Some(path) = std::env::var_os("BOSS_ACCOUNTING_DB").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let project = ProjectDirs::from("com", "AgentsCompany", "BossAgent")
        .ok_or_else(|| BossError::State("无法解析系统应用数据目录".to_string()))?;
    Ok(project.data_local_dir().join("boss-accounting.sqlite"))
}

fn init_database(connection: &mut Connection) -> BossResult<()> {
    connection.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS owners (
            id TEXT PRIMARY KEY,
            email TEXT NOT NULL,
            owner_code TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS employees (
            id TEXT PRIMARY KEY,
            owner_code TEXT NOT NULL,
            name TEXT NOT NULL,
            role TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('active', 'blocked', 'idle')),
            quality INTEGER NOT NULL CHECK (quality BETWEEN 0 AND 100),
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS employees_owner_idx ON employees(owner_code);

        CREATE TABLE IF NOT EXISTS work_events (
            id TEXT PRIMARY KEY,
            owner_code TEXT NOT NULL,
            employee_id TEXT NOT NULL,
            event_date TEXT NOT NULL,
            task_type TEXT NOT NULL,
            material_count INTEGER NOT NULL DEFAULT 0 CHECK (material_count >= 0),
            cost_cents INTEGER NOT NULL DEFAULT 0 CHECK (cost_cents >= 0),
            quality_score INTEGER NOT NULL CHECK (quality_score BETWEEN 0 AND 100),
            summary TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT '',
            updated_at TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS work_events_owner_date_idx
            ON work_events(owner_code, event_date, employee_id);

        CREATE TABLE IF NOT EXISTS ledger_transactions (
            id TEXT PRIMARY KEY,
            owner_code TEXT NOT NULL,
            tx_date TEXT NOT NULL,
            type TEXT NOT NULL CHECK (type IN ('income', 'expense')),
            category TEXT NOT NULL,
            counterparty TEXT NOT NULL DEFAULT '',
            amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
            currency TEXT NOT NULL DEFAULT 'CNY' CHECK (currency = 'CNY'),
            source TEXT NOT NULL DEFAULT '',
            evidence_path TEXT NOT NULL DEFAULT '',
            status TEXT NOT NULL DEFAULT 'needs_review'
                CHECK (status IN ('needs_review', 'posted', 'synced')),
            review_note TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS ledger_owner_date_idx
            ON ledger_transactions(owner_code, tx_date, created_at);
        CREATE INDEX IF NOT EXISTS ledger_invoice_source_idx
            ON ledger_transactions(owner_code, source);

        CREATE TABLE IF NOT EXISTS invoice_drafts (
            id TEXT PRIMARY KEY,
            owner_code TEXT NOT NULL,
            file_path TEXT NOT NULL DEFAULT '',
            content_hash TEXT NOT NULL DEFAULT '',
            business_key TEXT NOT NULL DEFAULT '',
            fields_json TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS actual_sync_jobs (
            id TEXT PRIMARY KEY,
            owner_code TEXT NOT NULL,
            transaction_id TEXT NOT NULL,
            status TEXT NOT NULL,
            request_json TEXT NOT NULL,
            response_json TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS actual_jobs_owner_idx
            ON actual_sync_jobs(owner_code, created_at);
        CREATE INDEX IF NOT EXISTS actual_jobs_transaction_idx
            ON actual_sync_jobs(owner_code, transaction_id, status);

        CREATE TABLE IF NOT EXISTS week_reviews (
            id TEXT PRIMARY KEY,
            owner_code TEXT NOT NULL,
            employee_id TEXT NOT NULL,
            week_start TEXT NOT NULL,
            week_end TEXT NOT NULL,
            status TEXT NOT NULL CHECK (status IN ('pending', 'reviewed', 'needs_supplement')),
            note TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            UNIQUE(owner_code, employee_id, week_start)
        );
        CREATE INDEX IF NOT EXISTS week_reviews_employee_idx
            ON week_reviews(owner_code, employee_id, week_start);
        "#,
    )?;
    ensure_column(
        connection,
        "employees",
        "updated_at",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        connection,
        "work_events",
        "created_at",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        connection,
        "work_events",
        "updated_at",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        connection,
        "ledger_transactions",
        "review_note",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        connection,
        "invoice_drafts",
        "content_hash",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    ensure_column(
        connection,
        "invoice_drafts",
        "business_key",
        "TEXT NOT NULL DEFAULT ''",
    )?;
    connection.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS invoice_owner_hash_idx
            ON invoice_drafts(owner_code, content_hash) WHERE content_hash <> '';
        CREATE INDEX IF NOT EXISTS invoice_owner_business_idx
            ON invoice_drafts(owner_code, business_key) WHERE business_key <> '';
        "#,
    )?;
    seed_demo(connection)?;
    Ok(())
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> BossResult<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|name| name == column) {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn seed_demo(connection: &mut Connection) -> BossResult<()> {
    let count: i64 = connection.query_row("SELECT COUNT(*) FROM owners", [], |row| row.get(0))?;
    if count > 0 {
        return Ok(());
    }
    let transaction = connection.transaction()?;
    let now = Utc::now().to_rfc3339();
    let today = Local::now().date_naive();
    transaction.execute(
        "INSERT INTO owners (id, email, owner_code, created_at) VALUES (?1, ?2, ?3, ?4)",
        params!["owner-demo", "boss@demo.local", "BOSS-7429", now],
    )?;
    let employees = [
        ("emp-001", "林倩", "小红书素材与评论", "active", 93_i64),
        ("emp-002", "周祺", "视频脚本与封面", "blocked", 86_i64),
        ("emp-003", "赵敏", "店铺客服与私信", "active", 91_i64),
        ("emp-004", "陈越", "投放与数据复盘", "idle", 78_i64),
    ];
    for (id, name, role, status, quality) in employees {
        transaction.execute(
            "INSERT INTO employees
             (id, owner_code, name, role, status, quality, created_at, updated_at)
             VALUES (?1, 'BOSS-7429', ?2, ?3, ?4, ?5, ?6, ?6)",
            params![id, name, role, status, quality, now],
        )?;
    }
    let events = [
        (
            "evt-001",
            "emp-001",
            "comment_archive",
            64_i64,
            32_800_i64,
            93_i64,
            "完成 6 条竞品评论归档，3 条进入选题池",
        ),
        (
            "evt-002",
            "emp-002",
            "cover_script",
            31,
            51_200,
            86,
            "封面 A/B 版本等待老板确认",
        ),
        (
            "evt-003",
            "emp-003",
            "store_support",
            17,
            9_600,
            91,
            "蒲公英合作咨询已分级，2 条需要商务跟进",
        ),
        (
            "evt-004",
            "emp-004",
            "ad_report",
            9,
            18_400,
            78,
            "上午投放日报缺少转化截图",
        ),
    ];
    for (id, employee_id, task_type, material_count, cost_cents, quality, summary) in events {
        transaction.execute(
            "INSERT INTO work_events
             (id, owner_code, employee_id, event_date, task_type, material_count,
              cost_cents, quality_score, summary, created_at, updated_at)
             VALUES (?1, 'BOSS-7429', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![
                id,
                employee_id,
                today.to_string(),
                task_type,
                material_count,
                cost_cents,
                quality,
                summary,
                now
            ],
        )?;
    }
    let yesterday = today - ChronoDuration::days(1);
    let ledger = [
        (
            "txn-1001",
            today,
            "income",
            "客户回款",
            "杭州某品牌店铺",
            1_280_000_i64,
            "合同回款截图",
            "posted",
        ),
        (
            "txn-1002",
            today,
            "expense",
            "达人样品",
            "供应链样品费",
            188_000_i64,
            "发票识别",
            "needs_review",
        ),
        (
            "txn-1003",
            yesterday,
            "expense",
            "AI 工具",
            "模型 API 充值",
            60_000_i64,
            "手动录入",
            "posted",
        ),
        (
            "txn-1004",
            yesterday,
            "expense",
            "素材采集",
            "第三方数据接口",
            42_000_i64,
            "工具同步",
            "posted",
        ),
    ];
    for (id, date, kind, category, counterparty, cents, source, status) in ledger {
        transaction.execute(
            "INSERT INTO ledger_transactions
             (id, owner_code, tx_date, type, category, counterparty, amount_cents,
              currency, source, evidence_path, status, review_note, created_at)
             VALUES (?1, 'BOSS-7429', ?2, ?3, ?4, ?5, ?6, 'CNY', ?7, '', ?8, '', ?9)",
            params![
                id,
                date.to_string(),
                kind,
                category,
                counterparty,
                cents,
                source,
                status,
                now
            ],
        )?;
    }
    transaction.commit()?;
    Ok(())
}
