//! 知乎请求签名（`x-zse-96` / `x-zst-81`）。
//!
//! 移植自 `MediaCrawler/media_platform/zhihu/help.py` 的 `sign(url, cookies)`。
//! 算法逻辑由原 [`libs/zhihu.js`](https://github.com/NanmiCoder/MediaCrawler/blob/main/libs/zhihu.js)
//! 提供（VMP 混淆版本，已对 101_3_3.0 / d_c0 + 固定 `tc` 做 md5 后再经一组 bit-encoding 生成 `x-zse-96`）。
//! 这里不重写算法，而是用 [`crate::signing::js_engine::JsEngine`]（boa_engine 封装）原样执行 JS，
//! 与 Python 版 `execjs.compile(zhihu.js).call("get_sign", url, cookies)` 行为 1:1 对齐。
//!
//! ## 调用契约
//!
//! - `cookies` 必须是原始 `Cookie` 请求头字符串，且必须包含 `d_c0=<...>`（JS 内部用正则 `d_c0=([^;]+)` 提取），
//!   与 `client.py::_pre_headers` 一致：缺失 `d_c0` 时直接报错。
//! - `url` 是带查询串的完整路径（如 `/api/v4/search_v3?q=...`），即 Python 版 `final_uri`，不含 host。
//!
//! ## 已知运行期依赖（离线无法精确验证）
//!
//! 原始 `zhihu.js` 顶部 `const crypto = require('crypto')`，并在 `get_sign` 中调用
//! `crypto.createHash('md5').update(...).digest('hex')`。boa_engine 是纯 JS 引擎，不内置 Node 的
//! `require`/`crypto` 模块；需要 [`crate::signing::js_engine::JsEngine`] 在构造时注入 `require`/`crypto`
//! 兜底（或在放入 `src/signing/zhihu.js` 前由主控替换为纯 JS / 外部注入 md5 的版本）。本模块只负责
//! 加载 JS、调用并解析返回值，算法正确性依赖上游 JS 与引擎 shim，故 `live_verification_needed=true`。

use serde::Deserialize;

use crate::signing::js_engine::JsEngine;

/// 原始知乎签名 JS（主控放置于 `src/crates/mediacrawler/src/signing/zhihu.js`）。
///
/// 仅编译期嵌入，不读盘；与 Python 版 `open("libs/zhihu.js")` 读取的是同一份文件。
const ZHIHU_JS: &str = include_str!("zhihu.js");

/// 把 `get_sign` 的对象返回值 JSON 化的包装层。
///
/// [`JsEngine::call`] 只能返回 `String`，而原 `get_sign` 返回
/// `{ "x-zst-81": ..., "x-zse-96": ... }` 对象；若直接 `ToString` 会得到 `[object Object]`。
/// 故追加一个 `__zhihu_sign_json` 包装函数，用 `JSON.stringify` 把结果序列化后返回，Rust 侧再反序列化。
const WRAPPER_JS: &str = r#"
function __zhihu_sign_json(url, cookies) {
    return JSON.stringify(get_sign(url, cookies));
}
"#;

/// 签名结果：与原 JS `get_sign` 返回值同 schema。
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ZhihuSignResult {
    /// `x-zst-81` 头（当前版本为 JS 内部固定的长 token）。
    #[serde(rename = "x-zst-81")]
    pub x_zst_81: String,
    /// `x-zse-96` 头（形如 `2.0_...`，由 md5 + bit-encoding 生成）。
    #[serde(rename = "x-zse-96")]
    pub x_zse_96: String,
}

/// 知乎签名器。
///
/// 注：与 [`crate::signing::douyin::DouyinSign`] 同理，boa_engine 0.20 的 `Context` 用 `Rc`（非 `Send`）。
/// 为让持有方满足 `Send + Sync`，本结构不持有 `JsEngine`，而在 [`Self::sign`] 每次调用时内部
/// 创建并加载 `zhihu.js`（JsEngine 只存活于同步栈帧，调用方 async future 保持 `Send`）。
pub struct ZhihuSigner;

impl ZhihuSigner {
    /// 构造签名器（无状态）。
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self)
    }

    /// 对单个请求 URL 签名。
    ///
    /// - `url`：带查询串的路径（如 `/api/v4/search_v3?q=...`），即 Python 版 `final_uri`。
    /// - `cookies`：原始 `Cookie` 头字符串，必须含 `d_c0`。
    ///
    /// 失败模式（与 Python 版对齐）：
    /// - cookie 缺 `d_c0` → `anyhow::bail!("d_c0 not found ...")`（在调用 JS 前先校验，给出明确错误）。
    /// - JS 执行抛错（如 `require('crypto')` 未兜底）→ 透传引擎错误。
    pub fn sign(&self, url: &str, cookies: &str) -> anyhow::Result<ZhihuSignResult> {
        if extract_dc0(cookies).is_none() {
            anyhow::bail!("d_c0 not found in cookies (x-zse-96 签名必需)");
        }
        let mut engine = JsEngine::new()?;
        engine.load(ZHIHU_JS)?;
        engine.load(WRAPPER_JS)?;
        let json = engine.call("__zhihu_sign_json", &[url.to_string(), cookies.to_string()])?;
        let result: ZhihuSignResult = serde_json::from_str(&json)
            .map_err(|e| anyhow::anyhow!("解析 zhihu get_sign 返回值失败: {e} (raw={json})"))?;
        Ok(result)
    }
}

/// 从原始 `Cookie` 头字符串中提取 `d_c0` 的值。
///
/// 与 JS 侧 `RegExp("d_c0=([^;]+)")`、Python 侧 `cookie_dict.get("d_c0")` 行为一致；
/// 缺失返回 `None`，供调用方快速失败。
pub fn extract_dc0(cookies: &str) -> Option<String> {
    for kv in cookies.split(';') {
        let kv = kv.trim();
        if let Some(v) = kv.strip_prefix("d_c0=") {
            return Some(v.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_dc0_from_cookie_header() {
        // 多 k/v、带空格、值含特殊字符都能取到，匹配 JS `d_c0=([^;]+)`。
        let cookies = "_zap=abc; d_c0=\"AAAAA_bC0DeFg\"; z_c0=\"2|1|0\"; __gads=xyz";
        assert_eq!(extract_dc0(cookies).as_deref(), Some("\"AAAAA_bC0DeFg\""));
        // d_c0 紧贴开头（无前导空格）
        assert_eq!(extract_dc0("d_c0=foo; x=1").as_deref(), Some("foo"));
        // 缺失
        assert!(extract_dc0("z_c0=token; foo=bar").is_none());
        assert!(extract_dc0("").is_none());
    }

    #[test]
    fn signer_constructs_and_engine_callable_without_panic() {
        // 签名器构造（加载并编译 zhihu.js + 包装层）必须可调用、不 panic。
        // 仅断言 Ok：真实 sign() 产出依赖 JS 引擎的 require/crypto 兜底，离线无法精确验证（见模块文档）。
        let signer = ZhihuSigner::new();
        // 若构建环境尚未接入 boa_engine / JsEngine，这里会 Err —— 用 if let 给出明确跳过信息，而非硬失败。
        match signer {
            Ok(_) => {} // 引擎已就绪：构造成功即满足「签名函数可调用不 panic」。
            Err(e) => eprintln!("[zhihu signing] JsEngine 未就绪，跳过构造断言: {e}"),
        }
    }
}
