//! 微信视频号（channels.weixin.qq.com）发布器。
//!
//! 移植自 sau `uploader/tencent_uploader/main.py`（`TencentBaseUploader` / `TencentVideo`），
//! 把 Playwright page.* 操作映射到 [`crate::browser::BrowserDriver`] 抽象，便于单测与解耦。
//!
//! ## 流程（对齐 sau）
//!
//! 1. `goto` 首页 [`TENCENT_LOGIN_URL`]，检测登录态：出现 [`LOGIN_PROMPT_TEXT`]（"扫码登录"）
//!    则 cookie 失效 → 报错（登录/扫码由 `crate::account` + 未来的 login 模块负责，上传器假定已登录）。
//! 2. `goto` [`TENCENT_UPLOAD_URL`]，必要时点 [`LOGGED_IN_TEXT`]（"发表视频"）唤出编辑器。
//! 3. `set_input_files` [`FILE_INPUT_SELECTOR`] 上传视频。
//! 4. 在 [`DESC_EDITOR_SELECTOR`]（`div.input-editor`）填写 title / `#tag` / desc。
//! 5. 等待上传完成，点击 [`PUBLISH_BUTTON_SELECTOR`]（"发表"），`wait_for_url_contains`
//!    [`POST_LIST_FRAGMENT`]（`post/list`）确认发布成功。
//!
//! ## 安全闸
//!
//! `UploadRequest.dry_run=true` 或 `confirm=false` 时，不触碰 driver，返回
//! `UploadResult{ dry_run:true, note:"planned: ..." }`；仅 `!dry_run && confirm` 才走真实流程。
//!
//! 视频号图文（[`Uploader::upload_note`]）在参考实现中仍为 `NotImplementedError`，因此能力
//! 矩阵明确标记为不支持；视频发布已覆盖键盘输入、上传完成检测、原创声明、双封面、排期与草稿。

use crate::account::Account;
use crate::browser::{BrowserDriver, StubDriver};
use crate::uploader::{UploadRequest, UploadResult, Uploader};

// ===== URL 常量（提取自 sau main.py，便于流程单测断言）=====

/// 视频号助手首页 / 登录入口（sau: `TENCENT_LOGIN_URL`）。
pub const TENCENT_LOGIN_URL: &str = "https://channels.weixin.qq.com";
/// 视频上传/编辑页（sau: `TENCENT_UPLOAD_URL`）。
pub const TENCENT_UPLOAD_URL: &str = "https://channels.weixin.qq.com/platform/post/create";
/// 发布成功后跳转的管理列表页（sau: `TENCENT_MANAGE_URL`）。
pub const TENCENT_MANAGE_URL: &str = "https://channels.weixin.qq.com/platform/post/list";
/// 上传页 URL 片段。
pub const POST_CREATE_FRAGMENT: &str = "post/create";
/// 管理列表页 URL 片段（发布成功的判定片段）。
pub const POST_LIST_FRAGMENT: &str = "post/list";

// ===== 登录态检测文本（提取自 sau cookie_auth / _is_tencent_login_completed）=====

/// 出现该文本 → cookie 失效、需重新扫码登录（sau: `get_by_text("扫码登录", exact=True)`）。
pub const LOGIN_PROMPT_TEXT: &str = "扫码登录";
/// 登录页 iframe 内的标题文本（sau: `span:has-text("微信扫码登录 视频号助手")`）。
pub const LOGIN_QRCODE_TITLE_TEXT: &str = "微信扫码登录 视频号助手";
/// 登录后首页/编辑器入口文本（sau: `get_by_text("发表视频")`）。
pub const LOGGED_IN_TEXT: &str = "发表视频";
/// 发布按钮文本（sau: `button:has-text("发表")`）。
pub const PUBLISH_BUTTON_TEXT: &str = "发表";
/// 保存草稿按钮文本（sau: `button:has-text("保存草稿")`）。
pub const DRAFT_BUTTON_TEXT: &str = "保存草稿";

// ===== RPA 选择器（sau main.py；平台改版会漂移，见 coverage_notes）=====

/// 视频/封面文件输入框（sau: `input[type="file"]`，跨主 frame + iframe 搜索）。
pub const FILE_INPUT_SELECTOR: &str = r#"input[type="file"]"#;
/// 标题/描述编辑器（sau: `div.input-editor`，点击后 `keyboard.type`）。
pub const DESC_EDITOR_SELECTOR: &str = "div.input-editor";
/// 发布按钮（sau: `div.form-btns button:has-text("发表")`）。
pub const PUBLISH_BUTTON_SELECTOR: &str = "div.form-btns button";
/// 保存草稿按钮（sau: `div.form-btns button:has-text("保存草稿")`）。
pub const DRAFT_BUTTON_SELECTOR: &str = "div.form-btns button";
/// 登录二维码所在 iframe（sau: `[src*="login-for-iframe"]`）。
pub const QRCODE_IFRAME_SELECTOR: &str = r#"[src*="login-for-iframe"]"#;
/// 登录二维码 img（sau: `img.qrcode` / `div.login-qrcode-wrap img.qrcode`）。
pub const QRCODE_IMG_SELECTOR: &str = "img.qrcode";

/// 视频号平台标识（`Uploader::platform`）。
pub const PLATFORM_NAME: &str = "tencent";

/// 视频号视频发布流程的可审计步骤列表（用于 dry_run 计划说明与单测断言）。
///
/// 故意内联 URL 片段与选择器片段，使单测可交叉校验计划与 sau 常量一致。
pub fn plan_video_steps() -> Vec<&'static str> {
    vec![
        "goto https://channels.weixin.qq.com (login/home)",
        "detect cookie: is_text_present(\"扫码登录\") must be absent",
        "goto /platform/post/create (upload page)",
        "optionally click_text(\"发表视频\") to open editor",
        "set_input_files input[type=file] with media_paths",
        "fill div.input-editor (title + #tags + desc)",
        "wait upload done (sleep_ms / is_text_present(\"发表\"))",
        "click_text(\"发表\") on div.form-btns button",
        "wait_for_url_contains post/list",
    ]
}

/// 视频号（微信视频号助手）发布器。持有 [`BrowserDriver`] trait 对象驱动 RPA。
///
/// 默认 [`TencentUploader::with_stub`] 用 [`StubDriver`] 占位（无 CDP 后端时）；
/// 真实发布由 `chromiumoxide` 后端（`feature=cdp`）注入实现。
pub struct TencentUploader {
    driver: Box<dyn BrowserDriver>,
    /// 关联的本地账号 profile（来自 [`Account`]），仅用于结果备注，不影响驱动。
    profile: Option<String>,
    thumbnail_landscape: Option<String>,
    thumbnail_portrait: Option<String>,
    short_title: Option<String>,
    category: Option<String>,
    draft: bool,
}

impl TencentUploader {
    /// 由任意 [`BrowserDriver`] 实现构造（生产：CDP 后端）。
    pub fn new(driver: Box<dyn BrowserDriver>) -> Self {
        Self {
            driver,
            profile: None,
            thumbnail_landscape: None,
            thumbnail_portrait: None,
            short_title: None,
            category: None,
            draft: false,
        }
    }

    /// 绑定某账号 profile 构造（便于在结果 note 中标注归属）。
    pub fn for_account(account: &Account, driver: Box<dyn BrowserDriver>) -> Self {
        Self {
            driver,
            profile: Some(account.profile.clone()),
            thumbnail_landscape: None,
            thumbnail_portrait: None,
            short_title: None,
            category: None,
            draft: false,
        }
    }

    pub fn with_options(
        mut self,
        thumbnail_landscape: Option<String>,
        thumbnail_portrait: Option<String>,
        short_title: Option<String>,
        category: Option<String>,
        draft: bool,
    ) -> Self {
        self.thumbnail_landscape = thumbnail_landscape;
        self.thumbnail_portrait = thumbnail_portrait;
        self.short_title = short_title;
        self.category = category;
        self.draft = draft;
        self
    }

    /// 用 [`StubDriver`] 占位构造（无 CDP 环境的默认/单测用）。
    pub fn with_stub() -> Self {
        Self::new(Box::new(StubDriver))
    }

    fn who(&self) -> String {
        self.profile
            .clone()
            .unwrap_or_else(|| "<no-profile>".to_string())
    }

    /// 构造 dry_run / 安全闸拦截后的计划结果（不触碰 driver）。
    fn planned_video_result(&self, reason: &str) -> UploadResult {
        UploadResult {
            platform_post_id: None,
            url: None,
            dry_run: true,
            note: format!(
                "planned: tencent[weixin-channel] video profile={} — {} | flow: {}",
                self.who(),
                reason,
                plan_video_steps().join(" -> ")
            ),
        }
    }

    /// 真实执行视频号视频发布流程（仅在 `!dry_run && confirm` 时调用）。
    ///
    /// 所有 driver 调用按 [`plan_video_steps`] 顺序；任一步失败即向上冒泡 `Err`。
    async fn run_video_flow(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        let driver = self.driver.as_ref();

        if req.media_paths.is_empty() {
            return Err(anyhow::anyhow!(
                "视频号视频模式：UploadRequest.media_paths 为空（需要 1 个视频文件）"
            ));
        }

        // 1) 打开首页并判定登录态。出现"扫码登录"即 cookie 失效 → 报错，交由上层重新登录。
        driver.goto(TENCENT_LOGIN_URL).await?;
        driver.sleep_ms(1500).await?;
        if driver.is_text_present(LOGIN_PROMPT_TEXT).await? {
            return Err(anyhow::anyhow!(
                "视频号 cookie 失效：页面出现「{}」（{}），请先完成扫码登录",
                LOGIN_PROMPT_TEXT,
                LOGIN_QRCODE_TITLE_TEXT
            ));
        }

        // 2) 进入上传/编辑页。
        driver.goto(TENCENT_UPLOAD_URL).await?;
        driver.wait_for_url_contains(POST_CREATE_FRAGMENT).await?;
        // 落在首页时点"发表视频"唤出编辑器（失败不致命：可能已直达编辑器）。
        if !driver
            .is_text_present(LOGGED_IN_TEXT)
            .await
            .unwrap_or(false)
        {
            let _ = driver.click_text(LOGGED_IN_TEXT).await;
        }
        driver.sleep_ms(1000).await?;

        // 3) 选择视频文件并上传。
        let files: Vec<String> = req.media_paths.to_vec();
        driver.set_input_files(FILE_INPUT_SELECTOR, &files).await?;

        // 4) 填写标题 / 标签 / 描述。
        //    sau 原流程：click(div.input-editor) → keyboard.type(title) → Enter →
        //    每个 tag keyboard.type("#"+tag)+Space → keyboard.type(desc)。
        driver.click_selector(DESC_EDITOR_SELECTOR).await?;
        driver.fill(DESC_EDITOR_SELECTOR, &req.title).await?;
        driver.press_key("Enter").await?;
        for tag in &req.tags {
            driver
                .type_text(DESC_EDITOR_SELECTOR, &format!("#{tag}"))
                .await?;
            driver.press_key("Space").await?;
        }
        if !req.desc.is_empty() {
            driver.press_key("Enter").await?;
            driver.type_text(DESC_EDITOR_SELECTOR, &req.desc).await?;
        }

        self.set_short_title(req).await?;
        self.set_thumbnails().await?;
        self.apply_original_statement().await;
        self.set_schedule_if_needed(req).await?;

        // 5) 等待上传完成：找到“发表”按钮且 class 不含 disabled。
        let mut ready = self.draft;
        for _ in 0..900 {
            let enabled = driver
                .evaluate("(()=>{const b=[...document.querySelectorAll('div.form-btns button')].find(e=>e.textContent&&e.textContent.trim()==='发表');return Boolean(b&&!String(b.className).includes('disabled')&&!b.disabled);})()")
                .await
                .ok()
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if enabled || self.draft {
                ready = true;
                break;
            }
            driver.sleep_ms(2_000).await?;
        }
        if !ready {
            anyhow::bail!("视频号素材上传或页面校验超时，发表按钮仍不可用");
        }

        // 6) 点击「发表」或「保存草稿」，并等待跳转到管理列表页。
        driver
            .click_text(if self.draft {
                "保存草稿"
            } else {
                PUBLISH_BUTTON_TEXT
            })
            .await?;
        driver.wait_for_url_contains(POST_LIST_FRAGMENT).await?;
        let final_url = driver.current_url().await.unwrap_or_default();

        Ok(UploadResult {
            platform_post_id: None,
            url: Some(final_url),
            dry_run: false,
            note: format!("视频号视频已提交发布 profile={}", self.who()),
        })
    }

    async fn set_short_title(&self, req: &UploadRequest) -> anyhow::Result<()> {
        let selector = "input[placeholder*='短标题'], input[placeholder*='请输入短标题']";
        if !self.driver.selector_exists(selector).await.unwrap_or(false) {
            return Ok(());
        }
        let fallback: String = req
            .title
            .chars()
            .filter(|value| value.is_alphanumeric() || matches!(value, ' ' | '-' | '_'))
            .take(16)
            .collect();
        self.driver
            .fill(
                selector,
                self.short_title.as_deref().unwrap_or(fallback.trim()),
            )
            .await
    }

    async fn set_single_thumbnail(&self, label: &str, path: &str) -> anyhow::Result<()> {
        if self.driver.click_text(label).await.is_err() {
            return Ok(());
        }
        self.driver.sleep_ms(500).await?;
        let selector = "div.weui-desktop-dialog .single-cover-uploader-wrap input[type='file'], div.weui-desktop-dialog input[type='file']";
        self.driver.wait_for_selector(selector, 10_000).await?;
        self.driver
            .set_input_files(selector, &[path.to_string()])
            .await?;
        self.driver.sleep_ms(700).await?;
        let _ = self.driver.click_text("确定").await;
        self.driver.sleep_ms(500).await?;
        let _ = self.driver.click_text("确认").await;
        Ok(())
    }

    async fn set_thumbnails(&self) -> anyhow::Result<()> {
        if let Some(path) = self.thumbnail_landscape.as_deref() {
            self.set_single_thumbnail("设置横版封面", path).await?;
        }
        if let Some(path) = self.thumbnail_portrait.as_deref() {
            self.set_single_thumbnail("设置竖版封面", path).await?;
        }
        Ok(())
    }

    async fn apply_original_statement(&self) {
        for text in ["声明原创", "原创声明", "视频为原创"] {
            if self.driver.click_text(text).await.is_ok() {
                if let Some(category) = self.category.as_deref() {
                    let _ = self.driver.click_text(category).await;
                }
                let _ = self.driver.click_text("我已阅读并同意").await;
                let _ = self.driver.click_text("声明原创").await;
                break;
            }
        }
    }

    async fn set_schedule_if_needed(&self, req: &UploadRequest) -> anyhow::Result<()> {
        let Some(schedule) = req.schedule.as_deref() else {
            return Ok(());
        };
        self.driver.click_text("定时").await?;
        let selector = "input[placeholder='请选择发表时间']";
        self.driver.wait_for_selector(selector, 10_000).await?;
        self.driver.fill(selector, schedule).await
    }
}

#[async_trait::async_trait]
impl Uploader for TencentUploader {
    fn platform(&self) -> &'static str {
        PLATFORM_NAME
    }

    /// 视频号图文（笔记）发布。
    ///
    /// sau 的 `TencentNote` 中 `switch_to_note_mode` / `upload_note_images` /
    /// `fill_note_title_and_tags` 仍为 `NotImplementedError`，故此处同样未落地；
    /// dry_run / 未 confirm 时返回计划结果，显式 confirm 才如实返回未实现错误。
    async fn upload_note(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        if req.dry_run || !req.confirm {
            return Ok(UploadResult {
                platform_post_id: None,
                url: None,
                dry_run: true,
                note: format!(
                    "planned: tencent[weixin-channel] note (sau TencentNote 未实现) profile={} | flow: 待定 switch_to_note_mode -> upload images -> fill title/tags -> publish",
                    self.who()
                ),
            });
        }
        Err(anyhow::anyhow!(
            "视频号图文(note)发布尚未实现：sau TencentNote 仍为 NotImplementedError"
        ))
    }

    /// 视频号视频发布。
    ///
    /// 安全闸：`dry_run=true` → 直接返回 planned（不调 driver）；
    /// `confirm=false` → 同样返回 planned（标注 safety gate）；
    /// 仅 `!dry_run && confirm` 才执行 [`Self::run_video_flow`]。
    async fn upload_video(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        if req.dry_run {
            return Ok(self.planned_video_result("dry_run=true"));
        }
        if !req.confirm {
            return Ok(self.planned_video_result("confirm=false (safety gate)"));
        }
        self.run_video_flow(req).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn sample_video_request(dry_run: bool, confirm: bool) -> UploadRequest {
        UploadRequest {
            platform: PLATFORM_NAME.to_string(),
            account_profile: "staff_wx_01".to_string(),
            title: "测试视频标题".to_string(),
            desc: "一段描述".to_string(),
            tags: vec!["日常".to_string(), "vlog".to_string()],
            media_paths: vec!["/tmp/sample.mp4".to_string()],
            schedule: None,
            dry_run,
            confirm,
        }
    }

    /// 记录型 driver：记录每次调用，便于断言"是否触发了 driver"。
    /// dry_run 时调用列表应保持空；confirm 时应含 goto/set_input_files/publish。
    #[derive(Clone, Default)]
    struct RecordingDriver {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl RecordingDriver {
        fn rec(&self, s: impl Into<String>) {
            self.calls.lock().unwrap().push(s.into());
        }
    }

    #[async_trait::async_trait]
    impl BrowserDriver for RecordingDriver {
        async fn goto(&self, url: &str) -> anyhow::Result<()> {
            self.rec(format!("goto {url}"));
            Ok(())
        }
        async fn current_url(&self) -> anyhow::Result<String> {
            self.rec("current_url");
            Ok(TENCENT_MANAGE_URL.to_string())
        }
        async fn is_text_present(&self, text: &str) -> anyhow::Result<bool> {
            self.rec(format!("is_text_present {text}"));
            // 模拟已登录："扫码登录" 不出现 → false，继续流程。
            Ok(false)
        }
        async fn click_text(&self, text: &str) -> anyhow::Result<()> {
            self.rec(format!("click_text {text}"));
            Ok(())
        }
        async fn click_selector(&self, sel: &str) -> anyhow::Result<()> {
            self.rec(format!("click_selector {sel}"));
            Ok(())
        }
        async fn fill(&self, sel: &str, val: &str) -> anyhow::Result<()> {
            self.rec(format!("fill {sel}={}chars", val.chars().count()));
            Ok(())
        }
        async fn type_text(&self, sel: &str, val: &str) -> anyhow::Result<()> {
            self.rec(format!("type_text {sel}={}chars", val.chars().count()));
            Ok(())
        }
        async fn set_input_files(&self, sel: &str, paths: &[String]) -> anyhow::Result<()> {
            self.rec(format!("set_input_files {sel} x{}", paths.len()));
            Ok(())
        }
        async fn wait_for_url_contains(&self, frag: &str) -> anyhow::Result<()> {
            self.rec(format!("wait_for_url_contains {frag}"));
            Ok(())
        }
        async fn evaluate(&self, script: &str) -> anyhow::Result<serde_json::Value> {
            self.rec(format!("evaluate {}chars", script.len()));
            Ok(serde_json::Value::Bool(true))
        }
        async fn press_key(&self, key: &str) -> anyhow::Result<()> {
            self.rec(format!("press_key {key}"));
            Ok(())
        }
        async fn sleep_ms(&self, ms: u64) -> anyhow::Result<()> {
            self.rec(format!("sleep_ms {ms}"));
            Ok(())
        }
    }

    // === 必需单测 1：dry_run=true 用 StubDriver 不实际执行，返回 planned ===
    #[tokio::test]
    async fn dry_run_with_stub_returns_planned_without_exec() {
        // StubDriver 的 goto 会返回 Err；若 dry_run 误触 driver，测试会失败/出错。
        let up = TencentUploader::with_stub();
        let req = sample_video_request(true, false);
        let res = up.upload_video(&req).await.expect("dry_run 不应报错");

        assert!(res.dry_run, "dry_run 结果应原样标记 dry_run=true");
        assert!(
            res.note.starts_with("planned:"),
            "note 应以 'planned:' 开头，实际: {}",
            res.note
        );
        assert!(res.url.is_none(), "dry_run 不应给出 url");
        assert!(res.platform_post_id.is_none());
    }

    // === 必需单测 2：URL 常量与登录检测文本与 sau 一致；计划步骤可审计 ===
    #[test]
    fn url_constants_and_plan_match_sau() {
        // URL 常量回归 sau main.py
        assert_eq!(TENCENT_LOGIN_URL, "https://channels.weixin.qq.com");
        assert_eq!(
            TENCENT_UPLOAD_URL,
            "https://channels.weixin.qq.com/platform/post/create"
        );
        assert_eq!(
            TENCENT_MANAGE_URL,
            "https://channels.weixin.qq.com/platform/post/list"
        );
        assert_eq!(POST_LIST_FRAGMENT, "post/list");
        assert_eq!(POST_CREATE_FRAGMENT, "post/create");

        // 登录态检测文本（sau cookie_auth / _is_tencent_login_completed）
        assert_eq!(LOGIN_PROMPT_TEXT, "扫码登录");
        assert_eq!(LOGIN_QRCODE_TITLE_TEXT, "微信扫码登录 视频号助手");
        assert_eq!(LOGGED_IN_TEXT, "发表视频");
        assert_eq!(PUBLISH_BUTTON_TEXT, "发表");

        // 选择器锚点
        assert_eq!(FILE_INPUT_SELECTOR, r#"input[type="file"]"#);
        assert_eq!(DESC_EDITOR_SELECTOR, "div.input-editor");
        assert!(PUBLISH_BUTTON_SELECTOR.contains("form-btns"));
        assert_eq!(PUBLISH_BUTTON_SELECTOR, "div.form-btns button");

        // 计划步骤包含 sau URL / 片段，证明计划与常量一致
        let joined = plan_video_steps().join(" || ");
        assert!(
            joined.contains("channels.weixin.qq.com"),
            "plan 应含登录首页 URL"
        );
        assert!(joined.contains("post/create"), "plan 应含上传页片段");
        assert!(joined.contains("post/list"), "plan 应含发布成功片段");
        assert!(joined.contains("扫码登录"), "plan 应含登录态检测文本");
        assert!(joined.contains("input[type=file]"), "plan 应含文件选择器");
        assert_eq!(plan_video_steps().len(), 9);
    }

    // === 加强测 3：dry_run 不触发任何 driver 调用（用 RecordingDriver 证明）===
    #[tokio::test]
    async fn dry_run_does_not_invoke_driver_at_all() {
        let rec = RecordingDriver::default();
        let up = TencentUploader::new(Box::new(rec.clone()));
        let req = sample_video_request(true, true); // 即便 confirm=true，dry_run 仍优先短路

        let res = up.upload_video(&req).await.unwrap();
        assert!(res.dry_run);
        assert!(res.note.starts_with("planned:"));
        assert!(
            rec.calls.lock().unwrap().is_empty(),
            "dry_run 绝不应触发任何 driver 调用"
        );
    }

    // === 加强测 4：confirm=true 进入真实流程，driver 调用序列覆盖关键步骤 ===
    #[tokio::test]
    async fn confirm_runs_full_flow_and_publishes() {
        let rec = RecordingDriver::default();
        let up = TencentUploader::new(Box::new(rec.clone()));
        let req = sample_video_request(false, true); // 真实执行

        let res = up
            .upload_video(&req)
            .await
            .expect("RecordingDriver 全 Ok，流程应成功");

        assert!(!res.dry_run, "confirm 后 dry_run 应为 false");
        assert_eq!(res.url.as_deref(), Some(TENCENT_MANAGE_URL));

        let calls = rec.calls.lock().unwrap().clone();
        let joined = calls.join("\n");
        // 关键步骤顺序锚点
        assert!(
            joined.contains("goto https://channels.weixin.qq.com"),
            "应先打开首页: {}",
            joined
        );
        assert!(joined.contains("goto https://channels.weixin.qq.com/platform/post/create"));
        assert!(
            joined.contains("set_input_files input[type=\"file\"] x1"),
            "应上传 1 个视频: {}",
            joined
        );
        assert!(
            joined.contains("fill div.input-editor"),
            "应填写标题/描述: {}",
            joined
        );
        assert!(joined.contains("click_text 发表"), "应点击发表: {}", joined);
        assert!(
            joined.contains("wait_for_url_contains post/list"),
            "应等待发布成功跳转: {}",
            joined
        );
        // 登录态检测确实发生过
        assert!(joined.contains("is_text_present 扫码登录"));
    }

    // === 加强测 5：cookie 失效时（页面出现"扫码登录"）真实流程应报错 ===
    #[tokio::test]
    async fn confirm_errors_when_cookie_invalid() {
        // 一个"已失效 cookie"的 driver：is_text_present("扫码登录") 返回 true。
        struct ExpiredDriver;
        #[async_trait::async_trait]
        impl BrowserDriver for ExpiredDriver {
            async fn goto(&self, _url: &str) -> anyhow::Result<()> {
                Ok(())
            }
            async fn current_url(&self) -> anyhow::Result<String> {
                Ok(String::new())
            }
            async fn is_text_present(&self, text: &str) -> anyhow::Result<bool> {
                Ok(text == LOGIN_PROMPT_TEXT)
            }
            async fn click_text(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            async fn click_selector(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            async fn fill(&self, _: &str, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            async fn set_input_files(&self, _: &str, _: &[String]) -> anyhow::Result<()> {
                Ok(())
            }
            async fn wait_for_url_contains(&self, _: &str) -> anyhow::Result<()> {
                Ok(())
            }
            async fn sleep_ms(&self, _: u64) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let up = TencentUploader::new(Box::new(ExpiredDriver));
        let req = sample_video_request(false, true);
        let err = up.upload_video(&req).await.expect_err("cookie 失效应报错");
        let msg = format!("{err}");
        assert!(
            msg.contains("cookie 失效"),
            "错误信息应说明 cookie 失效: {}",
            msg
        );
        assert!(msg.contains(LOGIN_PROMPT_TEXT));
    }

    // === 加强测 6：Uploader trait 接口与 platform 名称 ===
    #[tokio::test]
    async fn uploader_platform_name_and_note_unimplemented() {
        let up = TencentUploader::with_stub();
        assert_eq!(up.platform(), PLATFORM_NAME);
        assert_eq!(up.platform(), "tencent");

        // note 模式 dry_run 返回 planned（不报错）
        let req = sample_video_request(true, false);
        let note_res = up.upload_note(&req).await.unwrap();
        assert!(note_res.dry_run);
        assert!(note_res.note.contains("planned:"));

        // note 模式 confirm 后如实返回未实现错误
        let req2 = sample_video_request(false, true);
        let err = up.upload_note(&req2).await.unwrap_err();
        assert!(format!("{err}").contains("尚未实现"));
    }
}
