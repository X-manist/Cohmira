//! 百度贴吧采集器（PC 签名纯算法 + CDP 浏览器 fetch 文档化占位 + HTML 关键字段解析）。
//!
//! 移植自 `MediaCrawler/media_platform/tieba/client.py`（`BaiduTieBaClient` 的签名与
//! 浏览器 fetch 逻辑）与 `MediaCrawler/media_platform/tieba/help.py`（`TieBaExtractor`
//! 的搜索 JSON / HTML 字段抽取）。
//!
//! # 移植边界
//! - **签名（完整移植）**：[`sign_pc_params`] 实现 PC API 的 `md5(排序后 k=v 拼接 + salt)`
//!   纯算法，与 Python `_sign_pc_params` 字节对齐（已有 golden 向量单测）。
//! - **浏览器 fetch（trait 占位）**：原 Python 通过 Playwright 在登录态浏览器内执行
//!   `fetch(url,{credentials:'include'})` 取回 JSON（`_fetch_json_by_browser`）。
//!   Rust 侧**不引入重依赖**（chromiumoxide/fantoccini），而是定义
//!   [`FetchViaBrowser`] trait 作为 CDP 注入点，由宿主（goose-server 浏览器子进程）
//!   注入实现；占位实现 [`CdpBrowser`] 标 `todo!()`。
//! - **HTML 解析（关键部分移植）**：用 `regex` 抽取帖子详情页的标题/正文/作者/IP/时间
//!   等字段（[`extract_title_from_html`] 等），以及搜索结果 JSON API 的归一化解析
//!   [`parse_search_note_list_from_api`]。
//!
//! # 与原 Python 的差异
//! - 签名入参为 `&[(String, String)]`（HTTP 参数天然就是字符串）；Python 里 int 值经
//!   `f"{v}"` 转 str，Rust 由调用方在构造参数时 stringify，结果一致。
//! - HTML 解析用正则替代 `parsel` 的 xpath（crate 已有 `regex`，避免引入 `scraper`）。
//! - 暂不移植评论/子评论/创作者主页等次要流程（聚焦 search + 详情关键字段）。

use crate::model::{Content, ContentType, Creator, Platform};
use md5::{Digest, Md5};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;

// ----------------------------------------------------------------------------
// 常量
// ----------------------------------------------------------------------------

/// 贴吧 PC 站点根。
pub const HOST: &str = "https://tieba.baidu.com";

/// PC API 签名盐（来自 `client.py::PC_SIGN_SECRET`，勿改）。
pub const PC_SIGN_SECRET: &str = "36770b1f34c9bbf2e7d1a99d2b82fa9e";

// ----------------------------------------------------------------------------
// 签名纯算法（help: client.py::_sign_pc_params 的 1:1 移植）
// ----------------------------------------------------------------------------

/// 计算贴吧 PC API 的 `sign`。
///
/// 算法（对齐 Python `_sign_pc_params`）：
/// 1. 按 key 字典序排序；
/// 2. 跳过 key 为 `sign`/`sig` 的项（None 值由调用方直接不放入即等价）；
/// 3. 拼接 `k=v`（不加分隔符）；
/// 4. 末尾追加 [`PC_SIGN_SECRET`]；
/// 5. 返回 `md5` 的 32 位小写 hex。
///
/// `params` 视为多重映射（保留重复 key，按出现位置稳定排序），与 dict 语义在对请求
/// 签名时等价（业务参数 key 唯一）。
///
/// # 示例
///
/// ```
/// use mediacrawler::platform::tieba::sign_pc_params;
/// // 对齐 Python：{"subapp_type":"pc","_client_type":"20"}
/// let p = vec![("_client_type".into(),"20".into()),("subapp_type".into(),"pc".into())];
/// assert_eq!(sign_pc_params(&p), "e9b101df871c39eedcf9a232c2d26ec8");
/// ```
pub fn sign_pc_params(params: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = params.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sign_text = String::new();
    for (k, v) in sorted {
        if k == "sign" || k == "sig" {
            continue;
        }
        sign_text.push_str(k);
        sign_text.push('=');
        sign_text.push_str(v);
    }
    sign_text.push_str(PC_SIGN_SECRET);

    let mut h = Md5::new();
    h.update(sign_text.as_bytes());
    hex::encode(h.finalize())
}

// ----------------------------------------------------------------------------
// FetchViaBrowser trait —— CDP 注入点（对应 _fetch_json_by_browser 的浏览器侧）
// ----------------------------------------------------------------------------

/// 浏览器内 `fetch` 的注入结果。
#[derive(Debug, Clone)]
pub struct BrowserFetch {
    /// HTTP 状态码。
    pub status: u16,
    /// 响应正文（文本）。
    pub text: String,
}

/// 在已登录百度账号的浏览器上下文里执行 `fetch` 的抽象。
///
/// 对应 Python `client.py::_fetch_json_by_browser` 中通过 Playwright `page.evaluate`
/// 注入的下述 JS：
/// ```js
/// async ({ url, method, body }) => {
///     const headers = { "Accept": "application/json, text/plain, */*" };
///     const options = { method, credentials: "include", headers };
///     if (method === "POST") {
///         headers["Content-Type"] = "application/x-www-form-urlencoded;charset=UTF-8";
///         options.body = body;
///     }
///     const resp = await fetch(url, options);
///     const text = await resp.text();
///     return { status: resp.status, text };
/// }
/// ```
///
/// Rust 侧不绑定具体浏览器驱动；宿主（goose-server 的 CDP 浏览器子进程）实现该 trait
/// 并通过 [`TiebaCrawler::with_browser`] 注入。实现需保证：
/// - 目标页 origin 已是 `https://tieba.baidu.com`（必要时先 navigate，对齐
///   `_ensure_tieba_origin`）；
/// - `credentials: "include"`，携带登录 cookie（BDUSS/STOKEN/PTOKEN）；
/// - POST 时按 `application/x-www-form-urlencoded` 发送 `body`。
#[async_trait::async_trait]
pub trait FetchViaBrowser: Send + Sync {
    /// 在浏览器内执行 `fetch`，回传 `({status, text})`。
    async fn fetch(
        &self,
        url: &str,
        method: reqwest::Method,
        body: Option<&str>,
    ) -> anyhow::Result<BrowserFetch>;
}

/// CDP 注入占位实现（未接线）。
///
/// 保留接口形状与文档；真正调用前由宿主替换为可用的 [`FetchViaBrowser`] 实现。
/// 直接调用会 panic（`todo!()`），与「未接线」语义一致。
pub struct CdpBrowser;

#[async_trait::async_trait]
impl FetchViaBrowser for CdpBrowser {
    async fn fetch(
        &self,
        _url: &str,
        _method: reqwest::Method,
        _body: Option<&str>,
    ) -> anyhow::Result<BrowserFetch> {
        todo!(
            "通过 CDP（chromiumoxide/fantoccini 或宿主浏览器子进程）在 tieba.baidu.com \
             页面注入 fetch 并回传 {{status,text}}"
        )
    }
}

// ----------------------------------------------------------------------------
// 采集器
// ----------------------------------------------------------------------------

/// 贴吧采集器。
///
/// 遵循 bilibili/weibo/kuaishou 模式：`build_*`/`parse_*` 为纯函数（可 fixture 单测），
/// `TiebaCrawler` 持有 HTTP client 与可选的浏览器注入。`search` 等强依赖登录态的 PC
/// API 必须注入 [`FetchViaBrowser`]；未注入时返回 `Err`（不 panic）。
pub struct TiebaCrawler {
    /// 普通 HTTP client（用于无需浏览器 cookie 的兜底请求；当前 PC API 主路径走浏览器）。
    http: reqwest::Client,
    /// 浏览器内 fetch 注入。`None` 表示未接线。
    browser: Option<Box<dyn FetchViaBrowser>>,
}

impl TiebaCrawler {
    /// 构造采集器（未注入浏览器，`search` 将返回 `Err`）。
    pub fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            browser: None,
        }
    }

    /// 注入浏览器 fetch 实现，使 PC API（search/详情/评论）可用。
    pub fn with_browser(mut self, browser: Box<dyn FetchViaBrowser>) -> Self {
        self.browser = Some(browser);
        self
    }

    /// 取底层 HTTP client 引用（供宿主做其它请求）。
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// 在浏览器上下文里请求贴吧 PC JSON API（对应 `_fetch_json_by_browser`）。
    ///
    /// - 自动过滤空值由调用方在构造 `params`/`data` 时完成（对齐 Python 的 `v is None`
    ///   过滤——字符串场景无 None，等价于不传）；
    /// - `use_sign=true` 时按 method 选择签名源（GET→params，POST→data），注入默认
    ///   `subapp_type=pc`/`_client_type=20`（对齐 `setdefault`），计算并追加 `sign`；
    /// - 校验 `status==200`、可解析 JSON、且 `error_code`/`no` ∈ {0, None}。
    pub async fn fetch_json_by_browser(
        &self,
        uri: &str,
        method: reqwest::Method,
        params: &[(String, String)],
        data: &[(String, String)],
        use_sign: bool,
    ) -> anyhow::Result<serde_json::Value> {
        let browser = self.browser.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "tieba PC API 需要浏览器上下文（FetchViaBrowser）；通过 \
                 TiebaCrawler::with_browser 注入"
            )
        })?;

        let mut params = params.to_vec();
        let mut data = data.to_vec();

        if use_sign {
            let is_post = method == reqwest::Method::POST;
            // 对齐 Python：sign_source = data if POST else params
            let sign_source = if is_post { &mut data } else { &mut params };
            ensure_default(sign_source, "subapp_type", "pc");
            ensure_default(sign_source, "_client_type", "20");
            let sig = sign_pc_params(sign_source);
            sign_source.push(("sign".to_string(), sig));
        }

        let mut url = format!("{HOST}{uri}");
        if !params.is_empty() {
            url.push('?');
            url.push_str(&urlencode(&params));
        }
        let body = if method == reqwest::Method::POST {
            Some(urlencode(&data))
        } else {
            None
        };

        let resp = browser.fetch(&url, method.clone(), body.as_deref()).await?;
        if resp.status != 200 {
            anyhow::bail!("Tieba PC API failed, status={}, url={}", resp.status, url);
        }
        let json: serde_json::Value = serde_json::from_str(&resp.text)
            .map_err(|e| anyhow::anyhow!("Tieba PC API returned non-JSON, url={url}, err={e}"))?;

        // error_code = json.error_code ?? json.no ?? 0；非 {"0","None"} 视为业务错误。
        let err_code = json.get("error_code").or_else(|| json.get("no"));
        let bad = match err_code {
            None | Some(serde_json::Value::Null) => false,
            Some(v) => {
                let s = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                !matches!(s.as_str(), "0" | "None")
            }
        };
        if bad {
            anyhow::bail!("Tieba PC API error, url={url}, response={json}");
        }
        Ok(json)
    }

    /// 获取发帖页数据（`/c/f/pb/page_pc` POST，对应 `_get_pc_page_data`）。
    pub async fn get_pc_page_data(
        &self,
        note_id: &str,
        page: i64,
        tbs: &str,
    ) -> anyhow::Result<serde_json::Value> {
        self.fetch_json_by_browser(
            "/c/f/pb/page_pc",
            reqwest::Method::POST,
            &[],
            &build_page_pc_params(note_id, page, tbs),
            true,
        )
        .await
    }
}

#[async_trait::async_trait]
impl crate::platform::PlatformCrawler for TiebaCrawler {
    fn name(&self) -> &'static str {
        "tieba"
    }

    async fn search(&self, keyword: &str, limit: usize) -> anyhow::Result<Vec<Content>> {
        // 对齐 Python get_notes_by_keyword：rn 取 max(limit,20)，第 1 页。
        let page_size = limit.max(20) as i64;
        let params = build_search_params(keyword, 1, page_size);
        let json = self
            .fetch_json_by_browser(
                "/mo/q/search/multsearch",
                reqwest::Method::GET,
                &params,
                &[],
                true,
            )
            .await?;
        let api: SearchApiResp = serde_json::from_value(json)?;
        let mut notes = parse_search_note_list_from_api(&api);
        notes.truncate(limit);
        Ok(notes)
    }
}

// ----------------------------------------------------------------------------
// 纯函数：请求参数构造
// ----------------------------------------------------------------------------

/// 构造关键词搜索（`/mo/q/search/multsearch`）的查询参数（未含 `sign`）。
///
/// 对齐 `client.py::get_notes_by_keyword` 的默认入参（`st=0` 时间倒序，
/// `note_type` 固定主帖）。`sign` 由 [`TiebaCrawler::fetch_json_by_browser`] 追加。
pub fn build_search_params(keyword: &str, page: i64, page_size: i64) -> Vec<(String, String)> {
    vec![
        ("rn".to_string(), page_size.to_string()),
        ("st".to_string(), "0".to_string()),
        ("word".to_string(), keyword.to_string()),
        ("needbrand".to_string(), "1".to_string()),
        ("sug_type".to_string(), "2".to_string()),
        ("pn".to_string(), page.to_string()),
        ("come_from".to_string(), "search".to_string()),
        ("subapp_type".to_string(), "pc".to_string()),
        ("_client_type".to_string(), "20".to_string()),
    ]
}

/// 构造帖子分页（`/c/f/pb/page_pc`）的 POST body（未含 `sign`）。
///
/// 对齐 `_get_pc_page_data`。
pub fn build_page_pc_params(note_id: &str, page: i64, tbs: &str) -> Vec<(String, String)> {
    vec![
        ("pn".to_string(), page.to_string()),
        ("lz".to_string(), "0".to_string()),
        ("r".to_string(), "2".to_string()),
        ("mark_type".to_string(), "0".to_string()),
        ("back".to_string(), "0".to_string()),
        ("fr".to_string(), String::new()),
        ("kz".to_string(), note_id.to_string()),
        ("session_request_times".to_string(), "1".to_string()),
        ("tbs".to_string(), tbs.to_string()),
        ("subapp_type".to_string(), "pc".to_string()),
        ("_client_type".to_string(), "20".to_string()),
    ]
}

// ----------------------------------------------------------------------------
// 纯函数：搜索 JSON API 解析（对应 help.py::extract_search_note_list_from_api）
// ----------------------------------------------------------------------------

/// 从 `multsearch` 响应解析归一化 [`Content`] 列表。
///
/// 对齐 `TieBaExtractor.extract_search_note_list_from_api`：仅取
/// `cardInfo=="thread"` 或 `cardStyle=="thread"` 的卡片，从 `card.data` 取
/// `tid/title/content/time/post_num/forum_name/user.*`。
pub fn parse_search_note_list_from_api(api: &SearchApiResp) -> Vec<Content> {
    api.data
        .card_list
        .iter()
        .filter(|c| {
            c.card_info.as_deref() == Some("thread") || c.card_style.as_deref() == Some("thread")
        })
        .filter_map(|c| c.data.as_ref())
        .filter(|item| !value_as_string(&item.tid).is_empty())
        .map(|item| {
            let note_id = value_as_string(&item.tid);
            let tieba_name = ensure_tieba_suffix(&item.forum_name);
            let user = item.user.as_ref();
            let nickname = user
                .and_then(|u| first_nonempty(&[&u.show_nickname, &u.user_name]))
                .unwrap_or_default()
                .to_string();
            let portrait = user
                .and_then(|u| first_nonempty(&[&u.portrait, &u.portraith]))
                .map(|p| p.to_string())
                .unwrap_or_default();

            Content {
                platform: Platform::Tieba,
                content_type: ContentType::Note,
                platform_id: note_id.clone(),
                url: Some(format!("{HOST}/p/{note_id}")),
                title: normalize_text(&item.title),
                desc: {
                    let d = normalize_text(&item.content);
                    if d.is_empty() {
                        None
                    } else {
                        Some(d)
                    }
                },
                author: if nickname.is_empty() && portrait.is_empty() {
                    None
                } else {
                    Some(Creator {
                        platform: Platform::Tieba,
                        platform_user_id: user.map(|u| value_as_string(&u.id)).unwrap_or_default(),
                        nickname,
                        avatar: if portrait.is_empty() {
                            None
                        } else {
                            Some(avatar_from_portrait(&portrait))
                        },
                        desc: None,
                        fans_count: None,
                        follows_count: None,
                        note_count: None,
                    })
                },
                published_at: item.time.or(item.create_time),
                liked_count: None,
                comment_count: item.post_num,
                collected_count: None,
                share_count: None,
                tags: if tieba_name.is_empty() {
                    Vec::new()
                } else {
                    vec![tieba_name]
                },
                media_urls: Vec::new(),
            }
        })
        .collect()
}

// ----------------------------------------------------------------------------
// 纯函数：HTML 字段抽取（对应 help.py::extract_note_detail 的关键正则路径）
// ----------------------------------------------------------------------------

// 注：workspace `regex` 关闭了 unicode 特性（default-features=false），故避免
// `\d`/`\s`/`\S` 等 Perl 类，改用显式字符类 `[0-9]` / `[ \t\n\r\f\v]`。
static RE_TITLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)<title[^>]*>([^<]*)</title>").unwrap());
static RE_DESC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<meta[^>]*name=["']description["'][^>]*content=["']([^"']*)["']"#).unwrap()
});
static RE_DESC_REV: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?s)<meta[^>]*content=["']([^"']*)["'][^>]*name=["']description["']"#).unwrap()
});
static RE_THREAD_ID: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#""thread_id"[ \t\n\r\f\v]*:[ \t\n\r\f\v]*"?([0-9]+)"?"#).unwrap());
static RE_LZONLY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?s)id=["']lzonly_cntn["'][^>]*href=["']([^"']+)["']"#).unwrap());
static RE_IP: Lazy<Regex> = Lazy::new(|| Regex::new(r"IP属地:([^ \t\n\r\f\v]+?)</span>").unwrap());
static RE_PUB_TIME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<span class="tail-info">([0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2})</span>"#)
        .unwrap()
});
static RE_NOTE_ID_IN_URL: Lazy<Regex> = Lazy::new(|| Regex::new(r"/p/([0-9]+)").unwrap());

/// 从帖子详情页 HTML 抽取 `<title>`（去掉 `_百度贴吧`/`_Baidu Tieba` 后缀，对齐
/// `_clean_title` 的核心清洗）。
pub fn extract_title_from_html(html: &str) -> String {
    let raw = RE_TITLE
        .captures(html)
        .map(|c| c.get(1).map(|m| m.as_str()).unwrap_or(""))
        .unwrap_or("");
    let normalized = normalize_text(raw);
    // 去掉 `_百度贴吧` / `_Baidu Tieba` 结尾
    let trimmed = normalized
        .trim_end_matches("_百度贴吧")
        .trim_end_matches("_Baidu Tieba")
        .trim();
    trimmed.to_string()
}

/// 从帖子详情页 HTML 抽取 `<meta name="description" content="...">`（兼容 content/name
/// 两种属性顺序）。
pub fn extract_desc_from_html(html: &str) -> String {
    let cap = RE_DESC
        .captures(html)
        .or_else(|| RE_DESC_REV.captures(html))
        .map(|c| c.get(1).map(|m| m.as_str()).unwrap_or(""))
        .unwrap_or("");
    normalize_text(&html_unescape(cap))
}

/// 从帖子详情页 HTML 抽取帖子 id：先取 `#lzonly_cntn` 的 href 末段，回退到
/// `"thread_id": <n>`（对齐 `extract_note_detail` 的取值顺序）。
pub fn extract_note_id_from_html(html: &str) -> String {
    if let Some(c) = RE_LZONLY.captures(html) {
        let href = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let id = href
            .split('?')
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("");
        if !id.is_empty() {
            return id.to_string();
        }
    }
    RE_THREAD_ID
        .captures(html)
        .map(|c| c.get(1).map(|m| m.as_str()).unwrap_or("").to_string())
        .unwrap_or_default()
}

/// 从 HTML 抽取 `IP属地:xxx`（对齐 `extract_ip`）。
pub fn extract_ip_from_html(html: &str) -> String {
    RE_IP
        .captures(html)
        .map(|c| c.get(1).map(|m| m.as_str()).unwrap_or("").to_string())
        .unwrap_or_default()
}

/// 从 HTML 抽取发布时间 `YYYY-MM-DD HH:MM`（对齐 `extract_ip_and_pub_time` 的时间分支）。
pub fn extract_pub_time_from_html(html: &str) -> String {
    RE_PUB_TIME
        .captures(html)
        .map(|c| c.get(1).map(|m| m.as_str()).unwrap_or("").to_string())
        .unwrap_or_default()
}

/// 从 URL 中抽取 `/p/<id>` 的 id（对齐 `_extract_note_id_from_url`）。
pub fn extract_note_id_from_url(url: &str) -> String {
    RE_NOTE_ID_IN_URL
        .captures(url)
        .map(|c| c.get(1).map(|m| m.as_str()).unwrap_or("").to_string())
        .unwrap_or_default()
}

// ----------------------------------------------------------------------------
// 纯函数：创作者 portrait 抽取（对应 _extract_creator_portrait）
// ----------------------------------------------------------------------------

/// 从创作者主页 URL 抽取 portrait（取 `id`/`portrait`/`un` 查询参数，对齐
/// `_extract_creator_portrait`）。
pub fn extract_creator_portrait(creator_url: &str) -> String {
    let url = creator_url.trim();
    if url.is_empty() {
        return String::new();
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        // 非绝对 URL：取 `?` 之前部分（对齐 Python 分支）
        return url.split('?').next().unwrap_or("").to_string();
    }
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return String::new(),
    };
    for key in ["id", "portrait", "un"] {
        if let Some(v) = parsed
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.to_string())
        {
            let decoded = url_decode(&v);
            return decoded.split('?').next().unwrap_or("").to_string();
        }
    }
    String::new()
}

// ----------------------------------------------------------------------------
// 辅助纯函数
// ----------------------------------------------------------------------------

/// 若 `v` 不含 key 为 `key` 的项，则追加 `(key, val)`（对齐 Python `setdefault`）。
fn ensure_default(v: &mut Vec<(String, String)>, key: &str, val: &str) {
    if !v.iter().any(|(k, _)| k == key) {
        v.push((key.to_string(), val.to_string()));
    }
}

/// 折叠多余空白（对齐 `_normalize_text`）。
fn normalize_text(s: &str) -> String {
    static RE_WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t\n\r\f\v]+").unwrap());
    RE_WS.replace_all(s, " ").trim().to_string()
}

/// 贴吧名补「吧」后缀（对齐 `_ensure_tieba_suffix`）。
pub fn ensure_tieba_suffix(name: &str) -> String {
    let n = name.trim();
    if n.is_empty() || n.ends_with('吧') {
        n.to_string()
    } else {
        format!("{n}吧")
    }
}

/// portrait → 头像 URL（对齐 `_api_user_avatar` 的 bdstatic 兜底）。
pub fn avatar_from_portrait(portrait: &str) -> String {
    if portrait.is_empty() {
        return String::new();
    }
    format!("https://gss0.bdstatic.com/6LZ1dD3d1sgCo2Kml5_Y_D3/sys/portrait/item/{portrait}")
}

/// 由贴吧名构造吧主页链接（对齐 `_tieba_link_from_name`，使用 `quote` 编码）。
pub fn tieba_link_from_name(name: &str) -> String {
    if name.is_empty() {
        return HOST.to_string();
    }
    let kw = name.trim_end_matches('吧');
    format!("{HOST}/f?kw={}", urlquote(kw))
}

/// `serde_json::Value` → 字符串（int/str 均按字面；对齐 Python `str(item.get("tid"))`）。
fn value_as_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// 返回第一个非空字符串切片（用于回退取值）。
fn first_nonempty<'a>(opts: &[&'a String]) -> Option<&'a str> {
    for o in opts {
        if !o.is_empty() {
            return Some(o.as_str());
        }
    }
    None
}

/// HTML 反转义（覆盖 `&amp; &lt; &gt; &quot; &#39; &nbsp;`，对齐 `html.unescape` 的常用子集）。
fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// percent-decode（覆盖 `%XX`，UTF-8 还原；用于 portrait 查询参数解码）。
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_digit(bytes[i + 1]), hex_digit(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `urllib.parse.urlencode`（quote_plus）风格的查询串编码（与 `signing::bilibili` 一致）。
fn urlencode(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// quote_plus：保留 `A-Za-z0-9 _.-~`，空格→`+`，其余→`%XX`（大写）。
fn quote_plus(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `urllib.parse.quote` 风格（空格→`%20`，非保留字符之外→`%XX`）。
fn urlquote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => {
                out.push(b as char)
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------
// 响应类型（仅取需要的字段，未知字段忽略）
// ----------------------------------------------------------------------------

/// `multsearch` 响应。
#[derive(Debug, Default, Deserialize)]
pub struct SearchApiResp {
    #[serde(default)]
    pub data: SearchApiData,
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchApiData {
    #[serde(default)]
    pub card_list: Vec<SearchCard>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchCard {
    /// `cardInfo`（Python 原字段名）。
    #[serde(default)]
    pub card_info: Option<String>,
    /// `cardStyle`。
    #[serde(default)]
    pub card_style: Option<String>,
    #[serde(default)]
    pub data: Option<SearchThreadItem>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchThreadItem {
    /// `tid`（可能为 int 或 string）。
    #[serde(default)]
    pub tid: serde_json::Value,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    /// unix 秒。
    #[serde(default)]
    pub time: Option<i64>,
    #[serde(default)]
    pub create_time: Option<i64>,
    #[serde(default)]
    pub post_num: Option<i64>,
    #[serde(default)]
    pub forum_name: String,
    #[serde(default)]
    pub user: Option<SearchUser>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchUser {
    /// 用户 id（可能为 int/string/缺省）。
    #[serde(default)]
    pub id: serde_json::Value,
    #[serde(default)]
    pub show_nickname: String,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub portrait: String,
    /// 部分接口返回的高清 portrait 字段。
    #[serde(default)]
    pub portraith: String,
}

// ----------------------------------------------------------------------------
// 单测
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- sign 纯算法（golden 向量来自 Python 源码实测）----

    #[test]
    fn sign_pc_params_matches_python_simple() {
        // Python: {"subapp_type":"pc","_client_type":"20"}
        // sorted sign_text = "_client_type=20subapp_type=pc" + secret
        let params = vec![
            ("subapp_type".to_string(), "pc".to_string()),
            ("_client_type".to_string(), "20".to_string()),
        ];
        assert_eq!(sign_pc_params(&params), "e9b101df871c39eedcf9a232c2d26ec8");
    }

    #[test]
    fn sign_pc_params_skips_sign_sig_and_none_like() {
        // Python: {"b":"2","a":"1","sign":"x","sig":"x","c":None}
        // → sorted 保留 a,b → "a=1b=2" + secret（sign/sig 被跳过，c=None 由调用方不放入）
        let params = vec![
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
            ("sign".to_string(), "shouldskip".to_string()),
            ("sig".to_string(), "shouldskip".to_string()),
        ];
        assert_eq!(sign_pc_params(&params), "8e110db5f145d175303e669ff036fced");
    }

    #[test]
    fn sign_pc_params_int_values_stringified_match_python() {
        // 对齐 case2：search GET 参数含 int（rn=20,st=0,...）
        // Python md5 = 08e3e710fd1099af997166833c4e25c8
        let params = vec![
            ("rn".to_string(), "20".to_string()),
            ("st".to_string(), "0".to_string()),
            ("word".to_string(), "test".to_string()),
            ("needbrand".to_string(), "1".to_string()),
            ("sug_type".to_string(), "2".to_string()),
            ("pn".to_string(), "1".to_string()),
            ("come_from".to_string(), "search".to_string()),
            ("subapp_type".to_string(), "pc".to_string()),
            ("_client_type".to_string(), "20".to_string()),
        ];
        assert_eq!(sign_pc_params(&params), "08e3e710fd1099af997166833c4e25c8");
    }

    // ---- FetchViaBrowser trait 定义存在（编译期断言）----

    #[test]
    fn fetch_via_browser_trait_is_implemented_by_placeholder() {
        // trait object 可被构造，证明 trait 定义存在且 CdpBrowser 实现了它。
        fn _accept(_: &dyn FetchViaBrowser) {}
        let b = CdpBrowser;
        _accept(&b);
        // 接口形状占位断言（不实际调用，避免触发 todo!()）
        let _: Option<Box<dyn FetchViaBrowser>> = None;
    }

    // ---- 搜索 JSON API 解析（fixture）----

    #[test]
    fn parse_search_api_maps_thread_cards() {
        let raw = r#"{
            "data": {
                "card_list": [
                    {"cardInfo": "banner"},
                    {
                        "cardInfo": "thread",
                        "data": {
                            "tid": 7700123456,
                            "title": "猫爬架选购心得分享",
                            "content": "家里两只猫，买的这款爬架很稳",
                            "time": 1700000000,
                            "post_num": 42,
                            "forum_name": "宠物",
                            "user": {
                                "id": 99,
                                "show_nickname": "猫奴日记",
                                "portrait": "abc.123"
                            }
                        }
                    },
                    {
                        "cardStyle": "thread",
                        "data": {
                            "tid": "7700999888",
                            "title": "狗粮推荐",
                            "content": "",
                            "create_time": 1700000100,
                            "post_num": 7,
                            "forum_name": "狗吧",
                            "user": {"user_name": "匿名"}
                        }
                    }
                ]
            }
        }"#;
        let api: SearchApiResp = serde_json::from_str(raw).unwrap();
        let items = parse_search_note_list_from_api(&api);
        assert_eq!(items.len(), 2);

        let a = &items[0];
        assert_eq!(a.platform, Platform::Tieba);
        assert_eq!(a.content_type, ContentType::Note);
        assert_eq!(a.platform_id, "7700123456"); // int tid → 字符串
        assert_eq!(
            a.url.as_deref(),
            Some("https://tieba.baidu.com/p/7700123456")
        );
        assert_eq!(a.title, "猫爬架选购心得分享");
        assert_eq!(a.desc.as_deref(), Some("家里两只猫，买的这款爬架很稳"));
        assert_eq!(a.published_at, Some(1700000000));
        assert_eq!(a.comment_count, Some(42));
        assert_eq!(a.tags, vec!["宠物吧"]); // 吧后缀
        let author = a.author.as_ref().unwrap();
        assert_eq!(author.nickname, "猫奴日记");
        assert_eq!(author.platform_user_id, "99");
        assert!(author.avatar.as_deref().unwrap().ends_with("abc.123"));

        let b = &items[1];
        assert_eq!(b.platform_id, "7700999888"); // string tid 原样
        assert_eq!(b.title, "狗粮推荐");
        assert!(b.desc.is_none()); // 空 content → None
        assert_eq!(b.published_at, Some(1700000100)); // 走 create_time 回退
        assert_eq!(b.tags, vec!["狗吧"]); // 已有「吧」不重复
        assert_eq!(b.author.as_ref().unwrap().nickname, "匿名");
    }

    #[test]
    fn parse_search_api_empty_returns_empty() {
        let api: SearchApiResp = serde_json::from_str(r#"{"data":{}}"#).unwrap();
        assert!(parse_search_note_list_from_api(&api).is_empty());
    }

    // ---- HTML 关键字段抽取（fixture）----

    #[test]
    fn extract_html_title_desc_id_ip_time() {
        let html = r#"<!DOCTYPE html><html><head>
            <title>猫爬架选购心得_百度贴吧</title>
            <meta name="description" content="家里两只猫，买的这款爬架很稳 &amp; 好用">
            </head><body>
            <a id="lzonly_cntn" href="/p/7700123456?see_lz=1">只看楼主</a>
            <div class="post-tail-wrap">
                <span class="tail-info">IP属地:北京</span>
                <span class="tail-info">2024-01-15 09:30</span>
            </div>
            <script>var PageData = {thread_id: 7700123456};</script>
            </body></html>"#;

        assert_eq!(extract_title_from_html(html), "猫爬架选购心得"); // 去掉 _百度贴吧
        assert_eq!(
            extract_desc_from_html(html),
            "家里两只猫，买的这款爬架很稳 & 好用" // &amp; → &
        );
        assert_eq!(extract_note_id_from_html(html), "7700123456"); // 来自 lzonly_cntn href
        assert_eq!(extract_ip_from_html(html), "北京");
        assert_eq!(extract_pub_time_from_html(html), "2024-01-15 09:30");
    }

    #[test]
    fn extract_note_id_falls_back_to_thread_id_json() {
        let html = r#"<script>PageData = {"thread_id": 123459876}</script>"#;
        assert_eq!(extract_note_id_from_html(html), "123459876");
    }

    #[test]
    fn extract_note_id_from_url_works() {
        assert_eq!(
            extract_note_id_from_url("https://tieba.baidu.com/p/7700123456"),
            "7700123456"
        );
        assert_eq!(extract_note_id_from_url("/p/99"), "99");
        assert_eq!(extract_note_id_from_url("https://tieba.baidu.com/"), "");
    }

    // ---- 创作者 portrait 抽取 ----

    #[test]
    fn extract_creator_portrait_from_query() {
        assert_eq!(
            extract_creator_portrait("https://tieba.baidu.com/home/main?id=abc.123&un=x"),
            "abc.123"
        );
        assert_eq!(
            extract_creator_portrait("https://tieba.baidu.com/home/main?portrait=xyz%2E456"),
            "xyz.456" // percent-decode
        );
        // 非绝对 URL：取 ? 之前
        assert_eq!(extract_creator_portrait("someportrait?x=1"), "someportrait");
        assert_eq!(extract_creator_portrait(""), "");
    }

    // ---- 辅助函数 ----

    #[test]
    fn ensure_tieba_suffix_adds_or_keeps() {
        assert_eq!(ensure_tieba_suffix("宠物"), "宠物吧");
        assert_eq!(ensure_tieba_suffix("狗吧"), "狗吧");
        assert_eq!(ensure_tieba_suffix(""), "");
    }

    #[test]
    fn urlquote_and_quote_plus_encode_correctly() {
        assert_eq!(urlquote("宠物"), "%E5%AE%A0%E7%89%A9");
        assert_eq!(quote_plus("a b"), "a+b");
        assert_eq!(quote_plus("a/b"), "a%2Fb");
    }

    #[test]
    fn build_search_params_shape() {
        let p = build_search_params("猫爬架", 2, 50);
        // 不含 sign（由 fetch_json_by_browser 追加）
        assert!(!p.iter().any(|(k, _)| k == "sign"));
        let map: std::collections::HashMap<&str, &str> =
            p.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(map["word"], "猫爬架");
        assert_eq!(map["rn"], "50");
        assert_eq!(map["pn"], "2");
        assert_eq!(map["subapp_type"], "pc");
        assert_eq!(map["_client_type"], "20");
    }
}
