//! CDP 扫码登录驱动（chromiumoxide），实现 [`super::LoginDriver`]。
//!
//! 真实驱动 Chrome：
//! 1. 导航到平台登录页（[`super::PlatformLogin::login_url`]）。
//! 2. 等待二维码元素（`qr_selector`）→ **截图** → base64（统一处理 img src / canvas / data URI）。
//! 3. 轮询 `document.cookie` → 检查 `login_cookie_names` 出现 → 登录成功。
//! 4. 返回完整 cookie 串，回写账号池。
//!
//! 移植自 MediaCrawler `login_by_qrcode` + `find_login_qrcode` + social-auto-upload QR 登录流程。

#![cfg(feature = "cdp")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use tokio::sync::Mutex;

use super::{find_platform, LoginDriver, LoginSession, LoginStatus, PlatformLogin};

/// CDP 登录会话（持有浏览器 tab）。
struct CdpSession {
    page: Page,
    config: PlatformLogin,
    profile: String,
}

/// chromiumoxide CDP 扫码登录驱动。
pub struct CdpLoginDriver {
    /// 持有 browser 防 drop（drop 关 Chrome）。
    browser: Browser,
    /// 活跃登录会话：session_id → (page, config, profile)。
    sessions: Arc<Mutex<HashMap<String, CdpSession>>>,
}

impl CdpLoginDriver {
    /// 启动 Chrome（指定路径或自动探测）。
    pub async fn new(chrome_path: Option<&str>, headless: bool) -> anyhow::Result<Self> {
        let mut builder = BrowserConfig::builder();
        if let Some(p) = chrome_path {
            builder = builder.chrome_executable(p);
        }
        if headless {
            builder = builder.arg("--headless=new");
        }
        builder = builder.arg("--disable-blink-features=AutomationControlled");
        let config = builder
            .build()
            .map_err(|e| anyhow::anyhow!("browser config: {e}"))?;

        let (browser, mut handler) = Browser::launch(config).await?;
        // 驱动 chromiumoxide 事件循环。
        tokio::spawn(async move { while handler.next().await.is_some() {} });

        Ok(Self {
            browser,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

#[async_trait]
impl LoginDriver for CdpLoginDriver {
    async fn start(
        &self,
        platform: &str,
        profile: &str,
        _headless: bool,
    ) -> anyhow::Result<LoginSession> {
        let config =
            find_platform(platform).ok_or_else(|| anyhow::anyhow!("不支持的平台: {platform}"))?;
        let prof = if profile.is_empty() {
            format!("{}_default", platform)
        } else {
            profile.to_string()
        };

        // 1) 导航到登录页。
        let page = self.browser.new_page(config.login_url).await?;
        tokio::time::sleep(Duration::from_secs(2)).await; // 等页面稳定。

        // 2) 找到二维码元素 → 截图 → base64。
        let qr_image = match capture_qr(&page, config.qr_selector).await {
            Ok(qr) => Some(qr),
            Err(e) => {
                tracing::warn!("二维码截取失败（选择器可能需调整）: {e}");
                None
            }
        };

        let session_id = format!("cdp-{}-{}", platform, now_ts());
        self.sessions.lock().await.insert(
            session_id.clone(),
            CdpSession {
                page: page.clone(),
                config: config.clone(),
                profile: prof.clone(),
            },
        );

        Ok(LoginSession {
            session_id,
            platform: platform.into(),
            profile: prof,
            qr_image,
            status: LoginStatus::Pending,
            cookies: None,
            error: None,
        })
    }

    async fn poll(&self, session_id: &str) -> anyhow::Result<LoginSession> {
        let sessions = self.sessions.lock().await;
        let sess = sessions
            .get(session_id)
            .ok_or_else(|| anyhow::anyhow!("登录会话不存在: {session_id}"))?;

        // 3) 读 document.cookie → 检查登录态 cookie 名。
        let cookie_str = match sess.page.evaluate("document.cookie").await {
            Ok(r) => r.into_value::<String>().unwrap_or_default(),
            Err(e) => {
                return Ok(LoginSession {
                    session_id: session_id.into(),
                    platform: sess.config.platform.into(),
                    profile: sess.profile.clone(),
                    qr_image: None,
                    status: LoginStatus::Failed,
                    cookies: None,
                    error: Some(format!("读取 cookie 失败: {e}")),
                });
            }
        };

        let logged_in = sess
            .config
            .login_cookie_names
            .iter()
            .any(|name| cookie_str.contains(name));

        if logged_in {
            Ok(LoginSession {
                session_id: session_id.into(),
                platform: sess.config.platform.into(),
                profile: sess.profile.clone(),
                qr_image: None, // 登录成功，清空 QR。
                status: LoginStatus::LoggedIn,
                cookies: Some(cookie_str),
                error: None,
            })
        } else {
            Ok(LoginSession {
                session_id: session_id.into(),
                platform: sess.config.platform.into(),
                profile: sess.profile.clone(),
                qr_image: None,
                status: LoginStatus::Pending,
                cookies: None,
                error: None,
            })
        }
    }

    async fn stop(&self, session_id: &str) -> anyhow::Result<()> {
        if let Some(sess) = self.sessions.lock().await.remove(session_id) {
            // 关闭登录页（page.close 需 CDP；best-effort）。
            sess.page.evaluate("window.close()").await.ok();
        }
        Ok(())
    }
}

/// 截取二维码元素 → base64 / data URI / URL。
///
/// 移植自 MediaCrawler `find_login_qrcode`：
/// 1. 等待 QR 元素出现（最多 10s）。
/// 2. 读 `src` 属性：data URI 直接返回；http URL 返回（UI 直接渲染）。
/// 3. canvas 元素：JS `toDataURL()` 转 data URI。
async fn capture_qr(page: &Page, selector: &str) -> anyhow::Result<String> {
    // 等待 QR 元素出现。
    let deadline = Instant::now() + Duration::from_secs(10);
    let element = loop {
        match page.find_element(selector).await {
            Ok(el) => break el,
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(e) => anyhow::bail!("二维码元素未出现（选择器 {selector}）: {e}"),
        }
    };

    // 尝试读 src 属性。
    if let Ok(Some(src)) = element.attribute("src").await {
        if !src.is_empty() {
            return Ok(src); // data: 或 http:// URL（UI 直接渲染）。
        }
    }

    // canvas fallback：JS toDataURL。
    let js = format!(
        "(function(){{var el=document.querySelector({selector_js});if(!el)return '';if(el.tagName==='CANVAS')return el.toDataURL('image/png');if(el.src)return el.src;return '';}})()",
        selector_js = js_str(selector)
    );
    let result = page.evaluate(js).await?;
    let val: String = result.into_value().unwrap_or_default();
    if !val.is_empty() {
        return Ok(val);
    }

    anyhow::bail!("无法获取二维码（选择器 {selector} 既无 src 也非 canvas）")
}

fn js_str(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('\'', "\\'");
    format!("'{escaped}'")
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
