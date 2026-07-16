//! Social Connection 纯 Rust 运行时。
//!
//! 登录、在线检查、Cookie 持久化和浏览器生命周期全部在 Rust 进程内完成。浏览器层直接
//! 使用 Chrome DevTools Protocol，不调用 `sau`、Python、Playwright Server 或
//! chromedriver。

use crate::account::{Account, AccountFileInfo, CookieStore};
use crate::browser::cdp::{detect_browser_executable, BrowserLaunchOptions, CdpBrowser};
use crate::browser::{BrowserDriver, StubDriver};
use crate::cli::{NativeActionPlan, NoteUploadOptions, VideoUploadOptions};
use crate::uploader::bilibili::BiliUploader;
use crate::uploader::douyin::DouyinUploader;
use crate::uploader::kuaishou::KuaishouUploader;
use crate::uploader::tencent::TencentUploader;
use crate::uploader::xiaohongshu::XiaohongshuUploader;
use crate::uploader::youtube::YouTubeUploader;
use crate::uploader::{UploadRequest, UploadResult, Uploader};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{watch, RwLock};

pub const NATIVE_RUNTIME_CONFIG_ENV: &str = "SOCIAL_CONNECTION_NATIVE_CONFIG";

const DOUYIN_LOGIN_URL: &str = "https://creator.douyin.com/";
const DOUYIN_CHECK_URL: &str = "https://creator.douyin.com/creator-micro/content/upload";
const KUAISHOU_LOGIN_URL: &str = "https://passport.kuaishou.com/pc/account/login/?sid=kuaishou.web.cp.api&callback=https%3A%2F%2Fcp.kuaishou.com%2Frest%2Finfra%2Fsts%3FfollowUrl%3Dhttps%253A%252F%252Fcp.kuaishou.com%252Farticle%252Fpublish%252Fvideo%26setRootDomain%3Dtrue";
const KUAISHOU_CHECK_URL: &str = "https://cp.kuaishou.com/article/publish/video";
const XHS_LOGIN_URL: &str = "https://creator.xiaohongshu.com/login";
const XHS_CHECK_URL: &str =
    "https://creator.xiaohongshu.com/publish/publish?from=homepage&target=video";
const BILIBILI_LOGIN_URL: &str = "https://passport.bilibili.com/login";
const BILIBILI_CHECK_URL: &str = "https://member.bilibili.com/platform/upload/video/frame";
const TENCENT_LOGIN_URL: &str = "https://channels.weixin.qq.com";
const TENCENT_CHECK_URL: &str = "https://channels.weixin.qq.com/platform/post/create";
const YOUTUBE_LOGIN_URL: &str = "https://studio.youtube.com";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativePlatformCapability {
    pub platform: &'static str,
    pub login: bool,
    pub check: bool,
    pub upload_video: bool,
    pub upload_note: bool,
    pub schedule: bool,
    pub interactive_login: bool,
}

pub const NATIVE_PLATFORM_CAPABILITIES: &[NativePlatformCapability] = &[
    NativePlatformCapability {
        platform: "douyin",
        login: true,
        check: true,
        upload_video: true,
        upload_note: true,
        schedule: true,
        interactive_login: false,
    },
    NativePlatformCapability {
        platform: "kuaishou",
        login: true,
        check: true,
        upload_video: true,
        upload_note: true,
        schedule: true,
        interactive_login: false,
    },
    NativePlatformCapability {
        platform: "xiaohongshu",
        login: true,
        check: true,
        upload_video: true,
        upload_note: true,
        schedule: true,
        interactive_login: false,
    },
    NativePlatformCapability {
        platform: "bilibili",
        login: true,
        check: true,
        upload_video: true,
        upload_note: false,
        schedule: true,
        interactive_login: false,
    },
    NativePlatformCapability {
        platform: "tencent",
        login: true,
        check: true,
        upload_video: true,
        upload_note: false,
        schedule: true,
        interactive_login: false,
    },
    NativePlatformCapability {
        platform: "youtube",
        login: true,
        check: true,
        upload_video: true,
        upload_note: false,
        schedule: false,
        interactive_login: true,
    },
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeRuntimeDescriptor {
    pub data_root: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_executable: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NativeRuntime {
    data_root: PathBuf,
    browser_executable: Option<PathBuf>,
    proxy_url: Option<String>,
}

impl NativeRuntime {
    pub fn new(
        data_root: impl Into<PathBuf>,
        browser_executable: Option<impl Into<PathBuf>>,
        proxy_url: Option<impl Into<String>>,
    ) -> Self {
        Self {
            data_root: data_root.into(),
            browser_executable: browser_executable.map(Into::into),
            proxy_url: proxy_url
                .map(Into::into)
                .map(|value: String| value.trim().to_string())
                .filter(|value| !value.is_empty()),
        }
    }

    pub fn from_descriptor(descriptor: NativeRuntimeDescriptor) -> Self {
        Self {
            data_root: descriptor.data_root,
            browser_executable: descriptor.browser_executable,
            proxy_url: descriptor.proxy_url,
        }
    }

    pub fn descriptor(&self) -> NativeRuntimeDescriptor {
        NativeRuntimeDescriptor {
            data_root: self.data_root.clone(),
            browser_executable: self.browser_executable.clone(),
            proxy_url: self.proxy_url.clone(),
        }
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn cookies_dir(&self) -> PathBuf {
        self.data_root.join("cookies")
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.data_root.join("browser-profiles")
    }

    pub fn browser_executable(&self) -> Option<PathBuf> {
        self.browser_executable
            .as_ref()
            .filter(|path| path.is_file())
            .cloned()
            .or_else(detect_browser_executable)
    }

    pub fn browser_available(&self) -> bool {
        self.browser_executable().is_some()
    }

    pub fn proxy_url(&self) -> Option<&str> {
        self.proxy_url.as_deref()
    }

    pub fn store(&self) -> CookieStore {
        CookieStore::new(&self.data_root)
    }

    pub fn account_file(&self, account: &Account) -> PathBuf {
        self.store().account_path(account)
    }

    pub fn account_info(&self, account: &Account) -> std::io::Result<AccountFileInfo> {
        self.store().inspect(account)
    }

    pub fn discover_accounts(&self) -> std::io::Result<Vec<AccountFileInfo>> {
        self.store().list()
    }

    pub fn profile_dir(&self, account: &Account) -> PathBuf {
        self.profiles_dir()
            .join(&account.platform)
            .join(&account.profile)
    }

    pub fn qrcode_path(&self, account: &Account) -> PathBuf {
        self.cookies_dir().join(format!(
            "{}_{}_login_qrcode.png",
            account.platform, account.profile
        ))
    }

    pub fn delete_profile(&self, account: &Account) -> std::io::Result<bool> {
        let path = self.profile_dir(account);
        match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub async fn upload_video(
        &self,
        account: &Account,
        options: &VideoUploadOptions,
        dry_run: bool,
        confirm: bool,
    ) -> anyhow::Result<UploadResult> {
        // 复用已覆盖全部平台约束的类型化校验；动作始终由本进程内 Rust 上传器执行。
        let _ = NativeActionPlan::upload_video(account, options)?;
        let mut media_paths = vec![options.file.clone()];
        if account.platform == "douyin" {
            if let Some(cover) = options
                .thumbnail_portrait
                .as_ref()
                .or(options.thumbnail.as_ref())
                .or(options.thumbnail_landscape.as_ref())
            {
                media_paths.push(cover.clone());
            }
        }
        let request = UploadRequest {
            platform: account.platform.clone(),
            account_profile: account.profile.clone(),
            title: options.title.clone(),
            desc: options.description.clone(),
            tags: options.tags.clone(),
            media_paths,
            schedule: options.schedule.clone(),
            dry_run,
            confirm,
        };
        if dry_run || !confirm {
            return dispatch_video(account, options, request, Box::new(StubDriver)).await;
        }

        let previous = self.store().load_state(account)?;
        if previous.cookies.is_empty() && !self.profile_dir(account).exists() {
            anyhow::bail!(
                "账号登录态不存在，请先完成登录: {}/{}",
                account.platform,
                account.profile
            );
        }
        let headless = options.headless.unwrap_or(account.platform != "youtube");
        let browser = Arc::new(self.launch_browser(account, headless).await?);
        browser.load_storage_state(&previous).await?;
        let result = dispatch_video(account, options, request, Box::new(browser.clone())).await;
        if result.is_ok() {
            let state = browser.storage_state(previous.origins).await?;
            self.store().save_state(account, &state)?;
        }
        browser.close().await?;
        result
    }

    pub async fn upload_note(
        &self,
        account: &Account,
        options: &NoteUploadOptions,
        dry_run: bool,
        confirm: bool,
    ) -> anyhow::Result<UploadResult> {
        let _ = NativeActionPlan::upload_note(account, options)?;
        let note = if !dry_run && confirm && account.platform == "douyin" {
            if let Some(path) = options
                .note_file
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let path = Path::new(path);
                if !path.is_file() {
                    anyhow::bail!("抖音图文正文文件不存在: {}", path.display());
                }
                std::fs::read_to_string(path).map_err(|error| {
                    anyhow::anyhow!("读取抖音图文正文文件 {} 失败: {error}", path.display())
                })?
            } else {
                options.note.clone()
            }
        } else {
            options.note.clone()
        };
        if account.platform == "douyin" && note.chars().count() > 1_000 {
            anyhow::bail!(
                "抖音图文正文不能超过 1000 字符，当前为 {} 字符",
                note.chars().count()
            );
        }
        let request = UploadRequest {
            platform: account.platform.clone(),
            account_profile: account.profile.clone(),
            title: options.title.clone(),
            desc: note,
            tags: options.tags.clone(),
            media_paths: options.images.clone(),
            schedule: options.schedule.clone(),
            dry_run,
            confirm,
        };
        if dry_run || !confirm {
            return dispatch_note(account, options, request, Box::new(StubDriver)).await;
        }
        let previous = self.store().load_state(account)?;
        if previous.cookies.is_empty() && !self.profile_dir(account).exists() {
            anyhow::bail!(
                "账号登录态不存在，请先完成登录: {}/{}",
                account.platform,
                account.profile
            );
        }
        let headless = options.headless.unwrap_or(true);
        let browser = Arc::new(self.launch_browser(account, headless).await?);
        browser.load_storage_state(&previous).await?;
        let result = dispatch_note(account, options, request, Box::new(browser.clone())).await;
        if result.is_ok() {
            let state = browser.storage_state(previous.origins).await?;
            self.store().save_state(account, &state)?;
        }
        browser.close().await?;
        result
    }

    pub async fn check_account(
        &self,
        account: &Account,
        timeout: Duration,
    ) -> anyhow::Result<NativeCheckResult> {
        let info = self.account_info(account)?;
        let profile_exists = self.profile_dir(account).exists();
        if (!info.exists || !info.valid_json) && !profile_exists {
            return Ok(NativeCheckResult {
                success: false,
                current_url: String::new(),
                message: "账号登录态文件和独立浏览器 profile 均不存在。".to_string(),
            });
        }
        let future = async {
            let browser = Arc::new(self.launch_browser(account, true).await?);
            let previous = if info.exists && info.valid_json {
                self.store().load_state(account)?
            } else {
                Default::default()
            };
            browser.load_storage_state(&previous).await?;
            browser
                .goto(platform_spec(&account.platform)?.check_url)
                .await?;
            browser.sleep_ms(3_000).await?;
            let success = is_logged_in(&browser, &account.platform).await?;
            let current_url = browser.current_url().await.unwrap_or_default();
            if success {
                let state = browser.storage_state(previous.origins).await?;
                self.store().save_state(account, &state)?;
            }
            browser.close().await?;
            Ok::<_, anyhow::Error>(NativeCheckResult {
                success,
                current_url,
                message: if success {
                    if info.exists {
                        "账号在线登录状态有效。".to_string()
                    } else {
                        "已从独立浏览器 profile 恢复登录态并保存账号文件。".to_string()
                    }
                } else {
                    "账号在线登录状态已失效。".to_string()
                },
            })
        };
        tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| anyhow::anyhow!("账号在线检查超时（{} 秒）", timeout.as_secs()))?
    }

    pub fn start_login(&self, account: Account) -> NativeLoginHandle {
        let state = Arc::new(RwLock::new(NativeLoginState {
            running: true,
            success: false,
            status: "starting".to_string(),
            message: "正在启动 Rust 浏览器登录。".to_string(),
            error: None,
            current_url: String::new(),
            qrcode_path: None,
            account_file: self.account_file(&account),
            started_at: now_millis(),
            finished_at: None,
        }));
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let runtime = self.clone();
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            let result = runtime
                .run_login(account.clone(), task_state.clone(), cancel_rx)
                .await;
            if let Err(error) = result {
                let mut current = task_state.write().await;
                if current.status != "stopped" {
                    current.running = false;
                    current.success = false;
                    current.status = "failed".to_string();
                    current.message = error.to_string();
                    current.error = Some(error.to_string());
                    current.finished_at = Some(now_millis());
                }
            }
        });
        NativeLoginHandle {
            state,
            cancel_tx,
            task: Some(task),
        }
    }

    async fn run_login(
        &self,
        account: Account,
        state: Arc<RwLock<NativeLoginState>>,
        cancel_rx: watch::Receiver<bool>,
    ) -> anyhow::Result<()> {
        let browser = Arc::new(self.launch_browser(&account, false).await?);
        let qrcode_path = self.qrcode_path(&account);
        let result = self
            .run_login_with_browser(account, state, cancel_rx, browser.clone())
            .await;
        // 无论页面检测、Cookie 导出还是在线校验在哪一步失败，都主动结束浏览器；若任务被
        // 强制 abort，chromiumoxide 的 Browser::drop 仍会 kill_on_drop，避免 profile 被锁住。
        let _ = browser.close().await;
        if result.is_err() {
            let _ = std::fs::remove_file(qrcode_path);
        }
        result
    }

    async fn run_login_with_browser(
        &self,
        account: Account,
        state: Arc<RwLock<NativeLoginState>>,
        mut cancel_rx: watch::Receiver<bool>,
        browser: Arc<CdpBrowser>,
    ) -> anyhow::Result<()> {
        let spec = platform_spec(&account.platform)?;
        let previous = self.store().load_state(&account).unwrap_or_default();
        browser.load_storage_state(&previous).await?;
        browser.goto(spec.login_url).await?;
        {
            let mut current = state.write().await;
            current.status = "pending".to_string();
            current.message = if spec.interactive {
                "请在弹出的浏览器中完成账号登录。".to_string()
            } else {
                "正在等待登录二维码或扫码完成。".to_string()
            };
        }

        let deadline = tokio::time::Instant::now() + spec.login_timeout;
        let qrcode_path = self.qrcode_path(&account);
        let mut last_qrcode_capture: Option<tokio::time::Instant> = None;
        loop {
            let observation = observe_login_progress(&browser, &account.platform).await?;
            if observation.logged_in {
                {
                    let mut current = state.write().await;
                    current.status = "saving".to_string();
                    current.message = "平台已确认登录，正在保存账号登录态。".to_string();
                }
                browser.sleep_ms(2_000).await?;
                let saved = browser.storage_state(previous.origins.clone()).await?;
                self.store().save_state(&account, &saved)?;

                // 使用同一浏览器重试实际目标页校验，避免抖音 SPA 瞬时跳转或风控页造成误判。
                let mut verified_url = None;
                for attempt in 1..=3 {
                    {
                        let mut current = state.write().await;
                        current.status = "verifying".to_string();
                        current.message =
                            format!("登录态已保存，正在进行在线校验（{attempt}/3）。");
                    }
                    browser.goto(spec.check_url).await?;
                    browser.sleep_ms(2_500).await?;
                    if is_logged_in(&browser, &account.platform).await? {
                        verified_url = Some(browser.current_url().await.unwrap_or_default());
                        break;
                    }
                    if attempt < 3 {
                        browser.sleep_ms(1_000).await?;
                    }
                }

                if let Some(current_url) = verified_url {
                    let saved = browser.storage_state(saved.origins).await?;
                    self.store().save_state(&account, &saved)?;
                    let _ = std::fs::remove_file(&qrcode_path);
                    let mut current = state.write().await;
                    current.running = false;
                    current.success = true;
                    current.status = "logged_in".to_string();
                    current.message = "登录完成：账号文件已保存，并已通过在线校验。".to_string();
                    current.current_url = current_url;
                    current.qrcode_path = None;
                    current.finished_at = Some(now_millis());
                    return Ok(());
                }

                let _ = std::fs::remove_file(&qrcode_path);
                let mut current = state.write().await;
                current.running = false;
                current.success = false;
                current.status = "saved_unverified".to_string();
                current.message =
                    "扫码流程已结束，账号文件已经保存，但在线校验未通过；请稍后点击“检查账号”。"
                        .to_string();
                current.current_url = browser.current_url().await.unwrap_or_default();
                current.qrcode_path = None;
                current.finished_at = Some(now_millis());
                return Ok(());
            }

            let current_url = browser.current_url().await.unwrap_or_default();
            {
                let mut current = state.write().await;
                current.current_url = current_url;
                let should_apply_observation = observation.status != "pending"
                    || matches!(current.status.as_str(), "starting" | "pending");
                if should_apply_observation && current.status != observation.status {
                    current.status = observation.status.to_string();
                    current.message = observation.message.to_string();
                }
            }

            if observation.refresh_qrcode {
                let _ = browser.click_text("二维码失效").await;
                let _ = browser.click_text("点击刷新").await;
                browser.sleep_ms(1_000).await?;
                last_qrcode_capture = None;
            }

            let should_capture_qrcode = !spec.interactive
                && !matches!(observation.status, "scanned" | "verification_required")
                && last_qrcode_capture.is_none();
            if should_capture_qrcode {
                if let Some(path) = capture_qrcode(&browser, spec, &qrcode_path).await {
                    let mut current = state.write().await;
                    current.qrcode_path = Some(path);
                    if !matches!(current.status.as_str(), "scanned" | "verification_required") {
                        current.status = "awaiting_scan".to_string();
                        current.message =
                            "二维码已生成，状态会自动更新；请扫码并在手机端确认。".to_string();
                    }
                    last_qrcode_capture = Some(tokio::time::Instant::now());
                }
            }

            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!("等待 {} 登录超时", account.platform);
            }
            tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        let _ = std::fs::remove_file(&qrcode_path);
                        let mut current = state.write().await;
                        current.running = false;
                        current.success = false;
                        current.status = "stopped".to_string();
                        current.message = "登录任务已停止。".to_string();
                        current.qrcode_path = None;
                        current.finished_at = Some(now_millis());
                        return Ok(());
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(750)) => {}
            }
        }
    }

    async fn launch_browser(
        &self,
        account: &Account,
        headless: bool,
    ) -> anyhow::Result<CdpBrowser> {
        let mut options = BrowserLaunchOptions::new(self.profile_dir(account));
        options.executable = self.browser_executable();
        options.proxy_url = self.proxy_url.clone();
        // 抖音 cookie 检查在无头模式下会被风控误判，保持有头但隐藏窗口不可靠；先按参考实现
        // 的实际经验使用有头检查，其他平台使用 new-headless。
        options.headless = headless && account.platform != "douyin";
        CdpBrowser::launch_with(options).await
    }
}

async fn dispatch_video(
    account: &Account,
    options: &VideoUploadOptions,
    request: UploadRequest,
    driver: Box<dyn BrowserDriver>,
) -> anyhow::Result<UploadResult> {
    match account.platform.as_str() {
        "douyin" => {
            DouyinUploader::new(driver)
                .with_video_options(
                    options.thumbnail_landscape.clone(),
                    options
                        .thumbnail_portrait
                        .clone()
                        .or(options.thumbnail.clone()),
                    options.product_link.clone(),
                    options.product_title.clone(),
                )
                .upload_video(&request)
                .await
        }
        "kuaishou" => {
            KuaishouUploader::new(driver)
                .with_thumbnail(options.thumbnail.clone())
                .upload_video(&request)
                .await
        }
        "xiaohongshu" => {
            XiaohongshuUploader::new(account.clone(), driver)
                .with_thumbnail(options.thumbnail.clone())
                .upload_video(&request)
                .await
        }
        "bilibili" => {
            let category = options.category.clone();
            let mut uploader = BiliUploader::new(driver);
            if let Some(category) = category {
                uploader = uploader.with_category(category);
            }
            uploader = uploader.with_tid(options.tid);
            uploader.upload_video(&request).await
        }
        "tencent" => {
            TencentUploader::for_account(account, driver)
                .with_options(
                    options.thumbnail_landscape.clone(),
                    options
                        .thumbnail_portrait
                        .clone()
                        .or(options.thumbnail.clone()),
                    options.short_title.clone(),
                    options.category.clone(),
                    options.draft,
                )
                .upload_video(&request)
                .await
        }
        "youtube" => {
            YouTubeUploader::new(driver)
                .with_options(
                    options.thumbnail.clone(),
                    options.playlist.clone(),
                    options.visibility.as_deref().unwrap_or("public"),
                )
                .upload_video(&request)
                .await
        }
        _ => anyhow::bail!("不支持的视频发布平台: {}", account.platform),
    }
}

async fn dispatch_note(
    account: &Account,
    options: &NoteUploadOptions,
    request: UploadRequest,
    driver: Box<dyn BrowserDriver>,
) -> anyhow::Result<UploadResult> {
    match account.platform.as_str() {
        "douyin" => {
            DouyinUploader::new(driver)
                .with_note_bgm(options.bgm.clone())
                .upload_note(&request)
                .await
        }
        "kuaishou" => KuaishouUploader::new(driver).upload_note(&request).await,
        "xiaohongshu" => {
            XiaohongshuUploader::new(account.clone(), driver)
                .upload_note(&request)
                .await
        }
        _ => anyhow::bail!("不支持的图文发布平台: {}", account.platform),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeCheckResult {
    pub success: bool,
    pub current_url: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeLoginState {
    pub running: bool,
    pub success: bool,
    pub status: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub current_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qrcode_path: Option<PathBuf>,
    pub account_file: PathBuf,
    pub started_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}

pub struct NativeLoginHandle {
    state: Arc<RwLock<NativeLoginState>>,
    cancel_tx: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl NativeLoginHandle {
    pub async fn snapshot(&self) -> NativeLoginState {
        self.state.read().await.clone()
    }

    pub async fn is_running(&self) -> bool {
        self.state.read().await.running
    }

    pub async fn stop(&mut self) {
        let _ = self.cancel_tx.send(true);
        if let Some(mut task) = self.task.take() {
            let timed_out = tokio::select! {
                _ = &mut task => false,
                _ = tokio::time::sleep(Duration::from_secs(5)) => true,
            };
            if timed_out {
                task.abort();
                let _ = task.await;
                let mut current = self.state.write().await;
                if let Some(path) = current.qrcode_path.take() {
                    let _ = std::fs::remove_file(path);
                }
                current.running = false;
                current.success = false;
                current.status = "stopped".to_string();
                current.message = "登录任务已停止。".to_string();
                current.finished_at = Some(now_millis());
            }
        }
    }
}

impl Drop for NativeLoginHandle {
    fn drop(&mut self) {
        let _ = self.cancel_tx.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct PlatformSpec {
    login_url: &'static str,
    check_url: &'static str,
    qr_selectors: &'static [&'static str],
    interactive: bool,
    login_timeout: Duration,
}

fn platform_spec(platform: &str) -> anyhow::Result<&'static PlatformSpec> {
    match platform {
        "douyin" => Ok(&DOUYIN_SPEC),
        "kuaishou" => Ok(&KUAISHOU_SPEC),
        "xiaohongshu" => Ok(&XHS_SPEC),
        "bilibili" => Ok(&BILIBILI_SPEC),
        "tencent" => Ok(&TENCENT_SPEC),
        "youtube" => Ok(&YOUTUBE_SPEC),
        _ => anyhow::bail!("不支持的 Social Connection 平台: {platform}"),
    }
}

static DOUYIN_SPEC: PlatformSpec = PlatformSpec {
    login_url: DOUYIN_LOGIN_URL,
    check_url: DOUYIN_CHECK_URL,
    qr_selectors: &[
        "div#animate_qrcode_container img[src^='data:image']",
        "div[class*='animate_qrcode_container'] img[src^='data:image']",
        "div[class*='scan_qrcode_login_content'] img[src^='data:image']",
        "img[aria-label='二维码']",
    ],
    interactive: false,
    login_timeout: Duration::from_secs(600),
};
static KUAISHOU_SPEC: PlatformSpec = PlatformSpec {
    login_url: KUAISHOU_LOGIN_URL,
    check_url: KUAISHOU_CHECK_URL,
    qr_selectors: &[
        "main#login-form div.qr-login img[alt='qrcode']",
        "main#login-form img[src^='data:image']",
        "main#login-form canvas",
    ],
    interactive: false,
    login_timeout: Duration::from_secs(600),
};
static XHS_SPEC: PlatformSpec = PlatformSpec {
    login_url: XHS_LOGIN_URL,
    check_url: XHS_CHECK_URL,
    qr_selectors: &[
        ".login-box-container img[src^='data:image']",
        ".login-box-container img",
        "div[class*='login-box'] img",
    ],
    interactive: false,
    login_timeout: Duration::from_secs(600),
};
static BILIBILI_SPEC: PlatformSpec = PlatformSpec {
    login_url: BILIBILI_LOGIN_URL,
    check_url: BILIBILI_CHECK_URL,
    qr_selectors: &[
        ".qrcode-img img",
        ".qrcode-box img",
        "img[src^='data:image']",
        "canvas",
    ],
    interactive: false,
    login_timeout: Duration::from_secs(600),
};
static TENCENT_SPEC: PlatformSpec = PlatformSpec {
    login_url: TENCENT_LOGIN_URL,
    check_url: TENCENT_CHECK_URL,
    qr_selectors: &[
        "div.login-qrcode-wrap img.qrcode",
        "div.qrcode-wrap img.qrcode",
        "img.qrcode",
        "iframe[src*='login-for-iframe']",
    ],
    interactive: false,
    login_timeout: Duration::from_secs(600),
};
static YOUTUBE_SPEC: PlatformSpec = PlatformSpec {
    login_url: YOUTUBE_LOGIN_URL,
    check_url: YOUTUBE_LOGIN_URL,
    qr_selectors: &[],
    interactive: true,
    login_timeout: Duration::from_secs(900),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LoginObservation {
    logged_in: bool,
    status: &'static str,
    message: &'static str,
    refresh_qrcode: bool,
}

impl LoginObservation {
    const fn pending() -> Self {
        Self {
            logged_in: false,
            status: "pending",
            message: "正在等待平台确认登录。",
            refresh_qrcode: false,
        }
    }

    const fn logged_in() -> Self {
        Self {
            logged_in: true,
            status: "logged_in",
            message: "平台已确认登录。",
            refresh_qrcode: false,
        }
    }
}

fn classify_douyin_login(
    current_url: &str,
    visible_text: &str,
    cookie_names: &[String],
) -> LoginObservation {
    let url = current_url.to_ascii_lowercase();
    let cookie_names = cookie_names
        .iter()
        .map(|name| name.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let has_session_cookie = cookie_names.iter().any(|name| {
        matches!(
            name.as_str(),
            "sessionid" | "sessionid_ss" | "sid_guard" | "uid_tt" | "uid_tt_ss"
        )
    });
    let has_login_marker = ["扫码登录", "手机号登录", "二维码失效"]
        .iter()
        .any(|marker| visible_text.contains(marker));

    if url.contains("creator.douyin.com/creator-micro") && has_session_cookie && !has_login_marker {
        return LoginObservation::logged_in();
    }

    if visible_text.contains("二维码失效") {
        return LoginObservation {
            logged_in: false,
            status: "qr_expired",
            message: "二维码已失效，正在自动刷新二维码。",
            refresh_qrcode: true,
        };
    }

    if [
        "扫码成功",
        "已扫码",
        "请在手机上确认",
        "请在抖音内确认",
        "确认登录",
    ]
    .iter()
    .any(|marker| visible_text.contains(marker))
    {
        return LoginObservation {
            logged_in: false,
            status: "scanned",
            message: "二维码已扫描，请在抖音 App 中确认登录；页面会继续自动检测。",
            refresh_qrcode: false,
        };
    }

    if [
        "安全验证",
        "请完成验证",
        "请输入短信验证码",
        "短信验证码已发送",
    ]
    .iter()
    .any(|marker| visible_text.contains(marker))
    {
        return LoginObservation {
            logged_in: false,
            status: "verification_required",
            message: "抖音要求安全或短信验证，请在弹出的浏览器中完成；完成后会自动保存。",
            refresh_qrcode: false,
        };
    }

    if url.contains("creator.douyin.com/creator-micro") || has_session_cookie {
        return LoginObservation {
            logged_in: false,
            status: "verifying",
            message: "登录页面已跳转，正在等待抖音登录凭据落盘。",
            refresh_qrcode: false,
        };
    }

    LoginObservation::pending()
}

async fn observe_douyin_login(browser: &CdpBrowser) -> anyhow::Result<LoginObservation> {
    let current_url = browser.current_url().await?;
    let visible_text = browser
        .evaluate("document.body ? document.body.innerText : ''")
        .await?
        .as_str()
        .unwrap_or_default()
        .to_string();
    let cookie_names = browser
        .storage_state(Vec::new())
        .await?
        .cookies
        .into_iter()
        .map(|cookie| cookie.name)
        .collect::<Vec<_>>();
    Ok(classify_douyin_login(
        &current_url,
        &visible_text,
        &cookie_names,
    ))
}

async fn observe_login_progress(
    browser: &CdpBrowser,
    platform: &str,
) -> anyhow::Result<LoginObservation> {
    if platform == "douyin" {
        return observe_douyin_login(browser).await;
    }
    if is_logged_in(browser, platform).await? {
        return Ok(LoginObservation::logged_in());
    }
    Ok(LoginObservation::pending())
}

async fn is_logged_in(browser: &CdpBrowser, platform: &str) -> anyhow::Result<bool> {
    let url = browser.current_url().await?.to_ascii_lowercase();
    Ok(match platform {
        "douyin" => observe_douyin_login(browser).await?.logged_in,
        "kuaishou" => {
            url.contains("cp.kuaishou.com/article/")
                && !url.contains("passport.kuaishou.com")
                && !browser.selector_exists("main#login-form").await
        }
        "xiaohongshu" => {
            url.starts_with("https://creator.xiaohongshu.com")
                && !url.contains("/login")
                && !browser.selector_exists(".login-box-container").await
        }
        "bilibili" => {
            url.contains("member.bilibili.com/platform/upload")
                && !url.contains("passport.bilibili.com")
                && browser.is_text_present("立即投稿").await.unwrap_or(false)
        }
        "tencent" => {
            url.contains("channels.weixin.qq.com/platform/post/create")
                && !browser.is_text_present("扫码登录").await.unwrap_or(true)
        }
        "youtube" => {
            url.contains("studio.youtube.com/channel/")
                && !url.contains("accounts.google.com")
                && !url.contains("signin")
        }
        _ => false,
    })
}

async fn capture_qrcode(
    browser: &CdpBrowser,
    spec: &PlatformSpec,
    output: &Path,
) -> Option<PathBuf> {
    for selector in spec.qr_selectors {
        if browser.screenshot_element(selector, output).await.is_ok() {
            return Some(output.to_path_buf());
        }
    }
    None
}

pub fn write_native_runtime_descriptor(path: &Path, runtime: &NativeRuntime) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_vec_pretty(&runtime.descriptor())?;
    let tmp = path.with_extension(format!("json.{}.tmp", now_millis()));
    std::fs::write(&tmp, raw)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(target_os = "windows")]
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(tmp);
        return Err(error.into());
    }
    Ok(())
}

pub fn configured_native_runtime() -> NativeRuntime {
    if let Some(path) = std::env::var_os(NATIVE_RUNTIME_CONFIG_ENV).map(PathBuf::from) {
        if let Ok(raw) = std::fs::read_to_string(path) {
            if let Ok(descriptor) = serde_json::from_str::<NativeRuntimeDescriptor>(&raw) {
                return NativeRuntime::from_descriptor(descriptor);
            }
        }
    }
    let data_root = std::env::var_os("SOCIAL_CONNECTION_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("social-connection"));
    let browser = std::env::var_os("SOCIAL_CONNECTION_BROWSER").map(PathBuf::from);
    let proxy = std::env::var("SOCIAL_CONNECTION_PROXY_URL")
        .ok()
        .or_else(|| std::env::var("YT_PROXY").ok());
    NativeRuntime::new(data_root, browser, proxy)
}

pub fn native_capabilities() -> &'static [NativePlatformCapability] {
    NATIVE_PLATFORM_CAPABILITIES
}

/// 解析纯 Rust 运行时的数据根，并兼容旧版本设置中遗留的 `sauBin` launcher。
///
/// 新配置 `dataDir` 优先；未配置时复用当前进程环境；再尝试从旧 launcher 内容、`.venv`
/// 布局或 `operations-runtime/bin/sau` 的兄弟目录推断。这里只读取旧配置以找到账号文件，
/// 不会执行旧 launcher。
pub fn resolve_native_data_root(
    configured_data_dir: Option<&str>,
    legacy_sau_bin: Option<&str>,
    fallback: &Path,
) -> PathBuf {
    if let Some(path) = nonempty_path(configured_data_dir) {
        return path;
    }
    if let Some(path) = std::env::var_os("SOCIAL_CONNECTION_DATA_DIR") {
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    if let Some(sau_bin) = legacy_sau_bin
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let sau_path = Path::new(sau_bin);
        if let Some(path) = legacy_data_root_from_launcher(sau_path) {
            return path;
        }
        if let Some(path) = legacy_project_root_from_venv(sau_path) {
            return path;
        }
        if sau_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("bin"))
        {
            if let Some(runtime_root) = sau_path.parent().and_then(Path::parent) {
                let sibling = runtime_root.join("social-connection");
                if sibling.exists() {
                    return sibling;
                }
            }
        }
    }
    fallback.to_path_buf()
}

fn nonempty_path(value: Option<&str>) -> Option<PathBuf> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn legacy_project_root_from_venv(path: &Path) -> Option<PathBuf> {
    let bin_dir = path.parent()?;
    let expected = if cfg!(target_os = "windows") {
        "Scripts"
    } else {
        "bin"
    };
    if !bin_dir
        .file_name()?
        .to_str()?
        .eq_ignore_ascii_case(expected)
    {
        return None;
    }
    let venv_dir = bin_dir.parent()?;
    if venv_dir.file_name()?.to_str()? != ".venv" {
        return None;
    }
    Some(venv_dir.parent()?.to_path_buf())
}

fn legacy_data_root_from_launcher(path: &Path) -> Option<PathBuf> {
    let raw = std::fs::read_to_string(path).ok()?;
    for line in raw.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches("export ")
            .trim_start_matches("set ")
            .trim_matches('"');
        let Some((name, value)) = trimmed.split_once('=') else {
            continue;
        };
        if name.trim().trim_matches('"') != "SOCIAL_CONNECTION_DATA_DIR" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'');
        if !value.is_empty() && !value.contains('%') && !value.contains('$') {
            return Some(PathBuf::from(value));
        }
    }
    None
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime() -> NativeRuntime {
        NativeRuntime::new(
            std::env::temp_dir().join(format!("native-runtime-{}", now_millis())),
            None::<PathBuf>,
            None::<String>,
        )
    }

    #[test]
    fn profile_and_qrcode_paths_are_account_scoped() {
        let runtime = runtime();
        let account = Account::new("xhs", "staff_01");
        assert!(runtime
            .profile_dir(&account)
            .ends_with("browser-profiles/xiaohongshu/staff_01"));
        assert!(runtime
            .qrcode_path(&account)
            .ends_with("cookies/xiaohongshu_staff_01_login_qrcode.png"));
    }

    #[test]
    fn descriptor_roundtrip_preserves_native_configuration() {
        let runtime = NativeRuntime::new(
            "/tmp/social-native",
            Some("/tmp/chrome"),
            Some("http://127.0.0.1:7890"),
        );
        let restored = NativeRuntime::from_descriptor(runtime.descriptor());
        assert_eq!(restored.data_root(), Path::new("/tmp/social-native"));
        assert_eq!(restored.proxy_url(), Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn douyin_login_observation_requires_session_cookie_for_success() {
        let without_cookie = classify_douyin_login(
            "https://creator.douyin.com/creator-micro/home",
            "创作者中心",
            &[],
        );
        assert!(!without_cookie.logged_in);
        assert_eq!(without_cookie.status, "verifying");

        let with_cookie = classify_douyin_login(
            "https://creator.douyin.com/creator-micro/home",
            "创作者中心",
            &["sessionid".to_string()],
        );
        assert!(with_cookie.logged_in);
        assert_eq!(with_cookie.status, "logged_in");
    }

    #[test]
    fn douyin_login_observation_exposes_scan_and_expiry_phases() {
        let scanned = classify_douyin_login(
            "https://creator.douyin.com/",
            "扫码成功，请在手机上确认",
            &[],
        );
        assert_eq!(scanned.status, "scanned");
        assert!(!scanned.refresh_qrcode);

        let expired =
            classify_douyin_login("https://creator.douyin.com/", "二维码失效 点击刷新", &[]);
        assert_eq!(expired.status, "qr_expired");
        assert!(expired.refresh_qrcode);
    }

    #[test]
    fn legacy_launcher_only_migrates_data_root_without_execution() {
        let root = std::env::temp_dir().join(format!("legacy-social-{}", now_millis()));
        let launcher = root.join("bin/sau");
        let account_root = root.join("account data");
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::fs::write(
            &launcher,
            format!(
                "#!/bin/sh\nexport SOCIAL_CONNECTION_DATA_DIR='{}'\nexit 99\n",
                account_root.display()
            ),
        )
        .unwrap();
        assert_eq!(
            resolve_native_data_root(None, launcher.to_str(), Path::new("/fallback")),
            account_root
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn missing_account_check_fails_without_launching_browser() {
        let runtime = runtime();
        let result = runtime
            .check_account(&Account::new("douyin", "missing"), Duration::from_secs(1))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.message.contains("不存在"));
    }

    #[test]
    fn native_capability_matrix_covers_all_six_platforms() {
        let platforms = native_capabilities()
            .iter()
            .map(|capability| capability.platform)
            .collect::<Vec<_>>();
        assert_eq!(
            platforms,
            vec![
                "douyin",
                "kuaishou",
                "xiaohongshu",
                "bilibili",
                "tencent",
                "youtube"
            ]
        );
        assert!(native_capabilities()
            .iter()
            .all(|capability| capability.login && capability.check && capability.upload_video));
    }

    #[tokio::test]
    async fn native_dry_run_dispatches_every_supported_upload_without_browser() {
        let runtime = runtime();
        for platform in [
            "douyin",
            "kuaishou",
            "xiaohongshu",
            "bilibili",
            "tencent",
            "youtube",
        ] {
            let account = Account::new(platform, "qa");
            let options = VideoUploadOptions {
                file: "/path/does/not/need/to/exist-in-dry-run.mp4".into(),
                title: format!("{platform} dry run"),
                description: "纯 Rust 发布计划".into(),
                tid: (platform == "bilibili").then_some(17),
                visibility: (platform == "youtube").then_some("private".into()),
                ..Default::default()
            };
            let result = runtime
                .upload_video(&account, &options, true, false)
                .await
                .unwrap_or_else(|error| panic!("{platform} dry-run 分发失败: {error}"));
            assert!(result.dry_run, "{platform} 应返回 dry-run 计划");
        }

        for platform in ["douyin", "kuaishou", "xiaohongshu"] {
            let account = Account::new(platform, "qa");
            let options = NoteUploadOptions {
                images: vec!["/path/does/not/need/to/exist-in-dry-run.png".into()],
                title: format!("{platform} note dry run"),
                note: "纯 Rust 图文发布计划".into(),
                ..Default::default()
            };
            let result = runtime
                .upload_note(&account, &options, true, false)
                .await
                .unwrap_or_else(|error| panic!("{platform} note dry-run 分发失败: {error}"));
            assert!(result.dry_run, "{platform} 图文应返回 dry-run 计划");
        }
    }

    #[tokio::test]
    async fn douyin_note_file_is_resolved_in_rust_before_browser_launch() {
        let runtime = runtime();
        let account = Account::new("douyin", "qa");
        let options = NoteUploadOptions {
            images: vec!["image.png".into()],
            title: "标题".into(),
            note_file: Some("/definitely/missing/social-note.md".into()),
            ..Default::default()
        };
        let error = runtime
            .upload_note(&account, &options, false, true)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("正文文件不存在"));
    }
}
