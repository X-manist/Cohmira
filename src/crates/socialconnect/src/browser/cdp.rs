//! 纯 Rust CDP 浏览器后端。
//!
//! 该实现直接使用 [`chromiumoxide`] 与 Chrome DevTools Protocol 通讯，不依赖
//! Python、Node、Playwright Server 或 chromedriver。账号登录态以
//! [`crate::account::StorageState`] 保存，同时为每个平台/profile 使用独立的 Chrome
//! user-data-dir，兼容 Google/YouTube 等依赖 IndexedDB/localStorage 的登录流程。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chromiumoxide::cdp::browser_protocol::dom::SetFileInputFilesParams;
use chromiumoxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
use chromiumoxide::cdp::browser_protocol::network::{CookieParam, CookieSameSite, TimeSinceEpoch};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use tokio::sync::Mutex;

use crate::account::{Cookie, StorageState};

use super::BrowserDriver;

/// 启动独立浏览器账号上下文所需参数。
#[derive(Debug, Clone)]
pub struct BrowserLaunchOptions {
    pub executable: Option<PathBuf>,
    pub user_data_dir: PathBuf,
    pub headless: bool,
    pub proxy_url: Option<String>,
    pub window_width: u32,
    pub window_height: u32,
}

impl BrowserLaunchOptions {
    pub fn new(user_data_dir: impl Into<PathBuf>) -> Self {
        Self {
            executable: None,
            user_data_dir: user_data_dir.into(),
            headless: true,
            proxy_url: None,
            window_width: 1440,
            window_height: 1000,
        }
    }
}

/// chromiumoxide CDP 浏览器驱动。
pub struct CdpBrowser {
    browser: Mutex<Option<Browser>>,
    page: Page,
    handler_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl CdpBrowser {
    /// 兼容旧调用点的简化启动入口。
    pub async fn launch(chrome_path: Option<&str>, headless: bool) -> anyhow::Result<Self> {
        let profile = std::env::temp_dir().join(format!(
            "yunying-social-browser-{}-{}",
            std::process::id(),
            now_millis()
        ));
        let mut options = BrowserLaunchOptions::new(profile);
        options.executable = chrome_path.map(PathBuf::from);
        options.headless = headless;
        Self::launch_with(options).await
    }

    /// 启动一个使用独立持久化 profile 的 Chrome。
    pub async fn launch_with(options: BrowserLaunchOptions) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&options.user_data_dir)?;
        let executable = match options.executable {
            Some(path) => Some(path),
            None => detect_browser_executable(),
        }
        .ok_or_else(|| {
            anyhow::anyhow!("未找到可用的 Chrome/Chromium/Edge；请在设置中指定浏览器可执行文件")
        })?;

        let mut builder = BrowserConfig::builder()
            .chrome_executable(&executable)
            .user_data_dir(&options.user_data_dir)
            .window_size(options.window_width, options.window_height)
            .viewport(None)
            .launch_timeout(Duration::from_secs(45))
            .request_timeout(Duration::from_secs(120))
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--disable-features=HttpsFirstBalancedModeAutoEnable")
            .arg("--no-first-run")
            .arg("--no-default-browser-check");
        if options.headless {
            builder = builder.new_headless_mode();
        } else {
            builder = builder.with_head();
        }
        if let Some(proxy) = options
            .proxy_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            builder = builder.arg(format!("--proxy-server={proxy}"));
        }
        #[cfg(target_os = "linux")]
        {
            builder = builder.no_sandbox();
        }
        let config = builder
            .build()
            .map_err(|error| anyhow::anyhow!("构造 Chrome 启动参数失败: {error}"))?;
        let (browser, mut handler) = Browser::launch(config).await.map_err(|error| {
            anyhow::anyhow!("启动浏览器 {} 失败: {error}", executable.display())
        })?;
        let handler_task = tokio::spawn(async move {
            while let Some(result) = handler.next().await {
                if result.is_err() {
                    break;
                }
            }
        });
        let page = browser.new_page("about:blank").await?;
        // chromiumoxide 自带纯 Rust stealth 初始化，覆盖 webdriver/plugins/WebGL/window.chrome。
        page.enable_stealth_mode_with_agent(&desktop_user_agent())
            .await?;
        Ok(Self {
            browser: Mutex::new(Some(browser)),
            page,
            handler_task: Mutex::new(Some(handler_task)),
        })
    }

    pub fn page(&self) -> &Page {
        &self.page
    }

    pub async fn close(&self) -> anyhow::Result<()> {
        if let Some(mut browser) = self.browser.lock().await.take() {
            let _ = browser.close().await;
            let _ = browser.wait().await;
        }
        if let Some(task) = self.handler_task.lock().await.take() {
            task.abort();
        }
        Ok(())
    }

    /// 把账号 storage_state 注入当前浏览器。Chrome profile 自己已有登录态时也可安全重复。
    pub async fn load_storage_state(&self, state: &StorageState) -> anyhow::Result<()> {
        if !state.cookies.is_empty() {
            let cookies = state
                .cookies
                .iter()
                .map(cookie_to_cdp)
                .collect::<anyhow::Result<Vec<_>>>()?;
            if let Some(browser) = self.browser.lock().await.as_ref() {
                browser.set_cookies(cookies).await?;
            }
        }
        // localStorage 必须在对应 origin 下写入；写完恢复到 about:blank。
        for origin in &state.origins {
            let Some(origin_url) = origin.get("origin").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let Some(items) = origin
                .get("localStorage")
                .and_then(serde_json::Value::as_array)
            else {
                continue;
            };
            if items.is_empty() {
                continue;
            }
            self.page.goto(origin_url).await?;
            for item in items {
                let Some(name) = item.get("name").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let Some(value) = item.get("value").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                let script = format!("localStorage.setItem({}, {})", js_str(name), js_str(value));
                let _ = self.page.evaluate(script).await;
            }
        }
        Ok(())
    }

    /// 导出 Playwright 兼容 cookie；origins 由调用方传入旧值以避免丢失未打开域的数据。
    pub async fn storage_state(
        &self,
        previous_origins: Vec<serde_json::Value>,
    ) -> anyhow::Result<StorageState> {
        let cookies = if let Some(browser) = self.browser.lock().await.as_ref() {
            browser
                .get_cookies()
                .await?
                .into_iter()
                .map(|cookie| Cookie {
                    name: cookie.name,
                    value: cookie.value,
                    domain: cookie.domain,
                    path: cookie.path,
                    expires: (!cookie.session && cookie.expires >= 0.0).then_some(cookie.expires),
                    http_only: cookie.http_only,
                    secure: cookie.secure,
                    same_site: cookie.same_site.map(|value| match value {
                        CookieSameSite::Strict => "Strict".to_string(),
                        CookieSameSite::Lax => "Lax".to_string(),
                        CookieSameSite::None => "None".to_string(),
                    }),
                })
                .collect()
        } else {
            Vec::new()
        };
        Ok(StorageState {
            cookies,
            origins: previous_origins,
        })
    }

    pub async fn selector_exists(&self, selector: &str) -> bool {
        self.page.find_element(selector).await.is_ok()
    }

    pub async fn wait_for_selector(&self, selector: &str, timeout: Duration) -> anyhow::Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.selector_exists(selector).await {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!("等待选择器 {selector:?} 超时（{} 秒）", timeout.as_secs());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    pub async fn element_attribute(
        &self,
        selector: &str,
        name: &str,
    ) -> anyhow::Result<Option<String>> {
        let element = self.page.find_element(selector).await?;
        Ok(element.attribute(name).await?)
    }

    pub async fn screenshot_element(
        &self,
        selector: &str,
        output: &Path,
    ) -> anyhow::Result<Vec<u8>> {
        let element = self.page.find_element(selector).await?;
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(element
            .save_screenshot(CaptureScreenshotFormat::Png, output)
            .await?)
    }

    pub async fn screenshot_page(&self, output: &Path) -> anyhow::Result<Vec<u8>> {
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(self
            .page
            .save_screenshot(
                chromiumoxide::page::ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .full_page(false)
                    .build(),
                output,
            )
            .await?)
    }

    async fn eval_bool(&self, js: &str) -> anyhow::Result<bool> {
        let result = self.page.evaluate(js).await.map_err(map_err)?;
        Ok(result.into_value::<bool>().unwrap_or(false))
    }
}

#[async_trait]
impl BrowserDriver for CdpBrowser {
    async fn goto(&self, url: &str) -> anyhow::Result<()> {
        self.page.goto(url).await.map_err(map_err)?;
        Ok(())
    }

    async fn current_url(&self) -> anyhow::Result<String> {
        Ok(self.page.url().await.map_err(map_err)?.unwrap_or_default())
    }

    async fn is_text_present(&self, text: &str) -> anyhow::Result<bool> {
        self.eval_bool(&format!(
            "Boolean(document.body && document.body.innerText.includes({}))",
            js_str(text)
        ))
        .await
    }

    async fn click_text(&self, text: &str) -> anyhow::Result<()> {
        let result = self
            .page
            .evaluate(format!(
                "(function(){{const wanted={};const nodes=[...document.querySelectorAll('button,a,div,span,p')];const el=nodes.find(n=>n.offsetParent!==null&&n.textContent&&n.textContent.trim()===wanted)||nodes.find(n=>n.offsetParent!==null&&n.textContent&&n.textContent.includes(wanted));if(!el)return false;el.click();return true;}})()",
                js_str(text)
            ))
            .await?;
        if !result.into_value::<bool>().unwrap_or(false) {
            anyhow::bail!("未找到可点击文本: {text}");
        }
        Ok(())
    }

    async fn click_selector(&self, selector: &str) -> anyhow::Result<()> {
        let element = self.page.find_element(selector).await.map_err(map_err)?;
        element.click().await.map_err(map_err)?;
        Ok(())
    }

    async fn fill(&self, selector: &str, value: &str) -> anyhow::Result<()> {
        let element = self.page.find_element(selector).await.map_err(map_err)?;
        element.focus().await.map_err(map_err)?;
        // 先通过 JS 清空 input/contenteditable，避免多次 fill 变成追加。
        let _ = element
            .call_js_fn(
                "function(){if('value' in this){this.value='';this.dispatchEvent(new Event('input',{bubbles:true}));}else{this.textContent='';}}",
                false,
            )
            .await;
        element.type_str(value).await.map_err(map_err)?;
        Ok(())
    }

    async fn type_text(&self, selector: &str, value: &str) -> anyhow::Result<()> {
        let element = self.page.find_element(selector).await.map_err(map_err)?;
        element.focus().await.map_err(map_err)?;
        element.type_str(value).await.map_err(map_err)?;
        Ok(())
    }

    async fn set_input_files(&self, selector: &str, paths: &[String]) -> anyhow::Result<()> {
        self.set_input_files_nth(selector, 0, paths).await
    }

    async fn set_input_files_nth(
        &self,
        selector: &str,
        index: usize,
        paths: &[String],
    ) -> anyhow::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        for path in paths {
            if !Path::new(path).is_file() {
                anyhow::bail!("待上传文件不存在: {path}");
            }
        }
        let elements = self.page.find_elements(selector).await.map_err(map_err)?;
        let element = elements.get(index).ok_or_else(|| {
            anyhow::anyhow!("选择器 {selector:?} 没有第 {} 个 file input", index + 1)
        })?;
        self.page
            .execute(
                SetFileInputFilesParams::builder()
                    .files(paths.iter().cloned())
                    .node_id(element.node_id)
                    .build()
                    .map_err(anyhow::Error::msg)?,
            )
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn wait_for_url_contains(&self, fragment: &str) -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if self
                .page
                .url()
                .await
                .ok()
                .flatten()
                .is_some_and(|url| url.contains(fragment))
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!("等待 URL 含 {fragment:?} 超时");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn sleep_ms(&self, ms: u64) -> anyhow::Result<()> {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(())
    }

    async fn selector_exists(&self, selector: &str) -> anyhow::Result<bool> {
        Ok(CdpBrowser::selector_exists(self, selector).await)
    }

    async fn wait_for_selector(&self, selector: &str, timeout_ms: u64) -> anyhow::Result<()> {
        CdpBrowser::wait_for_selector(self, selector, Duration::from_millis(timeout_ms)).await
    }

    async fn attribute(&self, selector: &str, name: &str) -> anyhow::Result<Option<String>> {
        self.element_attribute(selector, name).await
    }

    async fn evaluate(&self, script: &str) -> anyhow::Result<serde_json::Value> {
        let result = self.page.evaluate(script).await.map_err(map_err)?;
        Ok(result.into_value().unwrap_or(serde_json::Value::Null))
    }

    async fn press_key(&self, key: &str) -> anyhow::Result<()> {
        let (code, virtual_key, text) = match key {
            "Enter" => ("Enter", 13, Some("\r")),
            "Escape" => ("Escape", 27, None),
            "Space" | " " => ("Space", 32, Some(" ")),
            "Backspace" => ("Backspace", 8, None),
            "Tab" => ("Tab", 9, None),
            other => (other, 0, Some(other)),
        };
        let mut down = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::RawKeyDown)
            .key(key)
            .code(code)
            .windows_virtual_key_code(virtual_key);
        if let Some(text) = text {
            down = down.text(text);
        }
        self.page
            .execute(down.build().map_err(anyhow::Error::msg)?)
            .await
            .map_err(map_err)?;
        self.page
            .execute(
                DispatchKeyEventParams::builder()
                    .r#type(DispatchKeyEventType::KeyUp)
                    .key(key)
                    .code(code)
                    .windows_virtual_key_code(virtual_key)
                    .build()
                    .map_err(anyhow::Error::msg)?,
            )
            .await
            .map_err(map_err)?;
        Ok(())
    }
}

fn cookie_to_cdp(cookie: &Cookie) -> anyhow::Result<CookieParam> {
    let mut builder = CookieParam::builder()
        .name(cookie.name.clone())
        .value(cookie.value.clone())
        .path(cookie.path.clone())
        .secure(cookie.secure)
        .http_only(cookie.http_only);
    if !cookie.domain.trim().is_empty() {
        builder = builder.domain(cookie.domain.clone());
    }
    if let Some(expires) = cookie.expires.filter(|value| *value > 0.0) {
        builder = builder.expires(TimeSinceEpoch::new(expires));
    }
    if let Some(same_site) = cookie.same_site.as_deref() {
        builder = builder.same_site(match same_site.to_ascii_lowercase().as_str() {
            "strict" => CookieSameSite::Strict,
            "none" => CookieSameSite::None,
            _ => CookieSameSite::Lax,
        });
    }
    builder.build().map_err(anyhow::Error::msg)
}

/// 探测用户已安装的 Chromium 系浏览器。
pub fn detect_browser_executable() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("SOCIAL_CONNECTION_BROWSER") {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Some(path);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let mut candidates = vec![
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
            PathBuf::from("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home).join("Applications");
            candidates.extend([
                home.join("Google Chrome.app/Contents/MacOS/Google Chrome"),
                home.join("Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
                home.join("Chromium.app/Contents/MacOS/Chromium"),
            ]);
        }
        if let Some(path) = candidates.into_iter().find(|path| path.is_file()) {
            return Some(path);
        }
    }

    #[cfg(target_os = "windows")]
    {
        let roots = [
            std::env::var_os("PROGRAMFILES"),
            std::env::var_os("PROGRAMFILES(X86)"),
            std::env::var_os("LOCALAPPDATA"),
        ];
        for root in roots.into_iter().flatten().map(PathBuf::from) {
            for relative in [
                "Google/Chrome/Application/chrome.exe",
                "Microsoft/Edge/Application/msedge.exe",
                "Chromium/Application/chrome.exe",
                "BraveSoftware/Brave-Browser/Application/brave.exe",
            ] {
                let candidate = root.join(relative);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    for executable in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "msedge",
        "brave-browser",
    ] {
        if let Ok(path) = which::which(executable) {
            return Some(path);
        }
    }
    None
}

fn desktop_user_agent() -> String {
    #[cfg(target_os = "macos")]
    return "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36".to_string();
    #[cfg(target_os = "windows")]
    return "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36".to_string();
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    return "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36".to_string();
}

fn map_err(error: chromiumoxide::error::CdpError) -> anyhow::Error {
    anyhow::anyhow!("CDP 浏览器错误: {error}")
}

fn js_str(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_string_uses_json_escaping() {
        assert_eq!(js_str("a'b"), "\"a'b\"");
        assert_eq!(js_str("a\n"), "\"a\\n\"");
    }

    #[test]
    fn configured_browser_path_wins() {
        let old = std::env::var_os("SOCIAL_CONNECTION_BROWSER");
        let temp = std::env::temp_dir().join(format!("browser-{}", now_millis()));
        std::fs::write(&temp, b"browser").unwrap();
        std::env::set_var("SOCIAL_CONNECTION_BROWSER", &temp);
        assert_eq!(detect_browser_executable(), Some(temp.clone()));
        if let Some(value) = old {
            std::env::set_var("SOCIAL_CONNECTION_BROWSER", value);
        } else {
            std::env::remove_var("SOCIAL_CONNECTION_BROWSER");
        }
        let _ = std::fs::remove_file(temp);
    }
}
