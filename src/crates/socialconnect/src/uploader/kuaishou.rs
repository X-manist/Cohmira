//! 快手创作者平台纯 Rust RPA 上传器。

use crate::browser::BrowserDriver;
use crate::uploader::{UploadRequest, UploadResult, Uploader};
use async_trait::async_trait;

pub const KUAISHOU_UPLOAD_URL: &str = "https://cp.kuaishou.com/article/publish/video";
pub const KUAISHOU_MANAGE_FRAGMENT: &str = "/article/manage/video";
pub const KUAISHOU_FILE_INPUT: &str = "input[type='file']";
pub const KUAISHOU_EDITOR: &str =
    "div[contenteditable='true'], textarea[placeholder*='描述'], textarea";
pub const KUAISHOU_SCHEDULE_INPUT: &str = "input[placeholder='选择日期时间']";

pub struct KuaishouUploader {
    driver: Box<dyn BrowserDriver>,
    thumbnail: Option<String>,
}

impl KuaishouUploader {
    pub fn new(driver: Box<dyn BrowserDriver>) -> Self {
        Self {
            driver,
            thumbnail: None,
        }
    }

    pub fn with_thumbnail(mut self, thumbnail: Option<String>) -> Self {
        self.thumbnail = thumbnail;
        self
    }

    fn planned(req: &UploadRequest, note: bool) -> UploadResult {
        UploadResult {
            platform_post_id: None,
            url: None,
            dry_run: true,
            note: format!(
                "planned: kuaishou {} -> 打开发布页 -> 上传素材 -> 填描述/话题{} -> 发布",
                if note { "note" } else { "video" },
                if req.schedule.is_some() {
                    " -> 设置定时发布"
                } else {
                    ""
                }
            ),
        }
    }

    async fn prepare(&self) -> anyhow::Result<()> {
        self.driver.goto(KUAISHOU_UPLOAD_URL).await?;
        self.driver.sleep_ms(1_500).await?;
        let url = self.driver.current_url().await?;
        if url.contains("passport.kuaishou.com")
            || self
                .driver
                .selector_exists("main#login-form")
                .await
                .unwrap_or(false)
        {
            anyhow::bail!("快手账号未登录或登录态已失效");
        }
        Ok(())
    }

    async fn fill_content(&self, req: &UploadRequest) -> anyhow::Result<()> {
        self.driver
            .wait_for_selector(KUAISHOU_EDITOR, 60_000)
            .await?;
        let mut content = if req.desc.trim().is_empty() {
            req.title.trim().to_string()
        } else {
            req.desc.trim().to_string()
        };
        for tag in req.tags.iter().take(3) {
            content.push_str(" #");
            content.push_str(tag.trim());
        }
        self.driver.fill(KUAISHOU_EDITOR, &content).await
    }

    async fn wait_upload_finished(&self) -> anyhow::Result<()> {
        for _ in 0..180 {
            if self
                .driver
                .is_text_present("上传失败")
                .await
                .unwrap_or(false)
            {
                anyhow::bail!("快手素材上传失败");
            }
            if !self.driver.is_text_present("上传中").await.unwrap_or(false) {
                return Ok(());
            }
            self.driver.sleep_ms(1_000).await?;
        }
        anyhow::bail!("等待快手素材上传完成超时")
    }

    async fn schedule_if_needed(&self, req: &UploadRequest) -> anyhow::Result<()> {
        let Some(schedule) = req.schedule.as_deref() else {
            return Ok(());
        };
        self.driver.click_text("定时发布").await?;
        self.driver
            .wait_for_selector(KUAISHOU_SCHEDULE_INPUT, 10_000)
            .await?;
        self.driver.fill(KUAISHOU_SCHEDULE_INPUT, schedule).await?;
        // Ant Design DatePicker 需要 input/change 事件；fill 已触发键盘事件，再补原生事件。
        let _ = self
            .driver
            .evaluate(
                "(()=>{const el=document.querySelector(\"input[placeholder='选择日期时间']\");if(!el)return false;el.dispatchEvent(new Event('input',{bubbles:true}));el.dispatchEvent(new Event('change',{bubbles:true}));el.blur();return true;})()",
            )
            .await;
        let _ = self.driver.press_key("Enter").await;
        Ok(())
    }

    async fn thumbnail_if_needed(&self) -> anyhow::Result<()> {
        let Some(thumbnail) = self.thumbnail.as_ref() else {
            return Ok(());
        };
        if self.driver.click_text("封面设置").await.is_err() {
            return Ok(());
        }
        self.driver.sleep_ms(500).await?;
        let _ = self.driver.click_text("上传封面").await;
        let selector = "div[role='document'] input[type='file'], .ant-modal input[type='file']";
        if self.driver.selector_exists(selector).await.unwrap_or(false) {
            self.driver
                .set_input_files(selector, std::slice::from_ref(thumbnail))
                .await?;
            self.driver.sleep_ms(500).await?;
            let _ = self.driver.click_text("确认").await;
        }
        Ok(())
    }

    async fn publish(&self) -> anyhow::Result<UploadResult> {
        self.driver.click_text("发布").await?;
        self.driver.sleep_ms(700).await?;
        if self
            .driver
            .is_text_present("确认发布")
            .await
            .unwrap_or(false)
        {
            self.driver.click_text("确认发布").await?;
        }
        self.driver
            .wait_for_url_contains(KUAISHOU_MANAGE_FRAGMENT)
            .await?;
        Ok(UploadResult {
            platform_post_id: None,
            url: Some(self.driver.current_url().await.unwrap_or_default()),
            dry_run: false,
            note: "快手内容已提交发布".to_string(),
        })
    }
}

#[async_trait]
impl Uploader for KuaishouUploader {
    fn platform(&self) -> &'static str {
        "kuaishou"
    }

    async fn upload_note(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        if req.dry_run || !req.confirm {
            return Ok(Self::planned(req, true));
        }
        if req.media_paths.is_empty() {
            anyhow::bail!("快手图文发布至少需要一张图片");
        }
        self.prepare().await?;
        self.driver.click_text("图文").await?;
        self.driver.sleep_ms(700).await?;
        self.driver
            .wait_for_selector(KUAISHOU_FILE_INPUT, 15_000)
            .await?;
        self.driver
            .set_input_files(KUAISHOU_FILE_INPUT, &req.media_paths)
            .await?;
        self.fill_content(req).await?;
        self.wait_upload_finished().await?;
        self.schedule_if_needed(req).await?;
        self.publish().await
    }

    async fn upload_video(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        if req.dry_run || !req.confirm {
            return Ok(Self::planned(req, false));
        }
        let Some(video) = req.media_paths.first() else {
            anyhow::bail!("快手视频发布缺少视频文件");
        };
        self.prepare().await?;
        self.driver
            .wait_for_selector(KUAISHOU_FILE_INPUT, 15_000)
            .await?;
        self.driver
            .set_input_files(KUAISHOU_FILE_INPUT, std::slice::from_ref(video))
            .await?;
        self.fill_content(req).await?;
        self.wait_upload_finished().await?;
        self.thumbnail_if_needed().await?;
        self.schedule_if_needed(req).await?;
        self.publish().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::StubDriver;

    fn request(note: bool) -> UploadRequest {
        UploadRequest {
            platform: "kuaishou".to_string(),
            account_profile: "default".to_string(),
            title: "标题".to_string(),
            desc: "正文".to_string(),
            tags: vec!["标签".to_string()],
            media_paths: if note {
                vec!["a.png".to_string()]
            } else {
                vec!["a.mp4".to_string()]
            },
            schedule: None,
            dry_run: true,
            confirm: false,
        }
    }

    #[tokio::test]
    async fn dry_run_covers_video_and_note_without_browser() {
        let uploader = KuaishouUploader::new(Box::new(StubDriver));
        assert!(
            uploader
                .upload_video(&request(false))
                .await
                .unwrap()
                .dry_run
        );
        assert!(uploader.upload_note(&request(true)).await.unwrap().dry_run);
    }
}
