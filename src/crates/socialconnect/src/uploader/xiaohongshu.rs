//! 小红书上传器（creator.xiaohongshu.com）— 图文笔记 + 视频。
//!
//! 移植自 sau（social-auto-upload）Python 实现
//! `social-connection/uploader/xiaohongshu_uploader/main.py`。
//!
//! # 流程
//!
//! 1. `goto` 创作者中心首页 → 登录态检测（落到 `/login` 或出现「扫一扫」二维码文本即未登录）。
//! 2. 图文：`goto /publish/publish?...target=image` → `set_input_files` 图片 → 填标题/正文/标签
//!    → `click_text("发布")` → `wait_for_url_contains("publish/success")`。
//! 3. 视频：`goto /publish/publish?...target=video` → `set_input_files` 视频 → 轮询「上传成功」
//!    → 填标题/正文/标签 → `click_text("发布")` → `wait_for_url_contains("publish/success")`。
//!
//! # 安全闸
//!
//! [`Uploader::upload_note`] / [`Uploader::upload_video`] 默认只规划：当
//! [`UploadRequest::dry_run`] 为真，或 [`UploadRequest::confirm`] 为假时，**不触碰 driver**，
//! 直接返回 `UploadResult { dry_run: true, note: "planned: ..." }`；只有 `confirm == true`
//! 且 `dry_run == false` 才真正驱动浏览器发布。
//!
//! # RPA 选择器易变性
//!
//! 下列 `SEL_*` 常量随小红书前端版本变化，CI 仅断言其与 sau 当前一致，真实发布需用
//! 已登录账号在 `creator.xiaohongshu.com` 上回归验证（见 `coverage_notes`）。

use crate::account::Account;
use crate::browser::BrowserDriver;
use crate::uploader::{UploadRequest, UploadResult, Uploader};
use async_trait::async_trait;

// =========================================================================
// URL 常量（提取自 sau main.py：XHS_DEFAULT_CREATOR_BASE_URL + 发布路径）
// =========================================================================

/// 创作者中心基地址（sau: `XHS_DEFAULT_CREATOR_BASE_URL`）。
pub const XHS_CREATOR_BASE_URL: &str = "https://creator.xiaohongshu.com";

/// 登录页路径（sau: `_is_xhs_login_completed` 用 `url.startswith(".../login")` 判定未登录）。
pub const XHS_LOGIN_PATH: &str = "/login";

/// 图文发布页路径（sau `XiaoHongShuNote.upload_note_content`：`target=image`）。
pub const XHS_NOTE_PUBLISH_PATH: &str = "/publish/publish?from=homepage&target=image";

/// 视频发布页路径（sau `XiaoHongShuVideo.upload_video_content` + `cookie_auth`：`target=video`）。
pub const XHS_VIDEO_PUBLISH_PATH: &str = "/publish/publish?from=homepage&target=video";

/// 发布成功 URL 片段（sau: `XHS_PUBLISH_SUCCESS_URL_PATTERN = "**/publish/success?**"`）。
pub const XHS_PUBLISH_SUCCESS_FRAGMENT: &str = "publish/success";

// =========================================================================
// 登录检测文本（提取自 sau `_save_xhs_qrcode` / `_find_xhs_qrcode_locator`）
// =========================================================================

/// 登录二维码「扫一扫」文本（出现即视为未登录）。
pub const XHS_LOGIN_TEXT_SCAN: &str = "扫一扫";
/// 登录区「APP扫一扫登录」标题文本。
pub const XHS_LOGIN_TEXT_SCAN_LOGIN: &str = "APP扫一扫登录";
/// 视频上传成功标识文本（sau 预览区多关键词之一，取最稳定项）。
pub const XHS_VIDEO_UPLOAD_DONE_TEXT: &str = "上传成功";

// =========================================================================
// RPA 选择器（提取自 sau main.py，随平台前端版本变化）
// =========================================================================

/// 登录框容器（sau: `XHS_LOGIN_BOX_SELECTOR = "div[class*='login-box']"`）。
pub const SEL_LOGIN_BOX: &str = "div[class*='login-box']";
/// 标题输入框（sau: `input[placeholder*="填写标题"]`）。
pub const SEL_TITLE: &str = "input[placeholder*=\"填写标题\"]";
/// 正文输入区（sau: `p[data-placeholder*="输入正文描述"]`）。
pub const SEL_DESC: &str = "p[data-placeholder*=\"输入正文描述\"]";
/// 图文图片上传 file input（sau 优先项：`input[type="file"][accept*="image"]`）。
pub const SEL_NOTE_IMAGE_INPUT: &str = "input[type=\"file\"][accept*=\"image\"]";
/// 上传组件通用 file input（sau 兜底项：`div[class^='upload-content'] input[class='upload-input']`）。
pub const SEL_UPLOAD_INPUT: &str = "div[class^='upload-content'] input[class='upload-input']";
/// 视频重传 file input（sau `handle_upload_error`：`div.progress-div [class^="upload-btn-input"]`）。
pub const SEL_VIDEO_RETRY_INPUT: &str = "div.progress-div [class^=\"upload-btn-input\"]";
/// 话题候选容器（sau `fill_tags`：`#creator-editor-topic-container`）。
pub const SEL_TOPIC_CONTAINER: &str = "#creator-editor-topic-container";
/// 话题候选第一项（sau `fill_tags`：`#creator-editor-topic-container .item`）。
pub const SEL_TOPIC_ITEM: &str = "#creator-editor-topic-container .item";
/// 定时发布开关（sau `set_schedule_time_xiaohongshu`：`.custom-switch-card` 内 `.d-switch`）。
pub const SEL_SCHEDULE_SWITCH: &str = ".custom-switch-card .d-switch";

// =========================================================================
// 平台约束（sau `fill_title` 截断 20；`fill_tags` 上限 10）
// =========================================================================

/// 小红书标题字数上限（sau: `self.title[:20]`）。
pub const XHS_TITLE_MAX: usize = 20;
/// 小红书话题标签上限（sau: 超过 10 会触发联想死循环）。
pub const XHS_TAG_MAX: usize = 10;

/// 视频上传轮询上限（毫秒）。
const VIDEO_UPLOAD_TIMEOUT_MS: u64 = 120_000;
/// 视频上传轮询步长（毫秒）。
const VIDEO_UPLOAD_POLL_MS: u64 = 3_000;

/// 小红书上传器：持有 [`BrowserDriver`] trait 对象，由 CDP 后端驱动真实发布。
///
/// 账号登录态（cookie）由调用方通过 [`Account`] + 外部 `CookieStore` 注入到 CDP 后端，
/// 本结构只负责发布流程编排与安全闸判定。
pub struct XiaohongshuUploader {
    account: Account,
    driver: Box<dyn BrowserDriver>,
    thumbnail: Option<String>,
}

impl XiaohongshuUploader {
    /// 创建上传器。`driver` 通常为 `Box<dyn BrowserDriver>`（CDP 后端）；单测可传 [`crate::browser::StubDriver`]。
    pub fn new(account: Account, driver: Box<dyn BrowserDriver>) -> Self {
        Self {
            account,
            driver,
            thumbnail: None,
        }
    }

    pub fn with_thumbnail(mut self, thumbnail: Option<String>) -> Self {
        self.thumbnail = thumbnail.filter(|value| !value.trim().is_empty());
        self
    }

    /// 当前账号引用（供上层在 `UploadRequest` 与 cookie 之间对齐 profile）。
    pub fn account(&self) -> &Account {
        &self.account
    }

    /// 图文发布页完整 URL。
    pub fn note_publish_url() -> String {
        format!("{XHS_CREATOR_BASE_URL}{XHS_NOTE_PUBLISH_PATH}")
    }

    /// 视频发布页完整 URL。
    pub fn video_publish_url() -> String {
        format!("{XHS_CREATOR_BASE_URL}{XHS_VIDEO_PUBLISH_PATH}")
    }

    /// 创作者中心首页 URL（用于登录态探测）。
    pub fn creator_home_url() -> String {
        format!("{XHS_CREATOR_BASE_URL}/")
    }

    /// 登录态检测：导航到创作者中心首页，若 URL 落到 `/login` 或页面出现「扫一扫」
    /// 二维码文本，则视为未登录并返回错误。
    ///
    /// 对应 sau `_is_xhs_login_completed`：`url.startswith(".../login")` 或
    /// `login-box` 可见即未登录。本 trait 无 `is_visible(selector)`，退化为文本探测。
    async fn ensure_logged_in(&self) -> anyhow::Result<()> {
        self.driver.goto(&Self::creator_home_url()).await?;
        self.driver.sleep_ms(1_500).await?;

        let url = self.driver.current_url().await?;
        if url.contains(XHS_LOGIN_PATH) {
            return Err(anyhow::anyhow!(
                "小红书未登录：当前停留在登录页 ({url})，请先 sau xiaohongshu login"
            ));
        }
        // 登录二维码区域文本出现 → 未登录（对应 sau 出现 login-box 二维码）。
        if self.driver.is_text_present(XHS_LOGIN_TEXT_SCAN).await? {
            return Err(anyhow::anyhow!(
                "小红书未登录：检测到登录二维码文本『{}』",
                XHS_LOGIN_TEXT_SCAN
            ));
        }
        Ok(())
    }

    /// 填写标题 / 正文 / 标签（对应 sau `fill_meta`）。
    ///
    /// - 标题按 [`XHS_TITLE_MAX`] 截断。
    /// - 标签按 [`XHS_TAG_MAX`] 截断，并以 `#话题` 形式拼入正文（[`BrowserDriver`] 无
    ///   "逐键输入 + wait_for_selector"，话题芯片候选 `#creator-editor-topic-container`
    ///   的点选增强未在此最小 driver 上复制，留待 CDP 后端，见 `coverage_notes`）。
    async fn fill_meta(&self, req: &UploadRequest) -> anyhow::Result<()> {
        // 标题（sau fill_title：self.title[:20]）。
        let title: String = req.title.chars().take(XHS_TITLE_MAX).collect();
        self.driver.fill(SEL_TITLE, &title).await?;

        // 正文 + 话题芯片（标签超 10 个先截断）。
        if !req.desc.is_empty() {
            self.driver.fill(SEL_DESC, &req.desc).await?;
        }
        for tag in req.tags.iter().take(XHS_TAG_MAX) {
            self.driver.type_text(SEL_DESC, &format!(" #{tag}")).await?;
            if self
                .driver
                .wait_for_selector(SEL_TOPIC_ITEM, 4_000)
                .await
                .is_ok()
            {
                self.driver.click_selector(SEL_TOPIC_ITEM).await?;
            } else {
                self.driver.press_key("Space").await?;
            }
        }
        Ok(())
    }

    /// 轮询视频上传完成：等待页面出现「上传成功」文本。
    ///
    /// 对应 sau `upload_video_content` 的 while 循环：预览区出现
    /// `上传成功/分辨率/重新上传/编辑封面/已上传/100%` 即跳出。本实现取最稳定的「上传成功」。
    async fn wait_video_uploaded(&self) -> anyhow::Result<()> {
        let mut waited: u64 = 0;
        while waited < VIDEO_UPLOAD_TIMEOUT_MS {
            if self
                .driver
                .is_text_present(XHS_VIDEO_UPLOAD_DONE_TEXT)
                .await?
            {
                return Ok(());
            }
            self.driver.sleep_ms(VIDEO_UPLOAD_POLL_MS).await?;
            waited += VIDEO_UPLOAD_POLL_MS;
        }
        Err(anyhow::anyhow!(
            "视频上传超时（{VIDEO_UPLOAD_TIMEOUT_MS}ms 未出现『{}』）",
            XHS_VIDEO_UPLOAD_DONE_TEXT
        ))
    }

    /// 点击「发布」按钮并等待跳转到成功页。
    ///
    /// 对应 sau：`button:has-text("发布")` 点击 +
    /// `wait_for_url("**/publish/success?**")`。本 driver 用 `click_text("发布")` +
    /// `wait_for_url_contains("publish/success")`。
    async fn publish_now(&self, scheduled: bool) -> anyhow::Result<()> {
        self.driver
            .click_text(if scheduled { "定时发布" } else { "发布" })
            .await?;
        self.driver
            .wait_for_url_contains(XHS_PUBLISH_SUCCESS_FRAGMENT)
            .await?;
        Ok(())
    }

    async fn schedule_if_needed(&self, req: &UploadRequest) -> anyhow::Result<bool> {
        let Some(schedule) = req.schedule.as_deref() else {
            return Ok(false);
        };
        self.driver.click_selector(SEL_SCHEDULE_SWITCH).await?;
        self.driver.sleep_ms(500).await?;
        self.driver
            .fill(".d-datepicker-input-filter input.d-text", schedule)
            .await?;
        Ok(true)
    }

    async fn set_thumbnail_if_needed(&self) -> anyhow::Result<()> {
        let Some(thumbnail) = self.thumbnail.as_ref() else {
            return Ok(());
        };
        self.driver.click_text("设置封面").await?;
        self.driver.sleep_ms(500).await?;
        let selector = "div.d-modal.cover-modal input[type='file'][accept*='image']";
        self.driver.wait_for_selector(selector, 10_000).await?;
        self.driver
            .set_input_files(selector, std::slice::from_ref(thumbnail))
            .await?;
        self.driver.sleep_ms(1_000).await?;
        self.driver.click_text("确定").await?;
        Ok(())
    }

    async fn check_original_declaration(&self) {
        let _ = self.driver.click_text("原创声明").await;
    }

    /// 真实执行图文发布流程（已过安全闸）。
    async fn run_note(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        if req.media_paths.iter().all(|p| !is_image(p)) {
            return Err(anyhow::anyhow!(
                "图文模式缺少图片文件，media_paths={:?}",
                req.media_paths
            ));
        }

        self.ensure_logged_in().await?;

        // 赶往图文发布页（sau upload_note_content）。
        self.driver.goto(&Self::note_publish_url()).await?;
        self.driver.wait_for_url_contains("target=image").await?;

        // 上传图片（sau：优先 image file input，兜底 upload-input）。
        if self
            .driver
            .set_input_files(SEL_NOTE_IMAGE_INPUT, &req.media_paths)
            .await
            .is_err()
        {
            // 兜底选择器：有些前端版本走 upload-content 通用入口。
            self.driver
                .set_input_files(SEL_UPLOAD_INPUT, &req.media_paths)
                .await?;
        }
        // 等待素材上传稳定、标题框就绪（sau：wait_for visible title，本 driver 退化为固定等待）。
        self.driver.sleep_ms(2_000).await?;

        self.fill_meta(req).await?;
        self.check_original_declaration().await;
        let scheduled = self.schedule_if_needed(req).await?;
        self.publish_now(scheduled).await?;

        let url = self.driver.current_url().await.unwrap_or_default();
        Ok(UploadResult {
            platform_post_id: None,
            url: Some(url),
            dry_run: false,
            note: format!("已发布小红书图文笔记（account={}）", self.account.profile),
        })
    }

    /// 真实执行视频发布流程（已过安全闸）。
    async fn run_video(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        let videos: Vec<String> = req
            .media_paths
            .iter()
            .filter(|p| is_video(p))
            .cloned()
            .collect();
        if videos.is_empty() {
            return Err(anyhow::anyhow!(
                "视频模式缺少视频文件，media_paths={:?}",
                req.media_paths
            ));
        }

        self.ensure_logged_in().await?;

        // 赶往视频发布页（sau upload_video_content）。
        self.driver.goto(&Self::video_publish_url()).await?;
        self.driver.wait_for_url_contains("target=video").await?;

        // 上传视频（sau：upload-content 内 upload-input）。
        self.driver
            .set_input_files(SEL_UPLOAD_INPUT, &videos)
            .await?;
        self.wait_video_uploaded().await?;

        self.fill_meta(req).await?;
        self.set_thumbnail_if_needed().await?;
        self.check_original_declaration().await;
        let scheduled = self.schedule_if_needed(req).await?;
        self.publish_now(scheduled).await?;

        let url = self.driver.current_url().await.unwrap_or_default();
        Ok(UploadResult {
            platform_post_id: None,
            url: Some(url),
            dry_run: false,
            note: format!("已发布小红书视频（account={}）", self.account.profile),
        })
    }
}

/// 安全闸默认结果：dry_run 或未 confirm 时返回的「计划态」。
///
/// 满足约束：`dry_run=true` 直接返回 `UploadResult{dry_run:true, note:"planned: ..."}`
/// 且**不调用 driver**；`confirm=true` 且 `dry_run=false` 才真执行。
fn planned_result(req: &UploadRequest, kind: &str) -> UploadResult {
    let tags = req.tags.join(",");
    let note = format!(
        "planned: xiaohongshu {kind} | account={} | title=\"{}\" | tags=[{}] | media={}",
        req.account_profile,
        req.title,
        tags,
        req.media_paths.join(",")
    );
    UploadResult {
        platform_post_id: None,
        url: None,
        dry_run: true,
        note,
    }
}

#[async_trait]
impl Uploader for XiaohongshuUploader {
    fn platform(&self) -> &'static str {
        "xiaohongshu"
    }

    async fn upload_note(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        // 安全闸：dry_run 或未 confirm → 只规划，不触碰 driver。
        if req.dry_run || !req.confirm {
            return Ok(planned_result(req, "图文"));
        }
        self.run_note(req).await
    }

    async fn upload_video(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        // 安全闸：dry_run 或未 confirm → 只规划，不触碰 driver。
        if req.dry_run || !req.confirm {
            return Ok(planned_result(req, "视频"));
        }
        self.run_video(req).await
    }
}

/// 判断文件是否为图片扩展名（图文模式 media 校验）。
fn is_image(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".jpg") || p.ends_with(".jpeg") || p.ends_with(".png") || p.ends_with(".webp")
}

/// 判断文件是否为视频扩展名（视频模式 media 校验）。
fn is_video(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".mp4") || p.ends_with(".mov") || p.ends_with(".m4v")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::Account;
    use crate::browser::StubDriver;
    use crate::uploader::{UploadRequest, Uploader};

    /// 用 [`StubDriver`] 构造上传器。StubDriver 除 `sleep_ms` 外全部报错，
    /// 因此任何「真执行」分支都会在测试中失败暴露；dry_run/confirm=false 分支不应触碰 driver。
    fn uploader() -> XiaohongshuUploader {
        XiaohongshuUploader::new(
            Account::new("xiaohongshu", "staff_xhs_01"),
            Box::new(StubDriver),
        )
    }

    fn image_req(dry_run: bool, confirm: bool) -> UploadRequest {
        UploadRequest {
            platform: "xiaohongshu".into(),
            account_profile: "staff_xhs_01".into(),
            title: "青岛海边一日游".into(),
            desc: "分享今天的行程".into(),
            tags: vec!["旅行".into(), "青岛".into()],
            media_paths: vec!["/tmp/a.jpg".into(), "/tmp/b.png".into()],
            schedule: None,
            dry_run,
            confirm,
        }
    }

    #[tokio::test]
    async fn dry_run_note_returns_planned_without_touching_driver() {
        let u = uploader();
        // dry_run=true → 必须不调用 StubDriver（否则 goto 会报错导致 unwrap 失败）。
        let res = u.upload_note(&image_req(true, false)).await.unwrap();
        assert!(res.dry_run, "dry_run 结果必须为 true");
        assert!(res.note.starts_with("planned:"), "note 应以 planned: 开头");
        assert!(res.note.contains("图文"), "图文分支应在 note 中标注类型");
        assert!(res.note.contains("青岛海边一日游"), "note 应含标题");
        assert!(res.note.contains("旅行"), "note 应含标签");
        assert!(res.platform_post_id.is_none() && res.url.is_none());
    }

    #[tokio::test]
    async fn dry_run_vs_confirm_gate_branches_note_and_video() {
        let u = uploader();

        // (1) dry_run=true, confirm=true → 仍走 planned（dry_run 优先级高于 confirm）。
        let res = u.upload_note(&image_req(true, true)).await.unwrap();
        assert!(res.dry_run);
        assert!(res.note.starts_with("planned:"));

        // (2) dry_run=false, confirm=false → 安全默认：planned，不触碰 driver。
        let res = u.upload_note(&image_req(false, false)).await.unwrap();
        assert!(res.dry_run, "未 confirm 应退回 planned");
        assert!(res.note.starts_with("planned:"));

        // (3) dry_run=false, confirm=true → 进入真执行分支；图文媒体正常，
        // 但 StubDriver.goto 会报错 → 这里断言「确实尝试驱动」（未在闸前返回）。
        let err = u.upload_note(&image_req(false, true)).await;
        assert!(
            err.is_err(),
            "confirm=true 应进入真执行分支并触碰 StubDriver"
        );

        // (4) 视频分支同理：confirm=false → planned 且标注「视频」。
        let mut video_req = image_req(false, false);
        video_req.media_paths = vec!["/tmp/v.mp4".into()];
        let res = u.upload_video(&video_req).await.unwrap();
        assert!(res.dry_run);
        assert!(res.note.contains("视频"), "视频分支应在 note 中标注类型");
    }

    #[test]
    fn url_constants_match_sau() {
        // 断言从 sau main.py 提取的 URL/路径/片段。
        assert_eq!(XHS_CREATOR_BASE_URL, "https://creator.xiaohongshu.com");
        assert_eq!(XHS_LOGIN_PATH, "/login");
        assert_eq!(
            XHS_NOTE_PUBLISH_PATH,
            "/publish/publish?from=homepage&target=image"
        );
        assert_eq!(
            XHS_VIDEO_PUBLISH_PATH,
            "/publish/publish?from=homepage&target=video"
        );
        assert_eq!(XHS_PUBLISH_SUCCESS_FRAGMENT, "publish/success");

        // 组合 URL。
        assert_eq!(
            XiaohongshuUploader::note_publish_url(),
            "https://creator.xiaohongshu.com/publish/publish?from=homepage&target=image"
        );
        assert_eq!(
            XiaohongshuUploader::video_publish_url(),
            "https://creator.xiaohongshu.com/publish/publish?from=homepage&target=video"
        );
    }

    #[test]
    fn login_text_and_selectors_extracted_from_sau() {
        // 登录检测文本（sau _find_xhs_qrcode_locator / _save_xhs_qrcode）。
        assert_eq!(XHS_LOGIN_TEXT_SCAN, "扫一扫");
        assert_eq!(XHS_LOGIN_TEXT_SCAN_LOGIN, "APP扫一扫登录");
        assert_eq!(XHS_VIDEO_UPLOAD_DONE_TEXT, "上传成功");

        // 选择器关键子串（sau main.py）。
        assert!(SEL_LOGIN_BOX.contains("login-box"));
        assert!(SEL_TITLE.contains("填写标题"));
        assert!(SEL_DESC.contains("输入正文描述"));
        assert!(SEL_NOTE_IMAGE_INPUT.contains("image"));
        assert!(SEL_UPLOAD_INPUT.contains("upload-input"));
        assert!(SEL_VIDEO_RETRY_INPUT.contains("upload-btn-input"));
        assert!(SEL_TOPIC_CONTAINER.contains("creator-editor-topic-container"));
        assert_eq!(SEL_TOPIC_ITEM, "#creator-editor-topic-container .item");

        // 平台约束。
        assert_eq!(XHS_TITLE_MAX, 20);
        assert_eq!(XHS_TAG_MAX, 10);
    }

    #[test]
    fn title_truncation_respects_xhs_limit() {
        // 模拟 fill_meta 的标题截断逻辑。
        let long_title = "一二三四五六七八九十一二三四五六七八九十一二三四五"; // 25 字
        let truncated: String = long_title.chars().take(XHS_TITLE_MAX).collect();
        assert_eq!(truncated.chars().count(), XHS_TITLE_MAX);
    }

    #[test]
    fn platform_name_matches_account() {
        let u = uploader();
        assert_eq!(u.platform(), "xiaohongshu");
        assert_eq!(u.account().platform, "xiaohongshu");
    }

    #[test]
    fn media_kind_helpers() {
        assert!(is_image("/tmp/A.JPG"));
        assert!(is_image("x.png"));
        assert!(!is_image("x.mp4"));
        assert!(is_video("clip.MP4"));
        assert!(is_video("a.mov"));
        assert!(!is_video("a.jpg"));
    }
}
