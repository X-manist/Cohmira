//! # 抖音（creator.douyin.com）视频/图文上传器
//!
//! 把原 social-auto-upload (sau) Python 的 `uploader/douyin_uploader/main.py`
//! RPA 流程移植为 Rust，操作目标为 [`crate::browser::BrowserDriver`] 抽象（对齐
//! Playwright `page.*` 调用）。真实浏览器后端由 `chromiumoxide` (feature=cdp) 实现，
//! 单测用 [`crate::browser::StubDriver`] / 测试 mock 驱动。
//!
//! ## 流程（对齐 sau `DouYinVideo.upload` / `DouYinNote.upload`）
//!
//! 1. [`DouyinUploader::ensure_login`]：goto 上传页 → 等页面稳定 → 检测登录文案
//!    （`手机号登录` / `扫码登录`）。命中则走扫码兜底（goto 首页 → 等 URL 跳到
//!    `creator-micro`，由真实 driver/用户扫码完成）。
//! 2. goto `creator.douyin.com/creator-micro/content/upload`。
//! 3. 视频：`set_input_files` 到 `div[class^='container'] input`；图文：先点
//!    `发布图文`，再 `set_input_files` 到 image input。
//! 4. 等发布页（v1 `content/publish` 或 v2 `content/post/video`；图文
//!    `content/post/image`）。
//! 5. 填标题（截 30 字符）→ 填描述+话题（`#tag` 拼接到 contenteditable 编辑器）。
//! 6. 视频等转码完成（页面出现 `重新上传`）。
//! 7. 可选：设置封面 / 自主声明 / 定时发布（best-effort，失败不中断）。
//! 8. 点 `发布` → 等 `content/manage` 跳转。
//!
//! ## 安全闸
//!
//! [`UploadRequest::dry_run`] 默认 true；仅当 `dry_run=false && confirm=true` 才真
//! 执行 driver 调用。dry_run 时直接返回 `planned: ...` 结果，不触碰 driver。
//!
//! ## 注意
//!
//! RPA 选择器/文案会随抖音前端版本变化（sau 注释里多次提到 version_1/version_2
//! DOM 差异），本模块把所有 URL/选择器/登录检测文本提取为 `pub const`，便于在
//! 选择器失效时单点修改；但真实可用性需用真实账号在目标平台验证（见
//! [`coverage_notes`](../index.html)）。

use async_trait::async_trait;
use std::path::Path;
use tracing::{debug, info, warn};

use crate::account::Account;
use crate::browser::BrowserDriver;
use crate::uploader::{UploadRequest, UploadResult, Uploader};

// =====================================================================================
// 常量：URL / 登录检测文本 / 选择器 / 文案（提取自 sau main.py，便于流程单测断言）
// =====================================================================================

/// 抖音创作者中心首页（扫码登录兜底入口）。sau: `https://creator.douyin.com/`。
pub const DOUYIN_HOME_URL: &str = "https://creator.douyin.com/";
/// 视频/图文上传页。sau: `creator-micro/content/upload`。
pub const DOUYIN_UPLOAD_URL: &str = "https://creator.douyin.com/creator-micro/content/upload";
/// 登录态判定片段：登录后任意 creator-micro 子页 URL 都含此串。
pub const DOUYIN_LOGGED_IN_FRAG: &str = "creator.douyin.com/creator-micro";
/// v1 视频发布页 URL 片段（sau `content/publish?enter_from=publish_page`）。
pub const DOUYIN_PUBLISH_V1_FRAG: &str = "content/publish";
/// v2 视频发布页 URL 片段（sau `content/post/video?enter_from=publish_page`）。
pub const DOUYIN_PUBLISH_V2_FRAG: &str = "content/post/video";
/// 图文发布页 URL 片段（sau `content/post/image`）。
pub const DOUYIN_NOTE_PUBLISH_FRAG: &str = "content/post/image";
/// 发布成功后跳转的内容管理页片段（sau `content/manage**`）。
pub const DOUYIN_MANAGE_FRAG: &str = "content/manage";

/// 登录检测：手机号登录文案（sau `cookie_auth` 检测项）。
pub const DOUYIN_LOGIN_TEXT_PHONE: &str = "手机号登录";
/// 登录检测：扫码登录文案（sau `cookie_auth` / `_is_douyin_login_completed` 检测项）。
pub const DOUYIN_LOGIN_TEXT_SCAN: &str = "扫码登录";
/// 登录检测：二维码失效文案（sau `_wait_for_douyin_login` 刷新触发条件）。
pub const DOUYIN_LOGIN_TEXT_QR_EXPIRED: &str = "二维码失效";

/// 视频文件 input 选择器（sau `div[class^='container'] input`）。
pub const DOUYIN_VIDEO_INPUT_SEL: &str = "div[class^='container'] input";
/// 图文图片 input 选择器（sau `div[class^='container'] input[accept*='image']`）。
pub const DOUYIN_NOTE_IMAGE_INPUT_SEL: &str = "div[class^='container'] input[accept*='image']";
/// 标题 input 选择器（sau `input[placeholder*="填写作品标题"]`）。
pub const DOUYIN_TITLE_INPUT_SEL: &str = "input[placeholder*='填写作品标题']";
/// 描述 contenteditable 编辑器选择器（sau `div.zone-container[contenteditable="true"]`）。
pub const DOUYIN_DESC_EDITOR_SEL: &str = "div.zone-container[contenteditable='true']";
/// 封面上传 input 选择器（sau 取 `input.semi-upload-hidden-input` 的 nth(1)）。
///
/// 注意：[`BrowserDriver`] 无法指定 nth 索引；真实 CDP 后端应在弹窗
/// `div.dy-creator-content-modal` 内取第 2 个 hidden input（sau 注释：[0]/[1] 是
/// AI 参考图，[2]/[3] 才是封面）。此处 const 仅用于流程编排与断言。
pub const DOUYIN_COVER_INPUT_SEL: &str =
    "div.dy-creator-content-modal input.semi-upload-hidden-input";
/// 定时发布时间 input 选择器（sau `.semi-input[placeholder="日期和时间"]`）。
pub const DOUYIN_SCHEDULE_TIME_INPUT_SEL: &str = ".semi-input[placeholder='日期和时间']";

/// 转码完成标记文案（sau `[class^="long-card"] div:has-text("重新上传")`）。
pub const DOUYIN_REUPLOAD_TEXT: &str = "重新上传";
/// 发布按钮文案（sau `get_by_role("button", name="发布", exact=True)`）。
pub const DOUYIN_PUBLISH_BTN_TEXT: &str = "发布";
/// 图文发布 Tab 文案（sau `get_by_text("发布图文", exact=True)`）。
pub const DOUYIN_NOTE_TAB_TEXT: &str = "发布图文";
/// 封面选择入口文案（sau `get_by_text("选择封面", exact=True)`）。
pub const DOUYIN_SELECT_COVER_TEXT: &str = "选择封面";
/// 封面应用按钮文案（sau `get_by_role("button", name="完成", exact=True)`）。
pub const DOUYIN_COVER_DONE_TEXT: &str = "完成";
/// 自主声明入口占位文案（sau `get_by_text("请选择自主声明")`）。
pub const DOUYIN_SELF_DECL_ENTRY_TEXT: &str = "请选择自主声明";
/// 自主声明默认选项（sau `set_self_declaration` 默认值）。
pub const DOUYIN_SELF_DECL_OPTION_TEXT: &str = "内容为个人观点或见解";
/// 自主声明弹窗确认按钮文案。
pub const DOUYIN_CONFIRM_BTN_TEXT: &str = "确定";
/// 定时发布单选文案（sau `[class^='radio']:has-text('定时发布')`）。
pub const DOUYIN_SCHEDULE_RADIO_TEXT: &str = "定时发布";

/// 标题最大字符数（sau `fill` 时 `title[:30]`）。
pub const DOUYIN_TITLE_MAX_CHARS: usize = 30;
/// 等视频转码完成的最大轮次（每轮 2s，约 5min；对齐 sau `while True` + 失败重试语义）。
const TRANSCODE_POLL_MAX: u32 = 150;
/// 转码轮询间隔毫秒（sau `asyncio.sleep(2)`）。
const TRANSCODE_POLL_MS: u64 = 2_000;
/// 页面稳定等待毫秒（sau `wait_for_timeout(2500)` / `1000`）。
const STABILIZE_MS: u64 = 2_500;
const SHORT_STABILIZE_MS: u64 = 1_000;

// =====================================================================================
// 流程步骤序列（dry_run 计划文本 + 单测断言用）
// =====================================================================================

/// 构造 dry-run 计划步骤序列（视频/图文），用于 [`UploadResult::note`] 与流程单测断言。
///
/// 步骤名是稳定的 `&'static str`，单测可对序列首尾、包含关系、相对顺序做断言。
pub fn planned_steps(req: &UploadRequest, is_video: bool) -> Vec<&'static str> {
    let mut steps: Vec<&'static str> = vec!["check_login", "goto_upload_page"];
    if is_video {
        steps.push("set_video_files");
        steps.push("wait_publish_page");
        steps.push("fill_title");
        steps.push("fill_description_and_tags");
        steps.push("wait_transcode");
        // media_paths[0]=视频，[1]=封面（sau DouYinVideo.thumbnail_landscape_path）。
        if req.media_paths.len() > 1 {
            steps.push("set_cover");
        }
        steps.push("set_self_declaration");
        if req.schedule.is_some() {
            steps.push("set_schedule_time");
        }
        steps.push("click_publish");
        steps.push("wait_manage_page");
    } else {
        steps.push("click_note_tab");
        steps.push("set_image_files");
        steps.push("wait_note_publish_page");
        steps.push("fill_title");
        steps.push("fill_description_and_tags");
        if req.schedule.is_some() {
            steps.push("set_schedule_time");
        }
        steps.push("click_publish");
        steps.push("wait_manage_page");
    }
    steps
}

/// 拼接描述与话题：`desc #tag1 #tag2`（对齐 sau 向 contenteditable 编辑器键入 `#tag` 的语义，
/// 并补上 sau 未实际写入的 desc 文本——这是对 sau `fill_title_and_description` 的修正）。
#[cfg(test)]
fn build_desc_with_tags(desc: &str, tags: &[String]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let desc = desc.trim();
    if !desc.is_empty() {
        parts.push(desc.to_string());
    }
    for t in tags {
        let t = t.trim();
        if !t.is_empty() {
            parts.push(format!("#{t}"));
        }
    }
    parts.join(" ")
}

/// 按字符截断标题（中文友好；sau `title[:30]`）。
fn truncate_title(title: &str) -> String {
    title.chars().take(DOUYIN_TITLE_MAX_CHARS).collect()
}

/// 校验账号 cookie 文件是否存在（对齐 sau `cookie_auth` 前置检查；复用 [`Account`]）。
pub fn check_cookie_file(account: &Account, base_dir: &Path) -> bool {
    account.cookie_path(base_dir).exists()
}

// =====================================================================================
// DouyinUploader
// =====================================================================================

/// 抖音上传器。持有 [`BrowserDriver`] trait 对象，所有 RPA 操作经 driver 完成。
///
/// 构造：`DouyinUploader::new(Box::new(StubDriver))`（占位）或
/// `Box::new(cdp_driver)`（真实发布）。实现 [`Uploader`] trait。
pub struct DouyinUploader {
    driver: Box<dyn BrowserDriver>,
    thumbnail_landscape: Option<String>,
    thumbnail_portrait: Option<String>,
    product_link: Option<String>,
    product_title: Option<String>,
    bgm: Option<String>,
}

impl DouyinUploader {
    /// 构造上传器。`driver` 通常是 `Box<dyn BrowserDriver>`。
    pub fn new(driver: Box<dyn BrowserDriver>) -> Self {
        Self {
            driver,
            thumbnail_landscape: None,
            thumbnail_portrait: None,
            product_link: None,
            product_title: None,
            bgm: None,
        }
    }

    pub fn with_video_options(
        mut self,
        thumbnail_landscape: Option<String>,
        thumbnail_portrait: Option<String>,
        product_link: Option<String>,
        product_title: Option<String>,
    ) -> Self {
        self.thumbnail_landscape = thumbnail_landscape;
        self.thumbnail_portrait = thumbnail_portrait;
        self.product_link = product_link;
        self.product_title = product_title;
        self
    }

    pub fn with_note_bgm(mut self, bgm: Option<String>) -> Self {
        self.bgm = bgm.filter(|value| !value.trim().is_empty());
        self
    }

    /// dry-run 计划结果（不调 driver）。note 形如 `planned: douyin video upload -> <URL> | steps: ...`。
    fn planned_result(&self, req: &UploadRequest, is_video: bool) -> UploadResult {
        let kind = if is_video { "video" } else { "note" };
        let steps = planned_steps(req, is_video);
        UploadResult {
            platform_post_id: None,
            url: None,
            dry_run: true,
            note: format!(
                "planned: douyin {kind} upload -> {DOUYIN_UPLOAD_URL} | steps: {}",
                steps.join(" -> ")
            ),
        }
    }

    /// 分发：安全闸 → 登录 → 视频/图文流程。
    async fn upload_impl(
        &self,
        req: &UploadRequest,
        is_video: bool,
    ) -> anyhow::Result<UploadResult> {
        // 安全闸：默认 dry_run；仅 confirm=true 且 dry_run=false 才真执行。
        if req.dry_run || !req.confirm {
            debug!(
                "douyin dry_run(req.dry_run={}, confirm={}) -> 返回计划，不调 driver",
                req.dry_run, req.confirm
            );
            return Ok(self.planned_result(req, is_video));
        }

        if req.media_paths.is_empty() {
            anyhow::bail!("media_paths 为空：无法发布（视频需 1 个视频文件，图文需 >=1 张图片）");
        }

        // 1. 登录检测 + 扫码兜底
        self.ensure_login().await?;

        // 2. goto 上传页（ensure_login 已导航过，此处幂等再导航一次保证落在 upload 页）
        info!(url = DOUYIN_UPLOAD_URL, "goto 抖音上传页");
        self.driver.goto(DOUYIN_UPLOAD_URL).await?;
        self.driver.wait_for_url_contains("content/upload").await?;

        // 3. 分支
        if is_video {
            self.upload_video_flow(req).await?;
        } else {
            self.upload_note_flow(req).await?;
        }

        Ok(UploadResult {
            platform_post_id: None,
            url: Some(DOUYIN_UPLOAD_URL.to_string()),
            dry_run: false,
            note: format!(
                "douyin {} published: {}",
                if is_video { "video" } else { "note" },
                req.title
            ),
        })
    }

    // ---------------------------------------------------------------------------------
    // 登录
    // ---------------------------------------------------------------------------------

    /// 登录检测 + 扫码兜底。对齐 sau `cookie_auth` + `douyin_cookie_gen`。
    async fn ensure_login(&self) -> anyhow::Result<()> {
        self.driver.goto(DOUYIN_UPLOAD_URL).await?;
        self.driver.sleep_ms(STABILIZE_MS).await?; // 等页面稳定，避免瞬时跳转误判
        if self.is_logged_in().await? {
            info!("抖音已登录（URL 含 content/upload 且无登录文案）");
            return Ok(());
        }
        // 扫码兜底：goto 首页，阻塞等 URL 跳到 creator-micro（真实 driver/用户完成扫码）。
        // 注意：本 trait 无截取二维码图片能力，二维码展示由真实 CDP 后端的 headed 浏览器承担。
        warn!("抖音 cookie 失效，进入扫码登录兜底（请在新打开的浏览器中扫码）");
        self.driver.goto(DOUYIN_HOME_URL).await?;
        self.driver
            .wait_for_url_contains(DOUYIN_LOGGED_IN_FRAG)
            .await?;
        info!("扫码登录完成，已跳转登录后页面");
        Ok(())
    }

    /// 当前是否已登录：URL 含 `content/upload` 且页面无登录文案。
    /// 对齐 sau `cookie_auth`：`"content/upload" in page.url and not has_login`。
    async fn is_logged_in(&self) -> anyhow::Result<bool> {
        let url = self.driver.current_url().await?;
        if !url.contains("content/upload") {
            return Ok(false);
        }
        let on_login_page = self.driver.is_text_present(DOUYIN_LOGIN_TEXT_PHONE).await?
            || self.driver.is_text_present(DOUYIN_LOGIN_TEXT_SCAN).await?;
        Ok(!on_login_page)
    }

    // ---------------------------------------------------------------------------------
    // 视频流程
    // ---------------------------------------------------------------------------------

    async fn upload_video_flow(&self, req: &UploadRequest) -> anyhow::Result<()> {
        let video_path = req
            .media_paths
            .first()
            .ok_or_else(|| anyhow::anyhow!("video media_paths 为空"))?;

        // set_input_files 视频文件
        info!(file = video_path, "上传视频文件");
        self.driver
            .set_input_files(DOUYIN_VIDEO_INPUT_SEL, std::slice::from_ref(video_path))
            .await?;

        // 等发布页（v1 或 v2）
        self.wait_publish_page_v1_or_v2().await?;
        self.driver.sleep_ms(SHORT_STABILIZE_MS).await?;

        // 标题（截 30 字符）+ 描述/话题
        let title = truncate_title(&req.title);
        debug!(title = %title, "填写标题");
        self.driver.fill(DOUYIN_TITLE_INPUT_SEL, &title).await?;

        debug!(desc = %req.desc, "填写描述+话题");
        self.driver.fill(DOUYIN_DESC_EDITOR_SEL, &req.desc).await?;
        for tag in &req.tags {
            self.driver
                .type_text(DOUYIN_DESC_EDITOR_SEL, &format!(" #{tag}"))
                .await?;
            self.driver.press_key("Space").await?;
        }
        let _ = self.driver.press_key("Escape").await;

        // 等转码完成（页面出现"重新上传"）
        self.wait_transcode_done().await?;

        // 封面（best-effort）
        let cover = self
            .thumbnail_portrait
            .as_deref()
            .or(self.thumbnail_landscape.as_deref())
            .or_else(|| req.media_paths.get(1).map(String::as_str));
        if let Some(cover) = cover {
            let portrait = self.thumbnail_portrait.as_deref() == Some(cover);
            let _ = self.set_cover(cover, portrait).await;
        }

        if let (Some(link), Some(title)) =
            (self.product_link.as_deref(), self.product_title.as_deref())
        {
            let _ = self.set_product_link(link, title).await;
        }

        // 自主声明（best-effort：失败仅 warn，不中断发布）
        let _ = self.set_self_declaration().await;

        // 定时发布
        if let Some(sched) = req.schedule.as_deref() {
            let _ = self.set_schedule_time(sched).await;
        }

        // 发布
        self.click_publish_and_wait().await?;
        Ok(())
    }

    /// 等视频发布页：先试 v1 (`content/publish`)，失败再试 v2 (`content/post/video`)。
    /// 对齐 sau `while True` 内对两个 URL 的 `wait_for_url(..., timeout=3000)` 尝试。
    async fn wait_publish_page_v1_or_v2(&self) -> anyhow::Result<()> {
        for _ in 0..240 {
            let url = self.driver.current_url().await?;
            if url.contains(DOUYIN_PUBLISH_V1_FRAG) {
                debug!("进入 v1 视频发布页");
                return Ok(());
            }
            if url.contains(DOUYIN_PUBLISH_V2_FRAG) {
                debug!("进入 v2 视频发布页");
                return Ok(());
            }
            self.driver.sleep_ms(500).await?;
        }
        anyhow::bail!("等待抖音视频发布页超时")
    }

    /// 轮询等转码完成：页面出现"重新上传"文案。
    /// 对齐 sau `[class^="long-card"] div:has-text("重新上传")` 计数 > 0。
    async fn wait_transcode_done(&self) -> anyhow::Result<()> {
        for _ in 0..TRANSCODE_POLL_MAX {
            if self.driver.is_text_present(DOUYIN_REUPLOAD_TEXT).await? {
                info!("视频转码完成（检测到\"重新上传\"）");
                return Ok(());
            }
            self.driver.sleep_ms(TRANSCODE_POLL_MS).await?;
        }
        anyhow::bail!(
            "等待视频转码超时（{}s）",
            TRANSCODE_POLL_MAX as u64 * TRANSCODE_POLL_MS / 1_000
        )
    }

    // ---------------------------------------------------------------------------------
    // 图文流程
    // ---------------------------------------------------------------------------------

    async fn upload_note_flow(&self, req: &UploadRequest) -> anyhow::Result<()> {
        // 切换到图文 Tab
        info!("切换到图文发布");
        self.driver.click_text(DOUYIN_NOTE_TAB_TEXT).await?;
        self.driver.sleep_ms(SHORT_STABILIZE_MS).await?;

        // 上传图片
        info!(count = req.media_paths.len(), "上传图文图片");
        self.driver
            .set_input_files(DOUYIN_NOTE_IMAGE_INPUT_SEL, &req.media_paths)
            .await?;

        // 等图文发布页
        self.driver
            .wait_for_url_contains(DOUYIN_NOTE_PUBLISH_FRAG)
            .await?;
        self.driver.sleep_ms(SHORT_STABILIZE_MS).await?;

        // 标题 + 描述/话题
        let title = truncate_title(&req.title);
        debug!(title = %title, "填写标题");
        self.driver.fill(DOUYIN_TITLE_INPUT_SEL, &title).await?;

        debug!(desc = %req.desc, "填写描述+话题");
        self.driver.fill(DOUYIN_DESC_EDITOR_SEL, &req.desc).await?;
        for tag in &req.tags {
            self.driver
                .type_text(DOUYIN_DESC_EDITOR_SEL, &format!(" #{tag}"))
                .await?;
            self.driver.press_key("Space").await?;
        }
        let _ = self.driver.press_key("Escape").await;

        if let Some(bgm) = self.bgm.as_deref() {
            let _ = self.select_bgm(bgm).await;
        }

        // 定时发布
        if let Some(sched) = req.schedule.as_deref() {
            let _ = self.set_schedule_time(sched).await;
        }

        // 发布
        self.click_publish_and_wait().await?;
        Ok(())
    }

    // ---------------------------------------------------------------------------------
    // 公共子步骤
    // ---------------------------------------------------------------------------------

    /// 设置封面：点"选择封面" → 上传封面文件 → 点"完成"。
    /// 对齐 sau `set_thumbnail`（已省略 shepherd 浮层清理等 JS evaluate 步骤——
    /// [`BrowserDriver`] 无 `evaluate` 能力，由 CDP 后端在 click 前清理）。
    async fn set_cover(&self, cover_path: &str, portrait: bool) -> anyhow::Result<()> {
        info!(cover = cover_path, "设置视频封面");
        let _ = self
            .driver
            .evaluate("document.querySelectorAll('.shepherd-element,.shepherd-modal-overlay-container').forEach(e=>e.remove())")
            .await;
        self.driver.click_text(DOUYIN_SELECT_COVER_TEXT).await?;
        self.driver.sleep_ms(1_500).await?;
        let _ = self
            .driver
            .click_text(if portrait {
                "设置竖封面"
            } else {
                "设置横封面"
            })
            .await;
        self.driver
            .set_input_files_nth(DOUYIN_COVER_INPUT_SEL, 1, &[cover_path.to_string()])
            .await?;
        self.driver.sleep_ms(3_000).await?;
        self.driver.click_text(DOUYIN_COVER_DONE_TEXT).await?;
        debug!("封面设置完成");
        Ok(())
    }

    async fn set_product_link(&self, link: &str, title: &str) -> anyhow::Result<()> {
        self.driver.click_text("添加标签").await?;
        self.driver.click_text("购物车").await?;
        self.driver
            .wait_for_selector("input[placeholder='粘贴商品链接']", 10_000)
            .await?;
        self.driver
            .fill("input[placeholder='粘贴商品链接']", link)
            .await?;
        self.driver.click_text("添加链接").await?;
        self.driver
            .wait_for_selector("input[placeholder='请输入商品短标题']", 10_000)
            .await?;
        self.driver
            .fill(
                "input[placeholder='请输入商品短标题']",
                &title.chars().take(10).collect::<String>(),
            )
            .await?;
        self.driver.click_text("完成编辑").await?;
        Ok(())
    }

    async fn select_bgm(&self, bgm: &str) -> anyhow::Result<()> {
        self.driver.click_text("选择音乐").await?;
        self.driver
            .wait_for_selector("input.semi-input[placeholder='搜索音乐']", 10_000)
            .await?;
        self.driver
            .fill("input.semi-input[placeholder='搜索音乐']", bgm)
            .await?;
        self.driver.sleep_ms(2_000).await?;
        let applied = self
            .driver
            .evaluate("(()=>{const card=document.querySelector('.card-container-tmocjc');const btn=card&&card.querySelector('.apply-btn-LUPP0D');if(!btn)return false;btn.click();return true;})()")
            .await?
            .as_bool()
            .unwrap_or(false);
        if !applied {
            anyhow::bail!("未找到 BGM {bgm:?} 的可用搜索结果");
        }
        Ok(())
    }

    /// 自主声明（best-effort）：点入口 → 选默认选项 → 确定。
    /// 对齐 sau `set_self_declaration`：异步渲染 + best-effort，等不到仅 warn。
    async fn set_self_declaration(&self) -> anyhow::Result<()> {
        self.driver.click_text(DOUYIN_SELF_DECL_ENTRY_TEXT).await?;
        self.driver.click_text(DOUYIN_SELF_DECL_OPTION_TEXT).await?;
        self.driver.click_text(DOUYIN_CONFIRM_BTN_TEXT).await?;
        debug!(option = DOUYIN_SELF_DECL_OPTION_TEXT, "自主声明已选择");
        Ok(())
    }

    /// 定时发布：点"定时发布"单选 → 填时间 input。
    /// 对齐 sau `set_schedule_time_douyin`（简化：driver.fill 替代 keyboard.type）。
    async fn set_schedule_time(&self, sched: &str) -> anyhow::Result<()> {
        info!(schedule = sched, "设置定时发布");
        self.driver.click_text(DOUYIN_SCHEDULE_RADIO_TEXT).await?;
        self.driver.sleep_ms(SHORT_STABILIZE_MS).await?;
        self.driver
            .fill(DOUYIN_SCHEDULE_TIME_INPUT_SEL, sched)
            .await?;
        Ok(())
    }

    /// 点"发布" → 等内容管理页跳转。对齐 sau 发布循环成功分支。
    async fn click_publish_and_wait(&self) -> anyhow::Result<()> {
        info!("点击发布按钮");
        self.driver.click_text(DOUYIN_PUBLISH_BTN_TEXT).await?;
        self.driver
            .wait_for_url_contains(DOUYIN_MANAGE_FRAG)
            .await?;
        info!("发布成功，已跳转到内容管理页");
        Ok(())
    }
}

#[async_trait]
impl Uploader for DouyinUploader {
    fn platform(&self) -> &'static str {
        "douyin"
    }

    async fn upload_note(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        self.upload_impl(req, false).await
    }

    async fn upload_video(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        self.upload_impl(req, true).await
    }
}

// =====================================================================================
// 单测
// =====================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// 仅记录调用次数的 driver：用于证明 dry_run 不触发任何 driver 操作。
    struct CountingDriver {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl BrowserDriver for CountingDriver {
        async fn goto(&self, _url: &str) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn current_url(&self) -> anyhow::Result<String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(String::new())
        }
        async fn is_text_present(&self, _text: &str) -> anyhow::Result<bool> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(false)
        }
        async fn click_text(&self, _text: &str) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn click_selector(&self, _selector: &str) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn fill(&self, _selector: &str, _value: &str) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn set_input_files(&self, _selector: &str, _paths: &[String]) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn wait_for_url_contains(&self, _fragment: &str) -> anyhow::Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn sleep_ms(&self, _ms: u64) -> anyhow::Result<()> {
            Ok(())
        }
    }

    /// 全 Ok 且记录调用日志的 driver：用于 confirm=true 路径的流程驱动。
    /// `current_url` 返回上传页 URL（使 `is_logged_in` 判定已登录），
    /// `is_text_present` 仅对"重新上传"返回 true（使转码轮询立即完成）。
    struct OkLoggerDriver {
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl BrowserDriver for OkLoggerDriver {
        async fn goto(&self, url: &str) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("goto:{url}"));
            Ok(())
        }
        async fn current_url(&self) -> anyhow::Result<String> {
            let uploaded = self
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry.starts_with("input:div[class^='container'] input#"));
            Ok(if uploaded {
                format!("https://creator.douyin.com/creator-micro/{DOUYIN_PUBLISH_V2_FRAG}")
            } else {
                DOUYIN_UPLOAD_URL.to_string()
            })
        }
        async fn is_text_present(&self, text: &str) -> anyhow::Result<bool> {
            Ok(text == DOUYIN_REUPLOAD_TEXT)
        }
        async fn click_text(&self, text: &str) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("click_text:{text}"));
            Ok(())
        }
        async fn click_selector(&self, sel: &str) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("click_sel:{sel}"));
            Ok(())
        }
        async fn fill(&self, sel: &str, val: &str) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("fill:{sel}={val}"));
            Ok(())
        }
        async fn type_text(&self, sel: &str, val: &str) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("type:{sel}={val}"));
            Ok(())
        }
        async fn set_input_files(&self, sel: &str, paths: &[String]) -> anyhow::Result<()> {
            self.log
                .lock()
                .unwrap()
                .push(format!("input:{sel}#{}", paths.len()));
            Ok(())
        }
        async fn set_input_files_nth(
            &self,
            sel: &str,
            index: usize,
            paths: &[String],
        ) -> anyhow::Result<()> {
            self.log
                .lock()
                .unwrap()
                .push(format!("input:{sel}@{index}#{}", paths.len()));
            Ok(())
        }
        async fn wait_for_url_contains(&self, frag: &str) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("waiturl:{frag}"));
            Ok(())
        }
        async fn press_key(&self, key: &str) -> anyhow::Result<()> {
            self.log.lock().unwrap().push(format!("key:{key}"));
            Ok(())
        }
        async fn sleep_ms(&self, _ms: u64) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn req(dry_run: bool, confirm: bool, is_video: bool) -> UploadRequest {
        let media_paths = if is_video {
            vec!["/tmp/video.mp4".to_string(), "/tmp/cover.jpg".to_string()]
        } else {
            vec!["/tmp/img1.jpg".to_string(), "/tmp/img2.jpg".to_string()]
        };
        UploadRequest {
            platform: "douyin".into(),
            account_profile: "p1".into(),
            title: "测试标题".into(),
            desc: "正文描述".into(),
            tags: vec!["话题A".into(), "话题B".into()],
            media_paths,
            schedule: Some("2026-07-08 10:00".into()),
            dry_run,
            confirm,
        }
    }

    // ---- 测试 1：dry_run 返回 planned 且不调用 driver ----
    #[tokio::test]
    async fn dry_run_returns_planned_without_touching_driver() {
        let calls = Arc::new(AtomicUsize::new(0));
        let drv = CountingDriver {
            calls: calls.clone(),
        };
        let up = DouyinUploader::new(Box::new(drv));

        let r = up.upload_video(&req(true, false, true)).await.unwrap();
        assert!(r.dry_run, "dry_run=true 时结果必须 dry_run=true");
        assert!(
            r.note.starts_with("planned: douyin video upload"),
            "note 应以 planned 动作前缀开头，实际: {}",
            r.note
        );
        assert!(r.note.contains(DOUYIN_UPLOAD_URL), "note 应含上传页 URL");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "dry_run 不得调用任何 driver 方法（sleep 除外）"
        );

        // confirm=false 但 dry_run=false 也应落到 dry-run 兜底（安全闸默认保守）。
        let r2 = up.upload_video(&req(false, false, true)).await.unwrap();
        assert!(r2.dry_run, "confirm=false 时必须返回 dry-run 计划");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    // ---- 测试 2：URL/选择器/登录检测常量与 sau 一致 ----
    #[test]
    fn url_and_selector_constants_match_sau() {
        // URL
        assert_eq!(
            DOUYIN_UPLOAD_URL,
            "https://creator.douyin.com/creator-micro/content/upload"
        );
        assert_eq!(DOUYIN_HOME_URL, "https://creator.douyin.com/");
        assert_eq!(DOUYIN_PUBLISH_V1_FRAG, "content/publish");
        assert_eq!(DOUYIN_PUBLISH_V2_FRAG, "content/post/video");
        assert_eq!(DOUYIN_NOTE_PUBLISH_FRAG, "content/post/image");
        assert_eq!(DOUYIN_MANAGE_FRAG, "content/manage");
        assert_eq!(DOUYIN_LOGGED_IN_FRAG, "creator.douyin.com/creator-micro");

        // 登录检测文本（sau cookie_auth / _is_douyin_login_completed）
        assert_eq!(DOUYIN_LOGIN_TEXT_PHONE, "手机号登录");
        assert_eq!(DOUYIN_LOGIN_TEXT_SCAN, "扫码登录");
        assert_eq!(DOUYIN_LOGIN_TEXT_QR_EXPIRED, "二维码失效");

        // 关键选择器
        assert_eq!(DOUYIN_VIDEO_INPUT_SEL, "div[class^='container'] input");
        assert!(DOUYIN_TITLE_INPUT_SEL.contains("填写作品标题"));
        assert!(DOUYIN_DESC_EDITOR_SEL.contains("zone-container"));
        assert_eq!(DOUYIN_REUPLOAD_TEXT, "重新上传");
        assert_eq!(DOUYIN_PUBLISH_BTN_TEXT, "发布");
    }

    // ---- 测试 3：流程步骤序列可断言（视频：首尾、包含、相对顺序）----
    #[test]
    fn planned_video_steps_sequence_is_assertable() {
        let steps = planned_steps(&req(true, false, true), true);
        // 首尾
        assert_eq!(steps.first(), Some(&"check_login"));
        assert_eq!(steps.last(), Some(&"wait_manage_page"));
        // 关键步骤存在
        assert!(steps.contains(&"set_video_files"));
        assert!(steps.contains(&"wait_publish_page"));
        assert!(steps.contains(&"wait_transcode"));
        assert!(
            steps.contains(&"set_cover"),
            "media_paths>1 时应含 set_cover"
        );
        assert!(steps.contains(&"set_self_declaration"));
        assert!(
            steps.contains(&"set_schedule_time"),
            "schedule 存在时应含定时步骤"
        );
        // 相对顺序：上传 → 等转码
        let pos_upload = steps.iter().position(|s| *s == "set_video_files").unwrap();
        let pos_transcode = steps.iter().position(|s| *s == "wait_transcode").unwrap();
        assert!(pos_upload < pos_transcode);
        // 发布必须是倒数第二步，等管理页是最后一步
        let pos_pub = steps.iter().position(|s| *s == "click_publish").unwrap();
        assert_eq!(pos_pub, steps.len() - 2);
        assert_eq!(
            steps.len() - 1,
            steps.iter().position(|s| *s == "wait_manage_page").unwrap()
        );
    }

    // ---- 测试 4：图文步骤序列（无转码/无封面/有 note tab）----
    #[test]
    fn planned_note_steps_skip_video_only_steps() {
        let steps = planned_steps(&req(true, false, false), false);
        assert!(steps.contains(&"click_note_tab"));
        assert!(steps.contains(&"set_image_files"));
        assert!(!steps.contains(&"wait_transcode"), "图文不应有转码步骤");
        assert!(!steps.contains(&"set_cover"), "图文不应有封面步骤");
        assert!(!steps.contains(&"set_self_declaration"), "图文无自主声明");
        assert_eq!(steps.last(), Some(&"wait_manage_page"));
    }

    // ---- 测试 5：confirm=true 真实执行，driver 调用序列与流程一致 ----
    #[tokio::test]
    async fn confirm_video_flow_drives_full_pipeline() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let up = DouyinUploader::new(Box::new(OkLoggerDriver { log: log.clone() }));

        let r = up.upload_video(&req(false, true, true)).await.unwrap();
        assert!(!r.dry_run, "confirm=true && dry_run=false 应真执行");
        assert!(r.note.starts_with("douyin video published"));

        let l = log.lock().unwrap().clone();
        // 视频文件上传（sau 选择器）
        assert!(
            l.iter()
                .any(|s| s.starts_with("input:div[class^='container'] input#1")),
            "应调用 set_input_files 上传 1 个视频文件，log={l:?}"
        );
        // 标题填写（截断后的标题）
        assert!(l
            .iter()
            .any(|s| s.starts_with("fill:input[placeholder*='填写作品标题']=")));
        // 描述+话题编辑器
        assert!(l.iter().any(|s| s.starts_with("fill:div.zone-container")));
        // 封面上传
        assert!(l
            .iter()
            .any(|s| s.contains("input:div.dy-creator-content-modal")));
        // 点发布 + 等管理页
        assert!(l.iter().any(|s| s == "click_text:发布"), "应点击发布按钮");
        assert!(
            l.iter().any(|s| s == "waiturl:content/manage"),
            "应等待内容管理页跳转"
        );
        assert!(
            !l.iter()
                .any(|s| s.contains("goto:https://creator.douyin.com/") && !s.contains("upload")),
            "is_logged_in=true 时不应触发扫码兜底 goto 首页"
        );
    }

    // ---- 测试 6：描述+话题拼接 ----
    #[test]
    fn desc_builder_combines_desc_and_tags() {
        assert_eq!(
            build_desc_with_tags("正文", &["a".into(), "b".into()]),
            "正文 #a #b"
        );
        assert_eq!(build_desc_with_tags("", &["x".into()]), "#x");
        assert_eq!(build_desc_with_tags("正文", &[]), "正文");
        // 空白 tag 被忽略
        assert_eq!(
            build_desc_with_tags("正文", &["  ".into(), "y".into()]),
            "正文 #y"
        );
    }

    // ---- 测试 7：标题按字符截断（中文友好）----
    #[test]
    fn title_truncates_by_char() {
        assert_eq!(truncate_title("短标题"), "短标题");
        let long = "一二三四五六七八九十一二三四五六七八九十一二三四五六七八九十"; // 30 字
        assert_eq!(truncate_title(long).chars().count(), DOUYIN_TITLE_MAX_CHARS);
    }

    // ---- 测试 8：check_cookie_file 复用 Account ----
    #[test]
    fn check_cookie_file_uses_account() {
        let acc = Account::new("douyin", "p1");
        let tmp = std::env::temp_dir().join(format!("dy-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(tmp.join("cookies"));
        assert!(!check_cookie_file(&acc, &tmp));
        std::fs::write(acc.cookie_path(&tmp), "[]").unwrap();
        assert!(check_cookie_file(&acc, &tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
