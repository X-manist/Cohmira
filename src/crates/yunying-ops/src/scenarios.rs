//! 端到端模拟用户任务（对齐 docs/17 §7 的 8 个真实风格任务）。
//!
//! 每个任务代表一类运营需求，附带：
//! - 自然语言请求（live 模式下发给 Goose，模型应自主调用 operations 工具）；
//! - 期望触达的能力段（用于断言模型没有空跑）；
//! - 一份代表性的合格 [`WorkParams`]（fixture 模式下用于验证质量门与干跑规划）。
//!
//! ## 两种运行模式
//!
//! - **fixture 模式**（无需 LLM，CI 可跑）：用预置 `work_params` 跑质量门 + 干跑规划，证明流水线契约成立。
//! - **live 模式**（需 config.json 真实 key）：把 `user_request` 发给嵌入式 Goose，断言模型自主产出
//!   通过质量门的 `WorkParams` 并调用了期望能力段对应的 operations 工具（对齐 docs/17 §1 验收）。

use crate::work_params::{AssetGenParams, CrawlerParams, PublishParams, ReportParams, WorkParams};

/// 一个模拟用户任务。
#[derive(Debug, Clone)]
pub struct Scenario {
    pub id: &'static str,
    /// 模拟需求（自然语言）。
    pub user_request: &'static str,
    /// 期望触达的能力段（crawler/report/image/video/publish）。
    pub expected: &'static [&'static str],
    /// 合格模型应产出的代表性 workParams。
    pub work_params: WorkParams,
}

/// 8 个内置模拟任务（与 docs/17 §7 一致）。
pub fn all() -> Vec<Scenario> {
    vec![
        Scenario {
            id: "baby_hotspots",
            user_request: "帮我调研最近母婴产品的热点选题，重点小红书",
            expected: &["crawler", "report"],
            work_params: wp_crawler_report("xhs", &["母婴", "婴儿床"], "热点分析报告"),
        },
        Scenario {
            id: "cat_tree_launch",
            user_request: "猫爬架要做一个小红书种草推广，先调研再配图再出图文发布参数",
            expected: &["crawler", "image", "publish"],
            work_params: {
                let mut p = wp_crawler_report("xhs", &["猫爬架"], "猫爬架调研");
                p.image = Some(AssetGenParams {
                    model: "gpt-image-2".into(),
                    prompt: "木质猫爬架，自然光，产品广告风格，3:4".into(),
                    aspect_ratio: "3:4".into(),
                    count: 3,
                    duration_seconds: None,
                    target_path: "media/generated/cat-tree-{n}.png".into(),
                });
                p.publish = Some(pub_note("xiaohongshu", "xhs_01", "猫爬架种草"));
                p
            },
        },
        Scenario {
            id: "crib_creator_library",
            user_request: "建一个婴儿床方向的 B 站 UP 主博主库和素材库",
            expected: &["crawler", "report"],
            work_params: wp_crawler_report("bili", &["婴儿床", "母婴"], "博主库报告"),
        },
        Scenario {
            id: "cat_tree_video",
            user_request: "给猫爬架做一条抖音投流短视频，要视频参数和抖音发布参数",
            expected: &["video", "publish"],
            work_params: WorkParams {
                video: Some(AssetGenParams {
                    model: "doubao-seedance-1.5-pro".into(),
                    prompt: "木质猫爬架，干净桌面展示，自然光，9:16，8秒720p".into(),
                    aspect_ratio: "9:16".into(),
                    count: 1,
                    duration_seconds: Some(8),
                    target_path: "media/generated/cat-tree.mp4".into(),
                }),
                publish: Some(pub_video("douyin", "dy_01", "猫爬架投流视频")),
                next_human_actions: vec!["人工审核视频后用小号试发布".into()],
                ..Default::default()
            },
        },
        Scenario {
            id: "baby_food_batch_publish",
            user_request: "辅食机做一周小红书和抖音的批量发布日历",
            expected: &["publish", "report"],
            work_params: WorkParams {
                publish: Some(pub_note("douyin", "dy_01", "辅食机一周计划")),
                report: Some(ReportParams {
                    note_path: "辅食机一周发布日历".into(),
                    sections: vec!["日历".into(), "每日脚本".into(), "发布命令".into()],
                    sink_content: vec![],
                }),
                next_human_actions: vec!["人工审核每日脚本后排期发布".into()],
                ..Default::default()
            },
        },
        Scenario {
            id: "food_review_trend",
            user_request: "调研美食点评类爆款短视频趋势，重点是抖音",
            expected: &["crawler", "report"],
            work_params: wp_crawler_report("dy", &["探店", "美食"], "美食点评趋势报告"),
        },
        Scenario {
            id: "crib_xhs_asset_research",
            user_request: "调研小红书婴儿床爆款图文素材，并配图生成素材研究报告",
            expected: &["crawler", "image", "report"],
            work_params: {
                let mut p = wp_crawler_report("xhs", &["婴儿床", "图文"], "素材研究报告");
                p.image = Some(AssetGenParams {
                    model: "gpt-image-2".into(),
                    prompt: "婴儿房婴儿床场景，温馨自然光，3:4".into(),
                    aspect_ratio: "3:4".into(),
                    count: 4,
                    duration_seconds: None,
                    target_path: "media/generated/crib-{n}.png".into(),
                });
                p
            },
        },
        Scenario {
            id: "restaurant_review_batch_video",
            user_request: "本地餐饮探店号试发布：先抖音调研，再生成视频，再试发布计划",
            expected: &["crawler", "video", "publish"],
            work_params: {
                let mut p = wp_crawler_report("dy", &["探店", "本地餐饮"], "探店调研");
                p.video = Some(AssetGenParams {
                    model: "doubao-seedance-1.5-pro".into(),
                    prompt: "本地餐厅探店，9:16，8秒720p".into(),
                    aspect_ratio: "9:16".into(),
                    count: 1,
                    duration_seconds: Some(8),
                    target_path: "media/generated/restaurant.mp4".into(),
                });
                p.publish = Some(pub_video("douyin", "dy_test", "探店试发布"));
                p
            },
        },
    ]
}

/// 干跑规划：把 [`WorkParams`] 翻译为一组「计划操作」（不执行），用于 e2e 断言与人工复核。
///
/// 每条计划操作对应一个 operations 工具调用（dry_run）。真实执行需 `confirm=true` 且
/// 通过账号 check / 素材存在性 / 人工审核（见 docs/17 §6）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedOp {
    StartCrawler {
        platform: String,
        keywords: Vec<String>,
    },
    GenerateImage {
        model: String,
        count: u32,
    },
    GenerateVideo {
        model: String,
        seconds: u32,
    },
    UploadNote {
        platform: String,
        account: String,
    },
    UploadVideo {
        platform: String,
        account: String,
    },
    CreateNote {
        title: String,
    },
}

/// 生成干跑计划操作列表。
pub fn plan(wp: &WorkParams) -> Vec<PlannedOp> {
    let mut ops = Vec::new();
    if let Some(c) = &wp.crawler {
        ops.push(PlannedOp::StartCrawler {
            platform: c.platform.clone(),
            keywords: c.keywords.clone(),
        });
    }
    if let Some(i) = &wp.image {
        ops.push(PlannedOp::GenerateImage {
            model: i.model.clone(),
            count: i.count,
        });
    }
    if let Some(v) = &wp.video {
        ops.push(PlannedOp::GenerateVideo {
            model: v.model.clone(),
            seconds: v.duration_seconds.unwrap_or(8),
        });
    }
    if let Some(p) = &wp.publish {
        let platform = p.platform.clone();
        let account = p.account_profile.clone();
        // 简化：有 video 段视为视频发布，否则图文。
        if wp.video.is_some() {
            ops.push(PlannedOp::UploadVideo { platform, account });
        } else {
            ops.push(PlannedOp::UploadNote { platform, account });
        }
    }
    if let Some(r) = &wp.report {
        ops.push(PlannedOp::CreateNote {
            title: r.note_path.clone(),
        });
    }
    ops
}

fn wp_crawler_report(platform: &str, keywords: &[&str], report_title: &str) -> WorkParams {
    WorkParams {
        crawler: Some(CrawlerParams {
            platform: platform.into(),
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            enable_comments: true,
            max_notes_count: 20,
            save_option: "json".into(),
            readback_required: true,
        }),
        report: Some(ReportParams {
            note_path: report_title.into(),
            sections: vec!["摘要".into(), "结论".into(), "下一步".into()],
            sink_content: vec![],
        }),
        next_human_actions: vec!["人工复核采集结果与报告".into()],
        ..Default::default()
    }
}

fn pub_note(platform: &str, account: &str, title: &str) -> PublishParams {
    PublishParams {
        platform: platform.into(),
        account_profile: account.into(),
        title: title.into(),
        body: "".into(),
        tags: vec!["测试".into()],
        media_paths: vec!["media/generated/demo.png".into()],
        schedule: None,
        dry_run: true,
        confirm: false,
    }
}

fn pub_video(platform: &str, account: &str, title: &str) -> PublishParams {
    PublishParams {
        platform: platform.into(),
        account_profile: account.into(),
        title: title.into(),
        body: "".into(),
        tags: vec!["测试".into()],
        media_paths: vec!["media/generated/demo.mp4".into()],
        schedule: None,
        dry_run: true,
        confirm: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_eight_scenarios_present() {
        let s = all();
        assert_eq!(s.len(), 8, "应有 8 个模拟任务");
        let ids: Vec<_> = s.iter().map(|x| x.id).collect();
        for expected in [
            "baby_hotspots",
            "cat_tree_launch",
            "crib_creator_library",
            "cat_tree_video",
            "baby_food_batch_publish",
            "food_review_trend",
            "crib_xhs_asset_research",
            "restaurant_review_batch_video",
        ] {
            assert!(ids.contains(&expected), "缺少任务 {expected}");
        }
    }

    #[test]
    fn every_scenario_passes_quality_gate() {
        for s in all() {
            assert!(
                s.work_params.passes_quality_gate(),
                "任务 {} 的 workParams 未通过质量门",
                s.id
            );
        }
    }

    #[test]
    fn plan_covers_expected_capabilities() {
        for s in all() {
            let ops = plan(&s.work_params);
            assert!(!ops.is_empty(), "任务 {} 干跑计划为空", s.id);
            // 期望的能力段都应有对应计划操作。
            for cap in s.expected {
                let covered = match *cap {
                    "crawler" => ops
                        .iter()
                        .any(|o| matches!(o, PlannedOp::StartCrawler { .. })),
                    "image" => ops
                        .iter()
                        .any(|o| matches!(o, PlannedOp::GenerateImage { .. })),
                    "video" => ops
                        .iter()
                        .any(|o| matches!(o, PlannedOp::GenerateVideo { .. })),
                    "publish" => ops.iter().any(|o| {
                        matches!(
                            o,
                            PlannedOp::UploadNote { .. } | PlannedOp::UploadVideo { .. }
                        )
                    }),
                    "report" => ops
                        .iter()
                        .any(|o| matches!(o, PlannedOp::CreateNote { .. })),
                    other => panic!("未知能力段 {other}"),
                };
                assert!(covered, "任务 {} 期望能力段 {} 未被计划覆盖", s.id, cap);
            }
        }
    }

    #[test]
    fn cat_tree_launch_plans_crawler_image_note() {
        let s = all()
            .into_iter()
            .find(|s| s.id == "cat_tree_launch")
            .unwrap();
        let ops = plan(&s.work_params);
        assert!(ops
            .iter()
            .any(|o| matches!(o, PlannedOp::StartCrawler { .. })));
        assert!(ops
            .iter()
            .any(|o| matches!(o, PlannedOp::GenerateImage { .. })));
        assert!(ops
            .iter()
            .any(|o| matches!(o, PlannedOp::UploadNote { .. })));
    }
}
