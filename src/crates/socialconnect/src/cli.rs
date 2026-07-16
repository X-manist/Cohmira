//! Social Connection 纯 Rust 动作参数模型。
//!
//! 该模块集中维护平台参数约束，并生成可用于 dry-run、日志和测试的动作计划。动作计划只在
//! Rust 进程内消费，不会启动外部 CLI 或 Python 子进程。

use crate::account::Account;
use crate::schedule::parse_schedule;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoUploadOptions {
    pub file: String,
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub schedule: Option<String>,
    pub thumbnail: Option<String>,
    pub thumbnail_landscape: Option<String>,
    pub thumbnail_portrait: Option<String>,
    pub product_link: Option<String>,
    pub product_title: Option<String>,
    pub tid: Option<u64>,
    pub short_title: Option<String>,
    pub category: Option<String>,
    pub draft: bool,
    pub playlist: Option<String>,
    pub visibility: Option<String>,
    pub debug: bool,
    pub headless: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoteUploadOptions {
    pub images: Vec<String>,
    pub title: String,
    pub note: String,
    pub note_file: Option<String>,
    pub tags: Vec<String>,
    pub schedule: Option<String>,
    pub bgm: Option<String>,
    pub debug: bool,
    pub headless: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeActionPlan {
    args: Vec<String>,
}

impl NativeActionPlan {
    pub fn login(account: &Account, headless: bool) -> Self {
        let mut args = vec![
            account.platform.clone(),
            "login".into(),
            "--account".into(),
            account.profile.clone(),
        ];
        if account.platform != "bilibili" {
            args.push(if headless && account.platform != "youtube" {
                "--headless".into()
            } else {
                "--headed".into()
            });
        }
        Self { args }
    }

    pub fn check(account: &Account) -> Self {
        Self {
            args: vec![
                account.platform.clone(),
                "check".into(),
                "--account".into(),
                account.profile.clone(),
            ],
        }
    }

    /// 构造视频发布动作计划，并校验平台专属参数。
    pub fn upload_video(account: &Account, options: &VideoUploadOptions) -> anyhow::Result<Self> {
        ensure_upload_capability(account, false)?;
        require_text("视频文件", &options.file)?;
        require_text("标题", &options.title)?;
        validate_schedule(options.schedule.as_deref())?;
        if account.platform == "xiaohongshu" && normalized_tag_count(&options.tags) > 10 {
            anyhow::bail!(
                "小红书标签最多 10 个，当前为 {} 个",
                normalized_tag_count(&options.tags)
            );
        }

        let mut args = vec![
            "--file".into(),
            options.file.clone(),
            "--title".into(),
            options.title.clone(),
        ];
        push_text(&mut args, "--desc", &options.description);
        push_tags(&mut args, &options.tags);
        push_optional(&mut args, "--schedule", options.schedule.as_deref());

        match account.platform.as_str() {
            "douyin" => {
                push_optional(&mut args, "--thumbnail", options.thumbnail.as_deref());
                push_optional(
                    &mut args,
                    "--thumbnail-landscape",
                    options.thumbnail_landscape.as_deref(),
                );
                push_optional(
                    &mut args,
                    "--thumbnail-portrait",
                    options.thumbnail_portrait.as_deref(),
                );
                push_optional(&mut args, "--product-link", options.product_link.as_deref());
                push_optional(
                    &mut args,
                    "--product-title",
                    options.product_title.as_deref(),
                );
                push_runtime_flags(&mut args, options.debug, options.headless);
            }
            "kuaishou" | "xiaohongshu" => {
                push_optional(&mut args, "--thumbnail", options.thumbnail.as_deref());
                push_runtime_flags(&mut args, options.debug, options.headless);
            }
            "bilibili" => {
                require_text("Bilibili 视频简介", &options.description)?;
                let tid = options
                    .tid
                    .filter(|value| *value > 0)
                    .ok_or_else(|| anyhow::anyhow!("Bilibili 发布必须提供大于 0 的 tid 分区 ID"))?;
                args.push("--tid".into());
                args.push(tid.to_string());
            }
            "tencent" => {
                push_optional(&mut args, "--thumbnail", options.thumbnail.as_deref());
                push_optional(
                    &mut args,
                    "--thumbnail-landscape",
                    options.thumbnail_landscape.as_deref(),
                );
                push_optional(
                    &mut args,
                    "--thumbnail-portrait",
                    options.thumbnail_portrait.as_deref(),
                );
                push_optional(&mut args, "--short-title", options.short_title.as_deref());
                push_optional(&mut args, "--category", options.category.as_deref());
                if options.draft {
                    args.push("--draft".into());
                }
                push_runtime_flags(&mut args, options.debug, options.headless);
            }
            "youtube" => {
                if options.schedule.is_some() {
                    anyhow::bail!("YouTube 参考 CLI 不支持 --schedule");
                }
                push_optional(&mut args, "--thumbnail", options.thumbnail.as_deref());
                push_optional(&mut args, "--playlist", options.playlist.as_deref());
                let visibility = options.visibility.as_deref().unwrap_or("public");
                if !matches!(visibility, "public" | "unlisted" | "private") {
                    anyhow::bail!("YouTube visibility 仅支持 public/unlisted/private");
                }
                args.push("--visibility".into());
                args.push(visibility.into());
                push_runtime_flags(&mut args, options.debug, options.headless);
            }
            _ => unreachable!("Account 已完成平台校验"),
        }

        Self::action(account, "upload-video", args)
    }

    /// 构造 Douyin/Kuaishou/Xiaohongshu 图文发布动作计划。
    pub fn upload_note(account: &Account, options: &NoteUploadOptions) -> anyhow::Result<Self> {
        ensure_upload_capability(account, true)?;
        if options.images.is_empty() || options.images.iter().any(|value| value.trim().is_empty()) {
            anyhow::bail!("图文发布至少需要一张有效图片");
        }
        require_text("标题", &options.title)?;
        validate_schedule(options.schedule.as_deref())?;
        if account.platform == "douyin" {
            if options.title.chars().count() > 20 {
                anyhow::bail!(
                    "抖音图文标题不能超过 20 字符，当前为 {} 字符",
                    options.title.chars().count()
                );
            }
            if options.images.len() > 35 {
                anyhow::bail!("抖音图文最多支持 35 张图片");
            }
            if options.note_file.is_none() && options.note.chars().count() > 1_000 {
                anyhow::bail!(
                    "抖音图文正文不能超过 1000 字符，当前为 {} 字符",
                    options.note.chars().count()
                );
            }
        }
        if account.platform == "xiaohongshu" && normalized_tag_count(&options.tags) > 10 {
            anyhow::bail!(
                "小红书标签最多 10 个，当前为 {} 个",
                normalized_tag_count(&options.tags)
            );
        }

        let mut args = vec!["--images".into()];
        args.extend(options.images.iter().cloned());
        args.push("--title".into());
        args.push(options.title.clone());
        push_text(&mut args, "--note", &options.note);
        if options.note_file.is_some() && account.platform != "douyin" {
            anyhow::bail!("--notef 仅支持抖音图文发布");
        }
        push_optional(&mut args, "--notef", options.note_file.as_deref());
        push_tags(&mut args, &options.tags);
        push_optional(&mut args, "--schedule", options.schedule.as_deref());
        if account.platform == "douyin" {
            push_optional(&mut args, "--bgm", options.bgm.as_deref());
        }
        push_runtime_flags(&mut args, options.debug, options.headless);
        Self::action(account, "upload-note", args)
    }

    /// 构造任意内部动作，例如 `upload-video` / `upload-note`。
    pub fn action(
        account: &Account,
        action: &str,
        trailing_args: impl IntoIterator<Item = String>,
    ) -> anyhow::Result<Self> {
        let action = action.trim();
        if action.is_empty() || action.starts_with('-') || action.chars().any(char::is_whitespace) {
            anyhow::bail!("非法 Social Connection action: {action:?}");
        }
        let mut args = vec![
            account.platform.clone(),
            action.to_string(),
            "--account".into(),
            account.profile.clone(),
        ];
        args.extend(trailing_args);
        Ok(Self { args })
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn into_args(self) -> Vec<String> {
        self.args
    }
}

fn ensure_upload_capability(account: &Account, note: bool) -> anyhow::Result<()> {
    let supported = if note {
        matches!(
            account.platform.as_str(),
            "douyin" | "kuaishou" | "xiaohongshu"
        )
    } else {
        matches!(
            account.platform.as_str(),
            "douyin" | "kuaishou" | "xiaohongshu" | "bilibili" | "tencent" | "youtube"
        )
    };
    if !supported {
        anyhow::bail!(
            "{} 不支持 {}",
            account.platform,
            if note { "upload-note" } else { "upload-video" }
        );
    }
    Ok(())
}

fn validate_schedule(schedule: Option<&str>) -> anyhow::Result<()> {
    if let Some(schedule) = schedule.map(str::trim).filter(|value| !value.is_empty()) {
        parse_schedule(schedule).map_err(|error| {
            anyhow::anyhow!("无效排期 {schedule:?}，格式应为 YYYY-MM-DD HH:MM：{error}")
        })?;
    }
    Ok(())
}

fn require_text(label: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("{label}不能为空");
    }
    Ok(())
}

fn push_text(args: &mut Vec<String>, flag: &str, value: &str) {
    if !value.trim().is_empty() {
        args.push(flag.into());
        args.push(value.into());
    }
}

fn push_optional(args: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        args.push(flag.into());
        args.push(value.into());
    }
}

fn push_tags(args: &mut Vec<String>, tags: &[String]) {
    let tags = tags
        .iter()
        .map(|tag| tag.trim().trim_start_matches('#'))
        .filter(|tag| !tag.is_empty())
        .collect::<Vec<_>>();
    if !tags.is_empty() {
        args.push("--tags".into());
        args.push(tags.join(","));
    }
}

fn normalized_tag_count(tags: &[String]) -> usize {
    tags.iter()
        .map(|tag| tag.trim().trim_start_matches('#'))
        .filter(|tag| !tag.is_empty())
        .count()
}

fn push_runtime_flags(args: &mut Vec<String>, debug: bool, headless: Option<bool>) {
    if debug {
        args.push("--debug".into());
    }
    if let Some(headless) = headless {
        args.push(if headless {
            "--headless".into()
        } else {
            "--headed".into()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_command_matches_reference_contract() {
        let account = Account::new("xhs", "staff_01");
        assert_eq!(
            NativeActionPlan::login(&account, true).into_args(),
            vec![
                "xiaohongshu",
                "login",
                "--account",
                "staff_01",
                "--headless"
            ]
        );
    }

    #[test]
    fn raw_upload_command_preserves_platform_specific_flags() {
        let account = Account::new("douyin", "creator");
        let command = NativeActionPlan::action(
            &account,
            "upload-video",
            [
                "--file".into(),
                "demo.mp4".into(),
                "--title".into(),
                "测试".into(),
                "--schedule".into(),
                "2026-07-12 20:00".into(),
            ],
        )
        .unwrap();
        assert_eq!(command.args()[0], "douyin");
        assert_eq!(command.args()[1], "upload-video");
        assert!(command.args().contains(&"--schedule".to_string()));
    }

    #[test]
    fn typed_video_commands_cover_platform_specific_contracts() {
        let douyin = Account::new("douyin", "creator");
        let command = NativeActionPlan::upload_video(
            &douyin,
            &VideoUploadOptions {
                file: "demo.mp4".into(),
                title: "测试".into(),
                tags: vec!["#科技".into(), "AI".into()],
                schedule: Some("2026-07-12 20:00".into()),
                thumbnail_landscape: Some("landscape.png".into()),
                product_link: Some("https://example.com/product".into()),
                headless: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(command.args().contains(&"--thumbnail-landscape".into()));
        assert!(command.args().contains(&"科技,AI".into()));
        assert!(command.args().contains(&"--headless".into()));

        let bilibili = Account::new("bilibili", "creator");
        let error = NativeActionPlan::upload_video(
            &bilibili,
            &VideoUploadOptions {
                file: "demo.mp4".into(),
                title: "测试".into(),
                description: "简介".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("tid"));

        let xhs = Account::new("xiaohongshu", "creator");
        let error = NativeActionPlan::upload_video(
            &xhs,
            &VideoUploadOptions {
                file: "demo.mp4".into(),
                title: "测试".into(),
                tags: (0..11).map(|index| format!("tag{index}")).collect(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("最多 10 个"));
    }

    #[test]
    fn typed_note_commands_reject_unsupported_platforms_and_bad_schedule() {
        let youtube = Account::new("youtube", "creator");
        let error = NativeActionPlan::upload_note(
            &youtube,
            &NoteUploadOptions {
                images: vec!["a.png".into()],
                title: "标题".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("不支持 upload-note"));

        let xhs = Account::new("xiaohongshu", "creator");
        let error = NativeActionPlan::upload_note(
            &xhs,
            &NoteUploadOptions {
                images: vec!["a.png".into()],
                title: "标题".into(),
                schedule: Some("tomorrow".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("YYYY-MM-DD HH:MM"));

        let douyin = Account::new("douyin", "creator");
        let plan = NativeActionPlan::upload_note(
            &douyin,
            &NoteUploadOptions {
                images: vec!["a.png".into()],
                title: "标题".into(),
                note_file: Some("content.md".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(plan.args().contains(&"--notef".into()));
        assert!(plan.args().contains(&"content.md".into()));

        let error = NativeActionPlan::upload_note(
            &douyin,
            &NoteUploadOptions {
                images: vec!["a.png".into()],
                title: "超".repeat(21),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("20 字符"));
    }
}
