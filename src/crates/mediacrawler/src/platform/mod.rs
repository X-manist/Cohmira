//! 平台采集抽象与各平台实现。
//!
//! v1 范围：纯 HTTP + cookie 可采集的平台。
//! - [`bilibili`]：wbi 签名搜索（纯 HTTP）。
//! - 强依赖浏览器渲染/签名逆向的平台（xhs JS 签名、douyin/zhihu、tieba）见各模块文档的降级策略。

pub mod bilibili;
pub mod douyin;
pub mod kuaishou;
pub mod tieba;
pub mod weibo;
pub mod xhs;
pub mod zhihu;

use crate::model::Content;

/// 平台采集器统一接口。
#[async_trait::async_trait]
pub trait PlatformCrawler: Send + Sync {
    /// 平台的人类可读名。
    fn name(&self) -> &'static str;

    /// 关键词搜索。`keyword` 为搜索词，`limit` 为期望结果条数（小样本，合规优先）。
    async fn search(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>>;
}

/// 已支持的采集平台工厂。
pub fn supported() -> Vec<&'static str> {
    vec!["xhs", "douyin", "bilibili"]
}
