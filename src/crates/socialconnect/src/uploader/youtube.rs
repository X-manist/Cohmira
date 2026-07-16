//! YouTube Studio 纯 Rust 浏览器上传器。

use crate::browser::BrowserDriver;
use crate::uploader::{UploadRequest, UploadResult, Uploader};
use async_trait::async_trait;

pub const YOUTUBE_UPLOAD_URL: &str = "https://www.youtube.com/upload";
pub const YOUTUBE_FILE_INPUT: &str = "input[type='file']";
pub const YOUTUBE_TITLE: &str = "#title-textarea #textbox";
pub const YOUTUBE_DESCRIPTION: &str = "#description-textarea #textbox";
pub const YOUTUBE_NEXT: &str = "#next-button";
pub const YOUTUBE_DONE: &str = "#done-button";

pub struct YouTubeUploader {
    driver: Box<dyn BrowserDriver>,
    thumbnail: Option<String>,
    playlist: Option<String>,
    visibility: String,
}

impl YouTubeUploader {
    pub fn new(driver: Box<dyn BrowserDriver>) -> Self {
        Self {
            driver,
            thumbnail: None,
            playlist: None,
            visibility: "public".to_string(),
        }
    }

    pub fn with_options(
        mut self,
        thumbnail: Option<String>,
        playlist: Option<String>,
        visibility: impl Into<String>,
    ) -> Self {
        self.thumbnail = thumbnail;
        self.playlist = playlist;
        self.visibility = visibility.into();
        self
    }

    async fn click_if_present(&self, selector: &str) -> anyhow::Result<bool> {
        if self.driver.selector_exists(selector).await.unwrap_or(false) {
            self.driver.click_selector(selector).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn wait_upload_complete(&self) -> anyhow::Result<()> {
        for _ in 0..360 {
            let body = self
                .driver
                .evaluate("document.body ? document.body.innerText : ''")
                .await
                .unwrap_or_default()
                .as_str()
                .unwrap_or_default()
                .to_string();
            if [
                "上传完成",
                "已上传",
                "处理",
                "检查",
                "Upload complete",
                "Checks",
                "Processing",
            ]
            .iter()
            .any(|marker| body.contains(marker))
            {
                return Ok(());
            }
            self.driver.sleep_ms(5_000).await?;
        }
        anyhow::bail!("等待 YouTube 视频上传完成超时")
    }
}

#[async_trait]
impl Uploader for YouTubeUploader {
    fn platform(&self) -> &'static str {
        "youtube"
    }

    async fn upload_note(&self, _req: &UploadRequest) -> anyhow::Result<UploadResult> {
        anyhow::bail!("YouTube 不支持图文发布")
    }

    async fn upload_video(&self, req: &UploadRequest) -> anyhow::Result<UploadResult> {
        if req.dry_run || !req.confirm {
            return Ok(UploadResult {
                platform_post_id: None,
                url: None,
                dry_run: true,
                note: format!(
                    "planned: youtube video -> upload -> metadata -> audience -> visibility={} -> publish",
                    self.visibility
                ),
            });
        }
        let Some(video) = req.media_paths.first() else {
            anyhow::bail!("YouTube 发布缺少视频文件");
        };
        let visibility = match self.visibility.as_str() {
            "public" => "PUBLIC",
            "unlisted" => "UNLISTED",
            "private" => "PRIVATE",
            other => anyhow::bail!("不支持的 YouTube visibility: {other}"),
        };

        self.driver.goto(YOUTUBE_UPLOAD_URL).await?;
        self.driver.sleep_ms(2_500).await?;
        let url = self.driver.current_url().await?.to_ascii_lowercase();
        if url.contains("accounts.google.com") || url.contains("signin") {
            anyhow::bail!("YouTube 登录态已失效");
        }
        self.driver
            .wait_for_selector(YOUTUBE_FILE_INPUT, 60_000)
            .await?;
        self.driver
            .set_input_files(YOUTUBE_FILE_INPUT, std::slice::from_ref(video))
            .await?;
        self.driver
            .wait_for_selector(YOUTUBE_TITLE, 120_000)
            .await?;
        self.driver
            .fill(
                YOUTUBE_TITLE,
                &req.title.chars().take(100).collect::<String>(),
            )
            .await?;
        if !req.desc.trim().is_empty() {
            self.driver.fill(YOUTUBE_DESCRIPTION, &req.desc).await?;
        }

        if let Some(thumbnail) = self.thumbnail.as_ref() {
            let selector =
                "#file-loader input[type='file'], ytcp-thumbnail-uploader input[type='file']";
            if self.driver.selector_exists(selector).await.unwrap_or(false) {
                let _ = self
                    .driver
                    .set_input_files(selector, std::slice::from_ref(thumbnail))
                    .await;
            }
        }

        // 受众是 YouTube Studio 必填项。
        let _ = self
            .click_if_present("tp-yt-paper-radio-button[name='VIDEO_MADE_FOR_KIDS_NOT_MFK']")
            .await?;

        if !req.tags.is_empty() {
            let _ = self.click_if_present("#toggle-button").await?;
            let tag_selector =
                "#tags-container #text-input, ytcp-form-input-container#tags-container input";
            if self
                .driver
                .selector_exists(tag_selector)
                .await
                .unwrap_or(false)
            {
                self.driver
                    .fill(
                        tag_selector,
                        &req.tags.join(",").chars().take(500).collect::<String>(),
                    )
                    .await?;
            }
        }

        // 播放列表属于增强项；存在匹配项时选择，不存在时不阻断主发布流程。
        if let Some(playlist) = self.playlist.as_deref() {
            if self
                .click_if_present(
                    "#basics ytcp-text-dropdown-trigger, ytcp-video-metadata-playlists ytcp-dropdown-trigger",
                )
                .await?
            {
                self.driver.sleep_ms(600).await?;
                let script = format!(
                    "(()=>{{const wanted={};const nodes=[...document.querySelectorAll('tp-yt-paper-checkbox,ytcp-checkbox-group')];const el=nodes.find(n=>n.textContent&&n.textContent.includes(wanted));if(!el)return false;el.click();return true;}})()",
                    serde_json::to_string(playlist)?
                );
                let _ = self.driver.evaluate(&script).await;
                let _ = self.driver.click_text("完成").await;
                let _ = self.driver.click_text("Done").await;
            }
        }

        for _ in 0..5 {
            let visibility_selector = format!("tp-yt-paper-radio-button[name='{visibility}']");
            if self
                .driver
                .selector_exists(&visibility_selector)
                .await
                .unwrap_or(false)
            {
                break;
            }
            if !self.click_if_present(YOUTUBE_NEXT).await? {
                self.driver.sleep_ms(1_000).await?;
            }
            self.driver.sleep_ms(800).await?;
        }
        self.driver
            .click_selector(&format!("tp-yt-paper-radio-button[name='{visibility}']"))
            .await?;
        self.wait_upload_complete().await?;
        self.driver.wait_for_selector(YOUTUBE_DONE, 60_000).await?;
        self.driver.click_selector(YOUTUBE_DONE).await?;
        self.driver.sleep_ms(3_000).await?;
        Ok(UploadResult {
            platform_post_id: None,
            url: None,
            dry_run: false,
            note: format!("YouTube 视频已按 {} 可见性提交", self.visibility),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::StubDriver;

    #[tokio::test]
    async fn dry_run_does_not_touch_browser() {
        let uploader = YouTubeUploader::new(Box::new(StubDriver));
        let request = UploadRequest {
            platform: "youtube".to_string(),
            account_profile: "default".to_string(),
            title: "title".to_string(),
            desc: String::new(),
            tags: vec![],
            media_paths: vec!["video.mp4".to_string()],
            schedule: None,
            dry_run: true,
            confirm: false,
        };
        assert!(uploader.upload_video(&request).await.unwrap().dry_run);
    }
}
