//! B站视频投稿 RPA 适配器（member.bilibili.com/platform/upload）。
//!
//! 移植自 social-auto-upload (sau) 的 B站投稿流程。原 sau 在此平台改用了 `biliup` CLI
//! （见 `social-connection/uploader/bilibili_uploader/runtime.py`）；本适配器按主控要求实现
//! 受控浏览器自动化（RPA）路径：直接驱动 member.bilibili.com 投稿页完成视频投稿。
//!
//! # 流程
//! 1. 打开投稿页 [`BILI_UPLOAD_URL`]
//! 2. 登录检测：URL 跳转登录页 / 缺少就绪文案 → 报错要求先登录
//! 3. 上传视频文件（[`SEL_VIDEO_INPUT`]）
//! 4. 等待上传完成（[`SEL_UPLOAD_SUCCESS`] 出现；受 trait 限制用 sleep 兜底）
//! 5. 填标题（[`SEL_TITLE`]）
//! 6. 选分区（[`SEL_CATEGORY_TRIGGER`] + 文本点击；分区名由 [`BiliUploader::with_category`] 注入）
//! 7. 填标签（[`SEL_TAG_INPUT`]）
//! 8. 填简介（[`SEL_DESC`]）
//! 9. 点击「立即投稿」（[`SEL_SUBMIT`]）
//! 10. 等待跳转完成页（[`BILI_PUBLISH_DONE_FRAGMENT`]）
//!
//! # 安全闸
//! [`UploadRequest::dry_run`] = true 或 [`UploadRequest::confirm`] = false 时，直接返回 `planned`
//! 计划，完全不触碰 driver；仅当 `confirm = true`（且 `dry_run = false`）才真执行 RPA。
//!
//! # RPA 选择器
//! member.bilibili.com 投稿页 DOM 会随版本变化，所有 URL / 选择器 / 登录检测文本均抽为
//! `pub const`，便于流程单测断言与集中维护；真实发布前需用真实账号回归（见 coverage_notes）。

use crate::browser::{BrowserDriver, StubDriver};
use crate::uploader::{UploadRequest, UploadResult, Uploader};
use anyhow::{bail, Result};
use async_trait::async_trait;

// ===== 常量：URL / 选择器 / 登录检测文本（集中维护，便于单测断言） =====

/// 投稿页 URL（member.bilibili.com 视频上传 frame）。
pub const BILI_UPLOAD_URL: &str = "https://member.bilibili.com/platform/upload/video/frame";
/// 投稿成功后跳转片段（完成页）。
pub const BILI_PUBLISH_DONE_FRAGMENT: &str = "/platform/upload/video/finish";
/// 登录页 URL 片段（未登录时会被跳转到 passport）。
pub const BILI_LOGIN_FRAGMENT: &str = "passport.bilibili.com/login";
/// 登录就绪文案：投稿页主提交按钮文字，出现即视为已登录。
pub const BILI_LOGIN_READY_TEXT: &str = "立即投稿";

/// 视频文件 input（投稿页拖拽区内的隐藏 file input）。
pub const SEL_VIDEO_INPUT: &str = ".bcc-upload-video-wrapper input";
/// 上传成功标记（出现即视为视频传完）。
pub const SEL_UPLOAD_SUCCESS: &str = ".upload-success-collapsable";
/// 标题输入框。
pub const SEL_TITLE: &str = ".input-title";
/// 简介输入区（contenteditable）。
pub const SEL_DESC: &str = ".desc-container [contenteditable=\"true\"]";
/// 标签输入框。
pub const SEL_TAG_INPUT: &str = ".tag-container input";
/// 分区下拉触发器。
pub const SEL_CATEGORY_TRIGGER: &str = ".video-type-select";
/// 提交按钮（立即投稿）。
pub const SEL_SUBMIT: &str = ".submit-add";

/// 页面渲染 / 上传稳定的兜底等待（毫秒）。
const SETTLE_MS: u64 = 2000;

/// B站视频投稿器。
///
/// 持有 [`BrowserDriver`]（trait object）；平台专属参数（如分区）走构造器注入，
/// 平台无关的发布草稿走 [`UploadRequest`]。
pub struct BiliUploader {
    driver: Box<dyn BrowserDriver>,
    /// 分区显示名（如「科技 / 数码」）。B站投稿必填，未设置时发布步骤会跳过分区选择
    /// 并在结果 note 标注（实际发布会被 B站阻断，属已知限制）。
    category: Option<String>,
    tid: Option<u64>,
}

impl BiliUploader {
    /// 用任意驱动构造。分区默认未设置，可用 [`with_category`] 补上。
    pub fn new(driver: Box<dyn BrowserDriver>) -> Self {
        Self {
            driver,
            category: None,
            tid: None,
        }
    }

    /// 用占位驱动构造（仅用于流程结构 / 编译期占位 / dry_run 单测，真实发布需 CDP 后端）。
    pub fn with_stub() -> Self {
        Self::new(Box::new(StubDriver))
    }

    /// 注入分区显示名（builder 风格）。平台专属参数不污染平台无关的 [`UploadRequest`]。
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }

    pub fn with_tid(mut self, tid: Option<u64>) -> Self {
        self.tid = tid.filter(|value| *value > 0);
        self
    }

    /// 登录态是否就绪：URL 未跳登录页，且投稿页存在就绪文案（[`BILI_LOGIN_READY_TEXT`]）。
    async fn is_logged_in(&self) -> Result<bool> {
        let url = self.driver.current_url().await?;
        if url.contains(BILI_LOGIN_FRAGMENT) {
            return Ok(false);
        }
        // is_text_present 为 true 即视为投稿页就绪（已登录）。
        self.driver.is_text_present(BILI_LOGIN_READY_TEXT).await
    }

    /// 选择分区：打开分区下拉 → 点击分区显示名。
    async fn select_category(&self, category: &str) -> Result<()> {
        self.driver.click_selector(SEL_CATEGORY_TRIGGER).await?;
        self.driver.click_text(category).await?;
        Ok(())
    }

    async fn select_tid(&self, tid: u64) -> Result<()> {
        self.driver.click_selector(SEL_CATEGORY_TRIGGER).await?;
        let script = format!(
            "(()=>{{const wanted='{tid}';const nodes=[...document.querySelectorAll('[data-value],[value],li,div')];const el=nodes.find(n=>String(n.dataset&&n.dataset.value||n.getAttribute('value')||'')===wanted);if(!el)return false;el.click();return true;}})()"
        );
        if !self
            .driver
            .evaluate(&script)
            .await?
            .as_bool()
            .unwrap_or(false)
        {
            anyhow::bail!("Bilibili 投稿页未找到 tid={tid} 的分区选项");
        }
        Ok(())
    }

    async fn set_schedule(&self, schedule: &str) -> Result<()> {
        self.driver.click_text("定时发布").await?;
        let selector = "input[placeholder*='发布时间'], input[placeholder*='日期']";
        self.driver.wait_for_selector(selector, 10_000).await?;
        self.driver.fill(selector, schedule).await?;
        self.driver.press_key("Enter").await?;
        Ok(())
    }

    /// 生成 dry_run 计划步骤（人类可读）。步骤固定，仅在设置了分区时附注分区名。
    fn plan_steps(&self) -> Vec<&'static str> {
        vec![
            "goto 投稿页 member.bilibili.com/platform/upload/video/frame",
            "检测登录态（URL 不跳 passport.bilibili.com/login 且存在「立即投稿」文案）",
            "set_input_files 上传视频文件",
            "等待上传完成",
            "fill 标题",
            "点击分区下拉并选择分区",
            "fill 标签",
            "fill 简介",
            "点击「立即投稿」",
            "等待跳转完成页 /platform/upload/video/finish",
        ]
    }
}

#[async_trait]
impl Uploader for BiliUploader {
    fn platform(&self) -> &'static str {
        "bilibili"
    }

    /// B站以视频投稿为主；图文复用同一投稿页入口，这里暂复用 [`upload_video`] 流程占位。
    async fn upload_note(&self, req: &UploadRequest) -> Result<UploadResult> {
        self.upload_video(req).await
    }

    async fn upload_video(&self, req: &UploadRequest) -> Result<UploadResult> {
        // === 安全闸：dry_run 或未 confirm → 直接返回 planned 计划，不调 driver ===
        if req.dry_run || !req.confirm {
            let steps = self.plan_steps();
            let mut note = format!("planned: bilibili 视频投稿，共 {} 步", steps.len());
            note.push_str("\n  - ");
            note.push_str(&steps.join("\n  - "));
            if self.category.is_none() && self.tid.is_none() {
                note.push_str("\n  ⚠ 未设置分区（with_category），真实发布会被 B站阻断");
            } else if let Some(tid) = self.tid {
                note.push_str(&format!("\n  ✓ 分区 tid: {tid}"));
            } else {
                note.push_str(&format!(
                    "\n  ✓ 分区: {}",
                    self.category.as_deref().unwrap_or("")
                ));
            }
            return Ok(UploadResult {
                platform_post_id: None,
                url: None,
                dry_run: true,
                note,
            });
        }

        // === 真实执行（confirm=true） ===
        if req.media_paths.is_empty() {
            bail!("bilibili upload_video: 至少需要一个视频文件 (media_paths)");
        }
        if req.title.trim().is_empty() {
            bail!("bilibili upload_video: title 不能为空");
        }

        // 1. 打开投稿页
        self.driver.goto(BILI_UPLOAD_URL).await?;
        self.driver.sleep_ms(SETTLE_MS).await?;

        // 2. 登录检测
        if !self.is_logged_in().await? {
            bail!(
                "bilibili: 未登录或登录态失效（URL 含 {} 或缺「{}」文案），请先完成 member.bilibili.com 登录",
                BILI_LOGIN_FRAGMENT,
                BILI_LOGIN_READY_TEXT
            );
        }

        // 3. 上传视频
        self.driver
            .set_input_files(SEL_VIDEO_INPUT, &req.media_paths)
            .await?;
        // 4. 等待上传完成。
        self.driver
            .wait_for_selector(SEL_UPLOAD_SUCCESS, 30 * 60 * 1_000)
            .await?;

        // 5. 标题
        self.driver.fill(SEL_TITLE, &req.title).await?;

        // 6. 分区（平台专属参数，由构造器注入）
        if let Some(category) = self.category.as_deref() {
            self.select_category(category).await?;
        } else if let Some(tid) = self.tid {
            self.select_tid(tid).await?;
        } else {
            bail!("bilibili upload_video: 缺少 tid 或分区名称");
        }

        // 7. 标签（逐个填入；多标签追加语义取决于驱动实现，见 coverage_notes）
        for tag in &req.tags {
            self.driver.fill(SEL_TAG_INPUT, tag).await?;
            self.driver.press_key("Enter").await?;
        }

        // 8. 简介
        if !req.desc.trim().is_empty() {
            self.driver.fill(SEL_DESC, &req.desc).await?;
        }

        if let Some(schedule) = req.schedule.as_deref() {
            self.set_schedule(schedule).await?;
        }

        // 9. 立即投稿
        self.driver.click_selector(SEL_SUBMIT).await?;

        // 10. 等待完成页
        self.driver
            .wait_for_url_contains(BILI_PUBLISH_DONE_FRAGMENT)
            .await?;

        let final_url = self.driver.current_url().await.unwrap_or_default();
        Ok(UploadResult {
            platform_post_id: None,
            url: Some(final_url),
            dry_run: false,
            note: "bilibili 视频投稿已提交".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uploader::UploadRequest;

    /// 构造最小可用的发布请求。
    fn sample_req(dry_run: bool, confirm: bool) -> UploadRequest {
        UploadRequest {
            platform: "bilibili".into(),
            account_profile: "creator".into(),
            title: "测试标题".into(),
            desc: "测试简介".into(),
            tags: vec!["标签A".into(), "标签B".into()],
            media_paths: vec!["/tmp/demo.mp4".into()],
            schedule: None,
            dry_run,
            confirm,
        }
    }

    #[tokio::test]
    async fn dry_run_returns_planned_without_touching_driver() {
        // StubDriver 除 sleep_ms 外全部返回错误；dry_run 必须在触碰 driver 前短路返回。
        let uploader = BiliUploader::with_stub().with_category("科技");
        let req = sample_req(true, false);

        let res = uploader.upload_video(&req).await.expect("dry_run 不应报错");

        assert!(res.dry_run, "dry_run 结果必须标记为 dry_run=true");
        assert!(
            res.note.starts_with("planned:"),
            "note 应以 'planned:' 开头，实际: {}",
            res.note
        );
        // 流程步骤关键词断言：覆盖 登录检测 / 上传 / 标题 / 分区 / 标签 / 简介 / 立即投稿 / 完成页
        assert!(res.note.contains("检测登录态"), "缺登录检测步骤");
        assert!(
            res.note.contains("set_input_files 上传视频文件"),
            "缺上传步骤"
        );
        assert!(res.note.contains("fill 标题"), "缺标题步骤");
        assert!(res.note.contains("选择分区"), "缺分区步骤");
        assert!(res.note.contains("立即投稿"), "缺立即投稿步骤");
        assert!(res.note.contains("完成页"), "缺完成页步骤");
        assert!(res.note.contains("科技"), "计划应包含注入的分区名");
        assert!(res.platform_post_id.is_none() && res.url.is_none());
        assert_eq!(uploader.platform(), "bilibili");
    }

    #[test]
    fn url_and_selector_constants_match_member_bilibili() {
        // URL 常量断言：确保流程指向 member.bilibili.com 投稿页。
        assert!(BILI_UPLOAD_URL.starts_with("https://member.bilibili.com/platform/upload"));
        assert!(BILI_UPLOAD_URL.ends_with("/video/frame"));
        assert_eq!(BILI_PUBLISH_DONE_FRAGMENT, "/platform/upload/video/finish");
        assert_eq!(BILI_LOGIN_FRAGMENT, "passport.bilibili.com/login");
        assert_eq!(BILI_LOGIN_READY_TEXT, "立即投稿");

        // 选择器非空（形态合理），便于流程回归时快速定位漂移。
        for sel in [
            SEL_VIDEO_INPUT,
            SEL_UPLOAD_SUCCESS,
            SEL_TITLE,
            SEL_DESC,
            SEL_TAG_INPUT,
            SEL_CATEGORY_TRIGGER,
            SEL_SUBMIT,
        ] {
            assert!(!sel.is_empty(), "选择器不应为空");
        }
        // 投稿页内部选择器普遍以 class 锚定。
        assert!(SEL_VIDEO_INPUT.contains("bcc-upload-video-wrapper"));
        assert!(SEL_SUBMIT.contains("submit"));
    }

    #[tokio::test]
    async fn confirm_gate_invokes_driver_and_fails_under_stub() {
        // confirm=true 且 dry_run=false 应真执行 → 第一步 goto 即触发 StubDriver 报错。
        // 证明安全闸仅在 confirm 时放行，且确实调用了 driver（而非静默返回成功）。
        let uploader = BiliUploader::with_stub();
        let req = sample_req(false, true);

        let err = uploader
            .upload_video(&req)
            .await
            .expect_err("confirm 路径在 StubDriver 下应报错");
        let msg = format!("{err}");
        assert!(
            msg.contains("StubDriver") || msg.contains("CDP"),
            "应来自 StubDriver，实际: {msg}"
        );
    }
}
