//! 抖音 `a_bogus` 签名（JS 引擎跑原 `douyin.js`）。
//!
//! 移植自 `MediaCrawler/media_platform/douyin/help.py` 的 `get_a_bogus_from_js`。
//! 通过 [`crate::signing::js_engine::JsEngine`]（boa_engine 封装）加载 `douyin.js`，
//! 调用其中导出的 `sign_datail` / `sign_reply` 计算 `a_bogus` 参数。
//!
//! ## JS 函数选择规则（1:1 对齐 `help.py`）
//!
//! - URI 含 `/reply`（评论回复接口 `/aweme/v1/web/comment/list/reply/`）→ `sign_reply(params, ua)`，
//!   JS 内部 arguments 为 `[0, 1, 8]`。
//! - 其余（搜索、视频详情、评论列表、用户主页等）→ `sign_datail(params, ua)`，arguments `[0, 1, 14]`。
//!
//! ## boa 兼容性
//!
//! `douyin.js` 为纯计算型（RC4 + SM3 变种 + 自定义位运算），仅依赖
//! `encodeURIComponent` / `String.fromCharCode` / `Array.prototype.forEach.call` /
//! `console.error`（仅在非法分支触发），无 `window`/`navigator`/`document`/
//! `localStorage`/`setTimeout`/`Buffer` 等宿主对象，理论上可在 boa_engine 下运行。
//! 若实际运行中 boa 抛错（如混淆代码用到 boa 未支持的语法），
//! [`DouyinSign::get_a_bogus`] 直接返回 `anyhow::Error`，由调用方降级——**不硬编码假签名**。
//!
//! 注：[`crate::signing::js_engine::JsEngine::call`] 为 `&mut self`（boa `Context` 非多线程共享），
//! 故 [`DouyinSign::get_a_bogus`] 亦为 `&mut self`，调用方需自行加锁或独占持有。

use crate::signing::js_engine::JsEngine;

/// 抖音 `douyin.js` 源码（由主控放置于本模块同目录 `signing/douyin.js`）。
const DOUYIN_JS: &str = include_str!("douyin.js");

/// 详情/搜索/评论列表/用户主页等通用接口使用的 JS 签名函数名。
pub const SIGN_FN_DETAIL: &str = "sign_datail";
/// 评论回复接口（`/comment/list/reply/`）使用的 JS 签名函数名。
pub const SIGN_FN_REPLY: &str = "sign_reply";

/// 抖音 a_bogus 签名器。
///
/// 注：boa_engine 0.20 的 `Context` 用 `Rc`（非 `Send`）。为让持有本类型的采集器满足
/// `Send + Sync`，本结构**不持有** `JsEngine`，而是在 [`Self::get_a_bogus`] 每次调用时
/// 内部创建 `JsEngine` 并加载 `douyin.js`。JsEngine 只存活于该同步栈帧（boa eval 为同步、
/// 无 await），因此调用方的 async future 不会捕获它，保持 `Send`。代价是每次签名重新解析
/// JS（约 ms 级），对合规小样本采集可接受。
pub struct DouyinSign;

impl DouyinSign {
    /// 创建签名器（无状态）。
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }

    /// 计算 `a_bogus`。移植自 `help.py::get_a_bogus_from_js`。
    ///
    /// - `uri`：请求路径，用于判定走 `sign_datail` 还是 `sign_reply`（规则见模块文档）。
    /// - `query_params`：**已 urlencode 的完整查询串**（含 common params、msToken 等），
    ///   对应 Python 的 `urllib.parse.urlencode(params)`，而非裸 dict。
    /// - `user_agent`：HTTP `User-Agent` 头值（参与签名指纹）。
    ///
    /// 返回值为 JS 端 `result_encrypt(...) + "="` 产出的字符串。
    pub fn get_a_bogus(
        &mut self,
        uri: &str,
        query_params: &str,
        user_agent: &str,
    ) -> anyhow::Result<String> {
        let mut engine = JsEngine::new()?;
        engine.load(DOUYIN_JS)?;
        let fn_name = select_sign_fn(uri);
        let raw = engine.call(fn_name, &[query_params.to_string(), user_agent.to_string()])?;
        Ok(normalize_a_bogus(raw))
    }
}

/// 按 URI 选择 JS 签名函数名（纯函数，便于单测）。
///
/// 规则：`uri` 含 `/reply` → [`SIGN_FN_REPLY`]，否则 [`SIGN_FN_DETAIL`]。
pub fn select_sign_fn(uri: &str) -> &'static str {
    if uri.contains("/reply") {
        SIGN_FN_REPLY
    } else {
        SIGN_FN_DETAIL
    }
}

/// 清理 boa 返回串：去前后空白与可能的外层引号（`to_std_string_escaped` 偶尔会包裹引号）。
fn normalize_a_bogus(raw: String) -> String {
    raw.trim().trim_matches('"').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认 UA（与 platform::douyin 的默认 UA 一致，Mac Chrome 125）。
    const TEST_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
            AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

    #[test]
    fn select_sign_fn_dispatches_on_reply_subpath() {
        // 搜索 / 详情 / 评论列表 / 用户主页 → sign_datail
        assert_eq!(
            select_sign_fn("/aweme/v1/web/general/search/single/"),
            SIGN_FN_DETAIL
        );
        assert_eq!(
            select_sign_fn("/aweme/v1/web/aweme/detail/"),
            SIGN_FN_DETAIL
        );
        assert_eq!(
            select_sign_fn("/aweme/v1/web/comment/list/"),
            SIGN_FN_DETAIL
        );
        assert_eq!(
            select_sign_fn("/aweme/v1/web/user/profile/other/"),
            SIGN_FN_DETAIL
        );
        assert_eq!(select_sign_fn("/aweme/v1/web/aweme/post/"), SIGN_FN_DETAIL);
        // 评论回复 → sign_reply
        assert_eq!(
            select_sign_fn("/aweme/v1/web/comment/list/reply/"),
            SIGN_FN_REPLY
        );
    }

    #[test]
    fn a_bogus_call_path_does_not_panic_or_skips_without_boa() {
        // a_bogus 调用路径：若测试环境无可用 boa JsEngine（new/load 返回 Err），则跳过；
        // 若可用，断言对搜索接口的调用成功（不 panic、不返回 Err、结果非空）。
        let Ok(mut signer) = DouyinSign::new() else {
            return;
        };
        let params = "device_platform=webapp&aid=6383&keyword=rust&msToken=fake";
        let res = signer.get_a_bogus("/aweme/v1/web/general/search/single/", params, TEST_UA);
        assert!(res.is_ok(), "a_bogus engine call failed: {:?}", res.err());
        let ab = res.unwrap();
        assert!(!ab.is_empty(), "a_bogus 不应为空");
    }

    #[test]
    fn normalize_strips_quotes_and_whitespace() {
        assert_eq!(normalize_a_bogus("  \"abc=\"  ".to_string()), "abc=");
        assert_eq!(normalize_a_bogus("DSaBqX...=".to_string()), "DSaBqX...=");
    }
}
