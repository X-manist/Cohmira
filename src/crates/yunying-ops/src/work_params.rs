//! 结构化工作参数（workParams）—— e2e 验收的核心契约。
//!
//! 对齐 docs/17 §1：合格的自然语言任务必须输出结构化 `workParams`，不能只给口头建议。
//! e2e 验收脚本（`e2e:operations-tasks`）据此生成 `work-packages/*.md` 与 `work-packages.json`。

use serde::{Deserialize, Serialize};

/// 一次运营任务的结构化产出。对应 docs/17 的 8 类模拟任务（baby_hotspots 等）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkParams {
    /// 采集参数。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crawler: Option<CrawlerParams>,
    /// 报告/笔记沉淀。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<ReportParams>,
    /// 图像生成（GPT image 2 / Seedream）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<AssetGenParams>,
    /// 视频生成（Seedance / Ark Plan）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<AssetGenParams>,
    /// 多平台发布。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<PublishParams>,
    /// 正式执行前缺失的 API/账号/cookie/文件。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_formal_config: Vec<String>,
    /// 员工下一步具体动作。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next_human_actions: Vec<String>,
}

impl WorkParams {
    /// 质量门：至少有一个非空的能力段，且（若有发布）给出 next_human_actions。
    pub fn passes_quality_gate(&self) -> bool {
        let has_segment = self.crawler.is_some()
            || self.report.is_some()
            || self.image.is_some()
            || self.video.is_some()
            || self.publish.is_some();
        if !has_segment {
            return false;
        }
        // 发布类任务必须有人工动作（不无人值守）。
        if self.publish.is_some() && self.next_human_actions.is_empty() {
            return false;
        }
        true
    }
}

/// 采集参数段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlerParams {
    pub platform: String,
    pub keywords: Vec<String>,
    #[serde(default)]
    pub enable_comments: bool,
    #[serde(default)]
    pub max_notes_count: usize,
    #[serde(default)]
    pub save_option: String,
    #[serde(default)]
    pub readback_required: bool,
}

/// 报告/笔记参数段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportParams {
    /// Beav note 路径或文件名。
    pub note_path: String,
    pub sections: Vec<String>,
    #[serde(default)]
    pub sink_content: Vec<String>,
}

/// 资产生成参数段（图/视频共用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetGenParams {
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub aspect_ratio: String,
    #[serde(default)]
    pub count: u32,
    /// 视频时长（秒），图像忽略。
    #[serde(default)]
    pub duration_seconds: Option<u32>,
    /// 目标素材落盘路径。
    #[serde(default)]
    pub target_path: String,
}

/// 发布参数段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishParams {
    pub platform: String,
    pub account_profile: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub media_paths: Vec<String>,
    /// 排期 "YYYY-MM-DD HH:MM"，None=立即。
    #[serde(default)]
    pub schedule: Option<String>,
    /// 默认 dry-run；真实发布需 confirm=true。
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub confirm: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_params_fails_gate() {
        assert!(!WorkParams::default().passes_quality_gate());
    }

    #[test]
    fn publish_without_human_action_fails_gate() {
        let p = WorkParams {
            publish: Some(PublishParams {
                platform: "douyin".into(),
                account_profile: "p1".into(),
                title: "t".into(),
                body: "".into(),
                tags: vec![],
                media_paths: vec![],
                schedule: None,
                dry_run: true,
                confirm: false,
            }),
            ..Default::default()
        };
        assert!(
            !p.passes_quality_gate(),
            "发布类任务必须给出 next_human_actions"
        );
    }

    #[test]
    fn crawler_only_passes_gate() {
        let p = WorkParams {
            crawler: Some(CrawlerParams {
                platform: "xhs".into(),
                keywords: vec!["猫爬架".into()],
                enable_comments: true,
                max_notes_count: 20,
                save_option: "json".into(),
                readback_required: true,
            }),
            ..Default::default()
        };
        assert!(p.passes_quality_gate());
    }

    #[test]
    fn json_roundtrip() {
        let p = WorkParams {
            missing_formal_config: vec!["缺少抖音 cookie".into()],
            next_human_actions: vec!["人工审核视频".into()],
            ..Default::default()
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: WorkParams = serde_json::from_str(&s).unwrap();
        assert_eq!(back.missing_formal_config.len(), 1);
        assert_eq!(back.next_human_actions, vec!["人工审核视频".to_string()]);
    }
}
