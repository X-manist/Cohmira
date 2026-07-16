//! 账号绑定 · 扫码登录模块。
//!
//! 设计目标（多平台账号池 + 扫码绑定）：
//! - 每个平台一份 [`PlatformLogin`] 配置（登录页 URL / 二维码选择器 / 登录态 cookie 名）。
//! - [`LoginDriver`] 抽象扫码登录流程：导航到登录页 → 截取二维码 → 轮询登录态 cookie → 回写账号池。
//! - [`LoginSession`] 注册表：跨 ipc 调用保持登录会话（start-login 返回 session id，get-login-status 轮询）。
//!
//! 实现：
//! - [`StubLoginDriver`]：返回占位二维码 + 模拟登录（无 Chrome 时的默认）。
//! - `CdpLoginDriver`（feature = "cdp"，chromiumoxide）：真实驱动 Chrome 扫码登录。
//!
//! 账号落库到 `platform_accounts` 表（见 db schema），operations 工具读此处拿 cookie。

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(feature = "cdp")]
pub mod cdp;

/// 某平台的扫码登录配置（静态配置，只序列化用于 UI 展示，不反序列化）。
#[derive(Debug, Clone, Serialize)]
pub struct PlatformLogin {
    pub platform: &'static str,
    /// 登录页 URL（页面会展示二维码）。
    pub login_url: &'static str,
    /// 二维码图片元素的 CSS 选择器（截图该区域作为 qr_image）。
    pub qr_selector: &'static str,
    /// 表示登录成功的 cookie 名（出现即视为已登录）。
    pub login_cookie_names: &'static [&'static str],
    /// 平台中文名（UI 展示）。
    pub label: &'static str,
}

/// 内置平台登录配置（多平台）。可按需扩展。
pub fn platform_logins() -> Vec<PlatformLogin> {
    vec![
        PlatformLogin {
            platform: "xhs",
            login_url: "https://www.xiaohongshu.com",
            qr_selector: "img.qrcode, canvas",
            login_cookie_names: &["web_session"],
            label: "小红书",
        },
        PlatformLogin {
            platform: "douyin",
            login_url: "https://www.douyin.com",
            qr_selector: "img[src*='qrcode'], .qrcode img",
            login_cookie_names: &["sid_guard", "LOGIN_STATUS"],
            label: "抖音",
        },
        PlatformLogin {
            platform: "bilibili",
            login_url: "https://passport.bilibili.com/login",
            qr_selector: "img.qrcode, .login-sao-box img",
            login_cookie_names: &["SESSDATA", "DedeUserID"],
            label: "B站",
        },
        PlatformLogin {
            platform: "weibo",
            login_url: "https://weibo.com/login.php",
            qr_selector: "img.qrcode",
            login_cookie_names: &["SSOLoginState", "SUB"],
            label: "微博",
        },
        PlatformLogin {
            platform: "kuaishou",
            login_url: "https://www.kuaishou.com/login",
            qr_selector: "img.qrcode",
            login_cookie_names: &["passToken"],
            label: "快手",
        },
        PlatformLogin {
            platform: "tencent",
            login_url: "https://channels.weixin.qq.com/login",
            qr_selector: "img.qrcode, .login__type__container__scan",
            login_cookie_names: &["login_type", "sessionid"],
            label: "视频号",
        },
    ]
}

pub fn find_platform(platform: &str) -> Option<PlatformLogin> {
    platform_logins()
        .into_iter()
        .find(|p| p.platform == platform)
}

/// 登录状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LoginStatus {
    Pending,
    Scanned,
    LoggedIn,
    Expired,
    Failed,
}

/// 一次扫码登录会话（start-login 创建，get-login-status 轮询）。
#[derive(Debug, Clone, Serialize)]
pub struct LoginSession {
    pub session_id: String,
    pub platform: String,
    pub profile: String,
    pub qr_image: Option<String>, // base64（data:image/png;base64,...）
    pub status: LoginStatus,
    pub cookies: Option<String>, // 登录成功后的 cookie 串
    pub error: Option<String>,
}

/// 登录驱动抽象。由 [`StubLoginDriver`]（默认）或 `CdpLoginDriver`（feature=cdp）实现。
#[async_trait::async_trait]
pub trait LoginDriver: Send + Sync {
    /// 启动扫码登录：导航到登录页，返回二维码（base64）+ session。
    async fn start(
        &self,
        platform: &str,
        profile: &str,
        headless: bool,
    ) -> anyhow::Result<LoginSession>;
    /// 轮询：检查登录态 cookie 是否出现，返回最新 session 状态。
    async fn poll(&self, session_id: &str) -> anyhow::Result<LoginSession>;
    /// 停止登录。
    async fn stop(&self, session_id: &str) -> anyhow::Result<()>;
}

/// 账号绑定服务：持有登录驱动 + 会话注册表。
pub struct LoginService {
    driver: Arc<dyn LoginDriver>,
    sessions: Arc<Mutex<HashMap<String, LoginSession>>>,
}

impl LoginService {
    pub fn new(driver: Arc<dyn LoginDriver>) -> Self {
        Self {
            driver,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn start_login(
        &self,
        platform: &str,
        profile: &str,
        headless: bool,
    ) -> anyhow::Result<Value> {
        let mut sess = self.driver.start(platform, profile, headless).await?;
        // profile 缺省为 platform_default。
        if sess.profile.is_empty() {
            sess.profile = format!("{}_default", platform);
        }
        let id = sess.session_id.clone();
        let qr = sess.qr_image.clone();
        self.sessions.lock().await.insert(id.clone(), sess);
        Ok(json!({ "sessionId": id, "qrImage": qr, "status": "pending" }))
    }

    pub async fn login_status(&self, session_id: &str) -> anyhow::Result<Value> {
        // 先让 driver 更新（poll 检测 cookie），再读注册表。
        if let Ok(updated) = self.driver.poll(session_id).await {
            self.sessions
                .lock()
                .await
                .insert(session_id.into(), updated);
        }
        let sess = self
            .sessions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("登录会话不存在: {session_id}"))?;
        Ok(serde_json::to_value(&sess)?)
    }

    pub async fn stop_login(&self, session_id: &str) -> anyhow::Result<()> {
        self.driver.stop(session_id).await.ok();
        self.sessions.lock().await.remove(session_id);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// StubLoginDriver：无 Chrome 时的默认（返回占位二维码 + 模拟登录）。
// ---------------------------------------------------------------------------

/// 占位二维码（1x1 透明 PNG base64）。真实驱动返回截取的平台二维码。
const PLACEHOLDER_QR: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

pub struct StubLoginDriver;

#[async_trait::async_trait]
impl LoginDriver for StubLoginDriver {
    async fn start(
        &self,
        platform: &str,
        profile: &str,
        _headless: bool,
    ) -> anyhow::Result<LoginSession> {
        Ok(LoginSession {
            session_id: format!("sess-{}-{}", platform, now_ts()),
            platform: platform.into(),
            profile: profile.into(),
            qr_image: Some(PLACEHOLDER_QR.into()),
            status: LoginStatus::Pending,
            cookies: None,
            error: None,
        })
    }

    async fn poll(&self, _session_id: &str) -> anyhow::Result<LoginSession> {
        // Stub：保持 pending（真实登录需 CDP 驱动检测 cookie）。
        Err(anyhow::anyhow!(
            "StubLoginDriver：需启用 cdp feature（chromiumoxide）才真实检测扫码登录"
        ))
    }

    async fn stop(&self, _session_id: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_logins_cover_main() {
        let ps = platform_logins();
        for p in ["xhs", "douyin", "bilibili", "weibo", "kuaishou", "tencent"] {
            assert!(ps.iter().any(|x| x.platform == p), "缺平台 {p}");
        }
    }

    #[test]
    fn each_platform_has_login_cookie() {
        for p in platform_logins() {
            assert!(
                !p.login_cookie_names.is_empty(),
                "{} 需至少一个登录态 cookie 名",
                p.platform
            );
        }
    }

    #[tokio::test]
    async fn stub_start_returns_qr_and_session() {
        let svc = LoginService::new(Arc::new(StubLoginDriver));
        let r = svc.start_login("xhs", "p1", true).await.unwrap();
        assert_eq!(r["status"], json!("pending"));
        assert!(r["qrImage"].is_string());
        assert!(r["sessionId"].as_str().unwrap().starts_with("sess-xhs-"));
    }
}
