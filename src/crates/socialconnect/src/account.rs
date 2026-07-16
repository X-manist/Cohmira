//! Social Connection 账号与登录态文件。
//!
//! 当前 `sau` 主线把每个账号保存为 Playwright/Patchright `storage_state`：
//! `cookies/<platform>_<profile>.json`，文件顶层是 `{ "cookies": [], "origins": [] }`。
//! 本模块同时兼容早期仅保存 cookie 数组的格式，以及 Bilibili/biliup 自己的 JSON 账号文件。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Error, ErrorKind};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 设置页与当前 `sau` CLI 共同支持的平台。
pub const SUPPORTED_PLATFORMS: &[&str] = &[
    "xiaohongshu",
    "douyin",
    "kuaishou",
    "bilibili",
    "tencent",
    "youtube",
];

/// 规整平台别名。返回 `None` 表示不是 Social Connection 当前支持的平台。
pub fn normalize_platform(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "xiaohongshu" | "xhs" | "rednote" => Some("xiaohongshu"),
        "douyin" | "dy" => Some("douyin"),
        "kuaishou" | "ks" => Some("kuaishou"),
        "bilibili" | "bili" | "b站" => Some("bilibili"),
        "tencent" | "weixin" | "wechat-channels" | "channels" => Some("tencent"),
        "youtube" | "yt" => Some("youtube"),
        _ => None,
    }
}

/// 校验用户自定义 profile。允许中文、空格、下划线、短横线和点，但拒绝路径成分。
pub fn validate_profile(raw: &str) -> std::io::Result<String> {
    let profile = raw.trim();
    let profile = if profile.is_empty() {
        "default"
    } else {
        profile
    };
    if profile == "."
        || profile == ".."
        || profile.contains('/')
        || profile.contains('\\')
        || profile.contains('\0')
    {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "账号 profile 不能包含路径分隔符或路径跳转",
        ));
    }
    Ok(profile.to_string())
}

/// 账号 profile：平台 + 本地账号名（非手机号/密码/昵称）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// 平台代号：douyin / xiaohongshu / kuaishou / bilibili / tencent / youtube。
    pub platform: String,
    /// 本地 profile 名，例如 `staff_xhs_01`。
    pub profile: String,
}

impl Account {
    pub fn new(platform: impl Into<String>, profile: impl Into<String>) -> Self {
        let platform = platform.into();
        let profile = profile.into();
        Self {
            platform: normalize_platform(&platform)
                .unwrap_or(platform.trim())
                .to_string(),
            profile: validate_profile(&profile).unwrap_or_else(|_| "default".to_string()),
        }
    }

    /// 严格构造，供 IPC/文件写操作使用。
    pub fn try_new(platform: &str, profile: &str) -> std::io::Result<Self> {
        let platform = normalize_platform(platform).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                format!("不支持的 Social Connection 平台: {platform}"),
            )
        })?;
        Ok(Self {
            platform: platform.to_string(),
            profile: validate_profile(profile)?,
        })
    }

    /// cookie 文件名：`<platform>_<profile>.json`（兼容已有账号池文件）。
    pub fn cookie_filename(&self) -> String {
        format!("{}_{}.json", self.platform, self.profile)
    }

    /// cookie 文件绝对路径：`<base>/cookies/<platform>_<profile>.json`。
    pub fn cookie_path(&self, base: &Path) -> PathBuf {
        base.join("cookies").join(self.cookie_filename())
    }
}

/// 单条 cookie（Playwright storage_state cookie 字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default = "default_cookie_path")]
    pub path: String,
    #[serde(default)]
    pub expires: Option<f64>,
    #[serde(default, rename = "httpOnly", alias = "http_only")]
    pub http_only: bool,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, rename = "sameSite", alias = "same_site")]
    pub same_site: Option<String>,
}

fn default_cookie_path() -> String {
    "/".to_string()
}

/// Playwright/Patchright storage_state。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StorageState {
    #[serde(default)]
    pub cookies: Vec<Cookie>,
    /// 保留 localStorage 等 origin 数据；Rust 核心不改写其内部结构。
    #[serde(default)]
    pub origins: Vec<Value>,
}

/// 设置页展示的一条账号文件信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountFileInfo {
    pub platform: String,
    pub account: String,
    pub cookie_path: String,
    pub exists: bool,
    pub size: u64,
    pub updated_at: Option<i64>,
    pub valid_json: bool,
    pub cookie_count: usize,
}

/// 登录态存储：读写某账号的 storage_state 文件。
#[derive(Debug, Clone)]
pub struct CookieStore {
    cookies_dir: PathBuf,
}

impl CookieStore {
    /// 以 Social Connection 数据根构造（账号目录为 `<base>/cookies`）。
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            cookies_dir: base_dir.into().join("cookies"),
        }
    }

    /// 以已经指向 `cookies` 的目录构造。
    pub fn from_cookies_dir(cookies_dir: impl Into<PathBuf>) -> Self {
        Self {
            cookies_dir: cookies_dir.into(),
        }
    }

    pub fn cookies_dir(&self) -> &Path {
        &self.cookies_dir
    }

    pub fn account_path(&self, account: &Account) -> PathBuf {
        self.cookies_dir.join(account.cookie_filename())
    }

    /// 读取 Playwright storage_state。兼容早期顶层 cookie 数组。
    pub fn load_state(&self, account: &Account) -> std::io::Result<StorageState> {
        let path = self.account_path(account);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(StorageState::default()),
            Err(error) => return Err(error),
        };
        parse_storage_state(&raw)
    }

    /// 读取账号 cookie。文件不存在返回空 vec（未登录）。
    pub fn load(&self, account: &Account) -> std::io::Result<Vec<Cookie>> {
        Ok(self.load_state(account)?.cookies)
    }

    /// 保存完整 storage_state（原子写：先写临时文件再 rename）。
    pub fn save_state(&self, account: &Account, state: &StorageState) -> std::io::Result<PathBuf> {
        let path = self.account_path(account);
        let raw = serde_json::to_string_pretty(state).map_err(json_error)?;
        atomic_write(&path, raw.as_bytes())?;
        Ok(path)
    }

    /// 保存账号 cookie，同时写出 sau/Playwright 可直接读取的 storage_state 对象。
    pub fn save(&self, account: &Account, cookies: &[Cookie]) -> std::io::Result<PathBuf> {
        self.save_state(
            account,
            &StorageState {
                cookies: cookies.to_vec(),
                origins: Vec::new(),
            },
        )
    }

    /// 导入 JSON 账号文件。支持 storage_state、旧 cookie 数组和 biliup JSON。
    pub fn import_json(&self, account: &Account, raw: &str) -> std::io::Result<PathBuf> {
        let value: Value = serde_json::from_str(raw).map_err(json_error)?;
        if !value.is_object() && !value.is_array() {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "账号文件必须是 JSON 对象或数组",
            ));
        }
        let pretty = serde_json::to_string_pretty(&value).map_err(json_error)?;
        let path = self.account_path(account);
        atomic_write(&path, pretty.as_bytes())?;
        Ok(path)
    }

    pub fn export_json(&self, account: &Account) -> std::io::Result<String> {
        std::fs::read_to_string(self.account_path(account))
    }

    pub fn delete(&self, account: &Account) -> std::io::Result<bool> {
        match std::fs::remove_file(self.account_path(account)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// 是否已准备账号文件。真实有效性仍应由纯 Rust CDP 在线校验。
    pub fn is_logged_in(&self, account: &Account) -> bool {
        self.inspect(account)
            .map(|info| info.exists && info.size > 2 && info.valid_json)
            .unwrap_or(false)
    }

    pub fn inspect(&self, account: &Account) -> std::io::Result<AccountFileInfo> {
        inspect_path(account, &self.account_path(account))
    }

    /// 扫描 cookies 目录内所有 `<platform>_<profile>.json`。
    pub fn list(&self) -> std::io::Result<Vec<AccountFileInfo>> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.cookies_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(out),
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Some(account) = account_from_filename(&file_name) else {
                continue;
            };
            out.push(inspect_path(&account, &entry.path())?);
        }
        out.sort_by(|left, right| {
            right
                .updated_at
                .unwrap_or_default()
                .cmp(&left.updated_at.unwrap_or_default())
                .then_with(|| left.platform.cmp(&right.platform))
                .then_with(|| left.account.cmp(&right.account))
        });
        Ok(out)
    }
}

fn parse_storage_state(raw: &str) -> std::io::Result<StorageState> {
    let value: Value = serde_json::from_str(raw).map_err(json_error)?;
    if value.is_array() {
        let cookies = serde_json::from_value(value).map_err(json_error)?;
        return Ok(StorageState {
            cookies,
            origins: Vec::new(),
        });
    }
    serde_json::from_value(value).map_err(json_error)
}

fn account_from_filename(file_name: &str) -> Option<Account> {
    let stem = file_name.strip_suffix(".json")?;
    for platform in SUPPORTED_PLATFORMS {
        let prefix = format!("{platform}_");
        if let Some(profile) = stem.strip_prefix(&prefix) {
            if let Ok(account) = Account::try_new(platform, profile) {
                return Some(account);
            }
        }
    }
    None
}

fn inspect_path(account: &Account, path: &Path) -> std::io::Result<AccountFileInfo> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let raw = metadata
        .as_ref()
        .and_then(|_| std::fs::read_to_string(path).ok());
    let parsed = raw
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok());
    let cookie_count = parsed
        .as_ref()
        .and_then(|value| {
            value
                .get("cookies")
                .and_then(Value::as_array)
                .or_else(|| value.as_array())
        })
        .map(Vec::len)
        .unwrap_or_default();
    Ok(AccountFileInfo {
        platform: account.platform.clone(),
        account: account.profile.clone(),
        cookie_path: path.to_string_lossy().into_owned(),
        exists: metadata.is_some(),
        size: metadata
            .as_ref()
            .map(|value| value.len())
            .unwrap_or_default(),
        updated_at: metadata
            .and_then(|value| value.modified().ok())
            .and_then(system_time_millis),
        valid_json: parsed.is_some(),
        cookie_count,
    })
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let tmp = path.with_extension(format!("json.{nonce}.tmp"));
    std::fs::write(&tmp, bytes)?;
    #[cfg(target_os = "windows")]
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

fn system_time_millis(value: SystemTime) -> Option<i64> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis() as i64)
}

fn json_error(error: serde_json::Error) -> Error {
    Error::new(ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_base(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "yunying-sc-{}-{}-{}",
            std::process::id(),
            name,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn cookie_filename_matches_sau_convention() {
        let a = Account::new("xhs", "staff_xhs_01");
        assert_eq!(a.platform, "xiaohongshu");
        assert_eq!(a.cookie_filename(), "xiaohongshu_staff_xhs_01.json");
    }

    #[test]
    fn profile_rejects_path_traversal() {
        assert!(validate_profile("../secret").is_err());
        assert!(validate_profile(r"a\b").is_err());
        assert_eq!(validate_profile("").unwrap(), "default");
    }

    #[test]
    fn store_roundtrip_uses_playwright_storage_state() {
        let base = tmp_base("rt");
        let store = CookieStore::new(&base);
        let acc = Account::new("douyin", "p1");
        assert!(!store.is_logged_in(&acc));

        let cookies = vec![
            Cookie {
                name: "LOGIN_STATUS".into(),
                value: "1".into(),
                domain: ".douyin.com".into(),
                path: "/".into(),
                expires: None,
                http_only: true,
                secure: true,
                same_site: Some("Lax".into()),
            },
            Cookie {
                name: "sid_guard".into(),
                value: "abc".into(),
                domain: ".douyin.com".into(),
                path: "/".into(),
                expires: Some(1e12),
                http_only: false,
                secure: true,
                same_site: None,
            },
        ];
        let saved_path = store.save(&acc, &cookies).unwrap();
        assert!(saved_path.ends_with("douyin_p1.json"));
        assert!(store.is_logged_in(&acc));

        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(&saved_path).unwrap()).unwrap();
        assert!(raw["cookies"].is_array());
        assert!(raw["origins"].is_array());
        assert_eq!(raw["cookies"][0]["httpOnly"], true);

        let loaded = store.load(&acc).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].name, "LOGIN_STATUS");
        assert_eq!(loaded[1].value, "abc");
    }

    #[test]
    fn legacy_cookie_array_is_supported() {
        let base = tmp_base("legacy");
        let store = CookieStore::new(&base);
        let acc = Account::new("bilibili", "legacy");
        std::fs::create_dir_all(store.cookies_dir()).unwrap();
        std::fs::write(
            store.account_path(&acc),
            r#"[{"name":"SESSDATA","value":"x","domain":".bilibili.com","path":"/","httpOnly":true}]"#,
        )
        .unwrap();
        assert_eq!(store.load(&acc).unwrap()[0].name, "SESSDATA");
    }

    #[test]
    fn list_discovers_profiles_with_underscores() {
        let base = tmp_base("list");
        let store = CookieStore::new(&base);
        let acc = Account::new("xiaohongshu", "staff_xhs_01");
        store
            .import_json(&acc, r#"{"cookies":[],"origins":[]}"#)
            .unwrap();
        std::fs::write(store.cookies_dir().join("login_qrcode.png"), b"png").unwrap();
        let items = store.list().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].platform, "xiaohongshu");
        assert_eq!(items[0].account, "staff_xhs_01");
        assert!(items[0].valid_json);
    }

    #[test]
    fn import_rejects_non_json_and_delete_is_idempotent() {
        let base = tmp_base("import");
        let store = CookieStore::new(&base);
        let acc = Account::new("youtube", "creator");
        assert!(store.import_json(&acc, "not-json").is_err());
        store.import_json(&acc, r#"{"cookies":[]}"#).unwrap();
        assert!(store.delete(&acc).unwrap());
        assert!(!store.delete(&acc).unwrap());
    }

    #[test]
    fn load_missing_returns_empty() {
        let base = tmp_base("missing");
        let store = CookieStore::new(&base);
        let acc = Account::new("bilibili", "never");
        assert!(store.load(&acc).unwrap().is_empty());
        assert!(!store.is_logged_in(&acc));
    }
}
