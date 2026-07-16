//! 小红书请求签名（`x-s` / `x-t` / `x-s-common` / `x-b3-traceid`），xhshow 纯算法 1:1 移植。
//!
//! 移植来源：
//! - `MediaCrawler/media_platform/xhs/playwright_sign.py`（含 GET 请求 a3 修复补丁）
//! - `MediaCrawler/.venv/.../xhshow/`（Cloxl 的 xhshow 纯算法库，MIT）
//!
//! ## 算法概要（`x-s`，对应 xhshow `sign_xs`）
//!
//! 1. 构造 `content_string`：POST = `uri + compact_json(body)`；GET = `uri?key=val&...`（值用
//!    `quote(v, safe=",")` 编码）。
//! 2. `d_value = md5_hex(content_string)`。
//! 3. 构造 144 字节 `payload`（[`build_payload_array`]）：版本号 + 随机种子 + 时间戳 +
//!    环境指纹占位 + `md5 ^ seed` + `a1` + `appid` + 环境校验 + `a3`（`custom_hash_v2`）。
//! 4. 对 `payload` 用 144 字节 `HEX_KEY` 做 XOR（[`xor_transform_array`]）。
//! 5. `x3 = encode_x3(xor)`（标准 base64 后置换为 `X3_BASE64_ALPHABET`）。
//! 6. `x-s = "XYS_" + encode(json({"x0","x1","x2","x3":"mns0301_"+x3,"x4"}))`（置换为
//!    `CUSTOM_BASE64_ALPHABET`）。
//!
//! ## a3 的 GET 修复
//!
//! xhshow 原实现对所有请求用 `md5(extract_api_path(content))` 计算 a3，但浏览器实际行为：
//! POST 用 `md5(api_path)`（即去掉 JSON body 后的 URI），GET 用 `md5(完整 content_string)`。
//! [`build_payload_array`] 内联了该修复：`content` 含 `{` 走 POST 分支，否则走 GET 分支。
//!
//! ## `x-s-common`
//!
//! 由 cookie 中的 `a1` + 浏览器指纹子集生成的 `b1`（RC4(key="xhswebmplfbt") 加密后自定
//! base64）+ `x9 = crc32_js_int(b1)`，按固定模板序列化为 JSON 后自定 base64。
//!
//! `x-s-common` 含大量随机指纹字段，跨实现无法逐字节比对；本实现只校验格式与算法
//! 原语（CRC32 / RC4 / base64）。`x-s` 在固定随机性下可逐字节对齐 Python 参考（见单测）。
//!
//! ## 随机性
//!
//! 不引入 `rand` 依赖：使用线程局部 xorshift64（由 `SystemTime` 纳秒 + 计数器播种）。
//! 随机性仅用于签名内部的"环境伪装"字段，xhs 不校验其密码学强度（Python 原实现同样用
//! `random` 而非 `secrets`，仅个别哈希用 `secrets`）。

use md5::{Digest, Md5};
use once_cell::sync::Lazy;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// 常量（全部来自 xhshow `CryptoConfig`，勿改）
// ---------------------------------------------------------------------------

/// 标准_base64 字母表（用于先生成标准 base64 再置换）。
const STANDARD_BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `x-s` / `x-s-common` 外层自定 base64 字母表。
const CUSTOM_BASE64_ALPHABET: &[u8; 64] =
    b"ZmserbBoHQtNP+wOcza/LpngG8yJq42KWYj0DSfdikx3VT16IlUAFM97hECvuRX5";

/// `x3` 自定 base64 字母表。
const X3_BASE64_ALPHABET: &[u8; 64] =
    b"MfgqrsbcyzPQRStuvC7mn501HIJBo2DEFTKdeNOwxWXYZap89+/A4UVLhijkl63G";

/// payload 异或密钥（288 hex = 144 字节，覆盖整个 payload）。
const HEX_KEY: &str = "71a302257793271ddd273bcee3e4b98d9d7935e1da33f5765e2ea8afb6dc77a51a499d23b67c20660025860cbf13d4540d92497f58686c574e508f46e1956344f39139bf4faf22a3eef120b79258145b2feb5193b6478669961298e79bedca646e1a693a926154a5a7a1bd1cf0dedb742f917a747a1e388b234f2277516db7116035439730fa61e9822a0eca7bff72d8";
static HEX_KEY_BYTES: Lazy<Vec<u8>> = Lazy::new(|| hex::decode(HEX_KEY).expect("HEX_KEY hex"));

/// payload 版本字节（mns0301）。
const VERSION_BYTES: [u8; 4] = [121, 104, 96, 41];
/// a3 段前缀。
const A3_PREFIX: [u8; 4] = [2, 97, 51, 16];

const PAYLOAD_LENGTH: usize = 144;
const A1_LENGTH: usize = 52;
const APP_ID_LENGTH: usize = 10;
const MD5_XOR_LENGTH: usize = 8;

/// 环境检测表（part11 XOR 用）。
const ENV_TABLE: [u8; 15] = [
    115, 248, 83, 102, 103, 201, 181, 131, 99, 94, 4, 68, 250, 132, 21,
];
/// 默认环境校验值（正常浏览器）。
const ENV_CHECKS_DEFAULT: [u8; 15] = [0, 1, 18, 1, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0];

/// `custom_hash_v2` 初始向量。
const HASH_IV: [u32; 4] = [1831565813, 461845907, 2246822507, 3266489909];

/// b1 RC4 密钥（`localStorage.getItem("b1")` 派生）。
const B1_SECRET_KEY: &[u8] = b"xhswebmplfbt";

/// b1 指纹里的 canvas 哈希（xhshow 固定值 `CANVAS_HASH`）。
const CANVAS_HASH: &str = "742cc32c";

/// trace_id / b3 用的十六进制字符集。
const HEX_CHARS: &[u8; 16] = b"abcdef0123456789";

// ---------------------------------------------------------------------------
// 随机数（无 rand 依赖：线程局部 xorshift64）
// ---------------------------------------------------------------------------

static RNG_COUNTER: AtomicU64 = AtomicU64::new(0x243F_6A88_85A3_08D3);

fn seed_init() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let c = RNG_COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    (now.as_nanos() as u64) ^ c.rotate_left(17)
}

thread_local! {
    static RNG_STATE: Cell<u64> = Cell::new(seed_init());
}

/// 下一个 64 位伪随机数（xorshift64，13/7/17 变体）。
fn next_u64() -> u64 {
    RNG_STATE.with(|cell| {
        let mut s = cell.get();
        if s == 0 {
            s = seed_init();
        }
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        cell.set(s);
        s
    })
}

fn next_u32() -> u32 {
    (next_u64() & 0xFFFF_FFFF) as u32
}

/// 闭区间 `[min, max]` 上的随机 u32。
fn rng_range(min: u32, max: u32) -> u32 {
    if max <= min {
        return min;
    }
    let span = (max - min + 1) as u64;
    min + (next_u64() % span) as u32
}

// ---------------------------------------------------------------------------
// 公共类型
// ---------------------------------------------------------------------------

/// 签名后的请求头集合。
#[derive(Debug, Clone)]
pub struct XhsHeaders {
    /// `x-s`：主签名（`XYS_` 前缀）。
    pub x_s: String,
    /// `x-t`：毫秒级 Unix 时间戳（字符串）。
    pub x_t: String,
    /// `x-s-common`：通用签名（自定 base64）。
    pub x_s_common: String,
    /// `x-b3-traceid`：16 位十六进制链路追踪 ID。
    pub x_b3_traceid: String,
}

/// 待签名内容来源。决定 `content_string` 的拼装方式。
pub enum SignInput<'a> {
    /// POST：`content_string = uri + body_json`（`body_json` 须为 Python
    /// `json.dumps(payload, separators=(",",":"), ensure_ascii=False)` 等价的紧凑 JSON，
    /// 由调用方保证键序）。
    Post { uri: &'a str, body_json: &'a str },
    /// GET：`content_string = uri?k1=v1&k2=v2`，值用 `quote(v, safe=",")` 编码。
    Get {
        uri: &'a str,
        params: Vec<(&'a str, &'a str)>,
    },
}

impl SignInput<'_> {
    /// 构造送入 MD5/签名的 content_string（对齐 xhshow `_build_content_string`）。
    pub fn content_string(&self) -> String {
        match self {
            SignInput::Post { uri, body_json } => format!("{uri}{body_json}"),
            SignInput::Get { uri, params } => {
                if params.is_empty() {
                    return uri.to_string();
                }
                let q: Vec<String> = params
                    .iter()
                    .map(|(k, v)| format!("{k}={}", quote_safe_comma(v)))
                    .collect();
                format!("{uri}?{}", q.join("&"))
            }
        }
    }

    /// 是否为 POST（content 含 `{` 判定的依据）。
    pub fn is_post(&self) -> bool {
        matches!(self, SignInput::Post { .. })
    }
}

/// 签名 payload 用到的"环境伪装"随机性。
///
/// 抽出为独立结构，便于在测试中固定以逐字节对齐 Python 参考（见
/// `sign_xs_matches_python_reference`）。
#[derive(Debug, Clone, Copy)]
pub struct Randomness {
    /// 32 位随机种子（写入 payload 并作为多处 XOR key）。
    pub seed: u32,
    /// 环境指纹时间偏移（秒，xhshow 默认 10..=50）。
    pub time_offset: u32,
    /// 序列值（xhshow 默认 15..=50）。
    pub sequence: u32,
    /// window props 长度（xhshow 默认 1000..=1200）。
    pub window_props: u32,
}

impl Randomness {
    /// 用线程局部 RNG 生成一组随机性。
    pub fn generate() -> Self {
        Self {
            seed: next_u32(),
            time_offset: rng_range(10, 50),
            sequence: rng_range(15, 50),
            window_props: rng_range(1000, 1200),
        }
    }
}

// ---------------------------------------------------------------------------
// 顶层签名入口
// ---------------------------------------------------------------------------

/// 为请求生成全套签名头。
///
/// - `cookie_str`：原始 Cookie 串，至少需含 `a1=...`。
/// - `timestamp`：Unix 秒（浮点，允许亚秒精度）。
pub fn sign(input: &SignInput<'_>, cookie_str: &str, timestamp: f64) -> XhsHeaders {
    let a1 = extract_cookie(cookie_str, "a1").unwrap_or_default();
    let content = input.content_string();
    let x_s = sign_xs(&content, &a1, timestamp);
    let ts_ms = (timestamp * 1000.0) as u64;
    XhsHeaders {
        x_s,
        x_t: ts_ms.to_string(),
        x_s_common: sign_xs_common(&a1, ts_ms),
        x_b3_traceid: b3_trace_id(),
    }
}

/// 生成 `x-s`（使用随机随机性）。
pub fn sign_xs(content_string: &str, a1_value: &str, timestamp: f64) -> String {
    sign_xs_with_randomness(content_string, a1_value, timestamp, &Randomness::generate())
}

/// 生成 `x-s`（使用给定随机性；测试用，可逐字节对齐 Python）。
pub fn sign_xs_with_randomness(
    content_string: &str,
    a1_value: &str,
    timestamp: f64,
    rng: &Randomness,
) -> String {
    let payload = build_payload_array(content_string, a1_value, "xhs-pc-web", timestamp, rng);
    let xor = xor_transform_array(&payload);
    let x3_b64 = encode_x3(&xor);
    // xhsign SIGNATURE_DATA_TEMPLATE：固定键序 x0,x1,x2,x3,x4
    let sig_json = format!(
        "{{\"x0\":\"4.2.6\",\"x1\":\"xhs-pc-web\",\"x2\":\"Windows\",\"x3\":\"mns0301_{x3_b64}\",\"x4\":\"\"}}"
    );
    format!("XYS_{}", encode(sig_json.as_bytes()))
}

/// 生成 `x-s-common`（依赖 cookie 中的 a1）。
pub fn sign_xs_common(a1_value: &str, timestamp_ms: u64) -> String {
    let b1 = generate_b1(timestamp_ms);
    let x9 = crc32_js_signed(b1.as_bytes());
    let a1_json = serde_json::to_string(a1_value).unwrap_or_else(|_| String::from("\"\""));
    let b1_json = serde_json::to_string(&b1).unwrap_or_else(|_| String::from("\"\""));
    // xhshow SIGNATURE_XSCOMMON_TEMPLATE：固定键序
    let json = format!(
        "{{\"s0\":5,\"s1\":\"\",\"x0\":\"1\",\"x1\":\"4.2.6\",\"x2\":\"Windows\",\
         \"x3\":\"xhs-pc-web\",\"x4\":\"4.86.0\",\"x5\":{a1_json},\"x6\":\"\",\"x7\":\"\",\
         \"x8\":{b1_json},\"x9\":{x9},\"x10\":0,\"x11\":\"normal\"}}"
    );
    encode(json.as_bytes())
}

/// 当前 Unix 秒（浮点）。
pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// 暴露给平台层使用的 64 位随机数（如 `search_id` 的随机段）。
pub fn next_search_rand() -> u64 {
    next_u64()
}

// ---------------------------------------------------------------------------
// payload 构造
// ---------------------------------------------------------------------------

/// 构造 144 字节 payload（mns0301 版本）。
///
/// 对齐 xhshow `CryptoProcessor.build_payload_array` + playwright_sign 的 a3 GET 修复。
fn build_payload_array(
    content_string: &str,
    a1_value: &str,
    app_identifier: &str,
    timestamp: f64,
    rng: &Randomness,
) -> Vec<u8> {
    let seed = rng.seed;
    let seed_byte = (seed & 0xFF) as u8;

    let ts_ms = (timestamp * 1000.0) as u64;
    let ts_bytes = le_bytes(ts_ms, 8);

    let eff_ts_ms = ((timestamp - rng.time_offset as f64) * 1000.0) as u64;
    let eff_ts_bytes = le_bytes(eff_ts_ms, 8);

    let d_value = md5_hex(content_string.as_bytes());
    let md5_full = hex::decode(&d_value).expect("md5 hex");

    let mut p: Vec<u8> = Vec::with_capacity(PAYLOAD_LENGTH);
    p.extend_from_slice(&VERSION_BYTES);
    p.extend(le_bytes(seed as u64, 4));
    p.extend_from_slice(&ts_bytes);
    p.extend_from_slice(&eff_ts_bytes);
    p.extend(le_bytes(rng.sequence as u64, 4));
    p.extend(le_bytes(rng.window_props as u64, 4));
    p.extend(le_bytes(content_string.len() as u64, 4));

    // md5 ^ seed（取前 MD5_XOR_LENGTH 字节）
    for byte in md5_full.iter().take(MD5_XOR_LENGTH) {
        p.push(*byte ^ seed_byte);
    }

    // a1：截断/补零到 A1_LENGTH，前置长度字节
    let mut a1b = a1_value.as_bytes().to_vec();
    a1b.truncate(A1_LENGTH);
    a1b.resize(A1_LENGTH, 0);
    p.push(a1b.len() as u8);
    p.extend(a1b);

    // app identifier：截断/补零到 APP_ID_LENGTH
    let mut appb = app_identifier.as_bytes().to_vec();
    appb.truncate(APP_ID_LENGTH);
    appb.resize(APP_ID_LENGTH, 0);
    p.push(appb.len() as u8);
    p.extend(appb);

    // part11：环境检测段（16 字节）
    p.push(1);
    p.push(seed_byte ^ ENV_TABLE[0]);
    for i in 1..15 {
        p.push(ENV_TABLE[i] ^ ENV_CHECKS_DEFAULT[i]);
    }

    // a3：POST 用 md5(api_path)，GET 用 md5(完整 content_string)（playwright_sign 修复）
    let a3_md5_hex = if content_string.contains('{') {
        md5_hex(extract_api_path(content_string).as_bytes())
    } else {
        d_value.clone()
    };
    let a3_md5_bytes = hex::decode(&a3_md5_hex).expect("a3 md5 hex");
    let mut hash_input = Vec::with_capacity(ts_bytes.len() + a3_md5_bytes.len());
    hash_input.extend_from_slice(&ts_bytes);
    hash_input.extend_from_slice(&a3_md5_bytes);
    let a3_hash = custom_hash_v2(&hash_input);

    p.extend_from_slice(&A3_PREFIX);
    for b in &a3_hash {
        p.push(b ^ seed_byte);
    }

    debug_assert_eq!(p.len(), PAYLOAD_LENGTH, "payload 必须为 144 字节");
    p
}

/// 从可能含查询串/JSON body 的 content 中提取纯 API 路径（去掉 `?` 与 `{` 之后的部分）。
fn extract_api_path(s: &str) -> &str {
    let brace = s.find('{');
    let question = s.find('?');
    match (brace, question) {
        (Some(b), Some(q)) => &s[..b.min(q)],
        (Some(b), None) => &s[..b],
        (None, Some(q)) => &s[..q],
        (None, None) => s,
    }
}

// ---------------------------------------------------------------------------
// a3 哈希：custom_hash_v2
// ---------------------------------------------------------------------------

/// xhshow `_custom_hash_v2`：8 字节对齐的轻量哈希，输出 16 字节。
///
/// 输入长度须为 8 的倍数（a3 段固定为 `ts_bytes(8) + md5(16) = 24`）。
fn custom_hash_v2(input: &[u8]) -> [u8; 16] {
    let mut s = HASH_IV;
    let len = input.len() as u32;

    s[0] ^= len;
    s[1] ^= len.wrapping_shl(8);
    s[2] ^= len.wrapping_shl(16);
    s[3] ^= len.wrapping_shl(24);

    for chunk in input.chunks_exact(8) {
        let v0 = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let v1 = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        s[0] = (s[0].wrapping_add(v0) ^ s[2]).rotate_left(7);
        s[1] = ((v0 ^ s[1]).wrapping_add(s[3])).rotate_left(11);
        s[2] = (s[2].wrapping_add(v1) ^ s[0]).rotate_left(13);
        s[3] = ((s[3] ^ v1).wrapping_add(s[1])).rotate_left(17);
    }

    let t0 = s[0] ^ len;
    let t1 = s[1] ^ t0;
    let t2 = s[2].wrapping_add(t1);
    let t3 = s[3] ^ t2;
    let rt0 = t0.rotate_left(9);
    let rt1 = t1.rotate_left(13);
    let rt2 = t2.rotate_left(17);
    let rt3 = t3.rotate_left(19);

    s[0] = rt0.wrapping_add(rt2);
    s[1] = rt1 ^ rt3;
    s[2] = rt2.wrapping_add(s[0]);
    s[3] = rt3 ^ s[1];

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&s[0].to_le_bytes());
    out[4..8].copy_from_slice(&s[1].to_le_bytes());
    out[8..12].copy_from_slice(&s[2].to_le_bytes());
    out[12..16].copy_from_slice(&s[3].to_le_bytes());
    out
}

/// 用 `HEX_KEY` 对 payload 逐字节 XOR（对齐 xhshow `xor_transform_array`）。
fn xor_transform_array(src: &[u8]) -> Vec<u8> {
    let key = &*HEX_KEY_BYTES;
    src.iter()
        .enumerate()
        .map(|(i, &b)| if i < key.len() { b ^ key[i] } else { b })
        .collect()
}

// ---------------------------------------------------------------------------
// base64（标准编码 + 字母表置换）
// ---------------------------------------------------------------------------

/// 标准 base64 编码（RFC 4648，`=` 填充）。
fn std_b64_encode(data: &[u8]) -> String {
    const STD: &[u8; 64] = STANDARD_BASE64_ALPHABET;
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | data[i + 2] as u32;
        out.push(STD[(n >> 18 & 63) as usize] as char);
        out.push(STD[(n >> 12 & 63) as usize] as char);
        out.push(STD[(n >> 6 & 63) as usize] as char);
        out.push(STD[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = data.len() - i;
    if rem == 1 {
        let n = (data[i] as u32) << 16;
        out.push(STD[(n >> 18 & 63) as usize] as char);
        out.push(STD[(n >> 12 & 63) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(STD[(n >> 18 & 63) as usize] as char);
        out.push(STD[(n >> 12 & 63) as usize] as char);
        out.push(STD[(n >> 6 & 63) as usize] as char);
        out.push('=');
    }
    out
}

/// 字母表置换：把标准 base64 串中的字符按 `from → to` 重映射（`=` 保留）。
fn b64_translate(encoded: &str, from: &[u8; 64], to: &[u8; 64]) -> String {
    encoded
        .bytes()
        .map(|b| {
            if b == b'=' {
                b
            } else {
                to[from.iter().position(|&c| c == b).unwrap_or(0)]
            }
        })
        .map(|b| b as char)
        .collect()
}

/// `x-s` / `x-s-common` 外层编码：标准 base64 → `CUSTOM_BASE64_ALPHABET`。
fn encode(data: &[u8]) -> String {
    b64_translate(
        &std_b64_encode(data),
        STANDARD_BASE64_ALPHABET,
        CUSTOM_BASE64_ALPHABET,
    )
}

/// `x3` 编码：标准 base64 → `X3_BASE64_ALPHABET`。
fn encode_x3(data: &[u8]) -> String {
    b64_translate(
        &std_b64_encode(data),
        STANDARD_BASE64_ALPHABET,
        X3_BASE64_ALPHABET,
    )
}

// ---------------------------------------------------------------------------
// CRC32（xhshow `CRC32.crc32_js_int`，用于 x9）
// ---------------------------------------------------------------------------

/// 标准多项式 0xEDB88320 的 CRC32 查找表（编译期生成）。
const CRC_TABLE: [u32; 256] = {
    const POLY: u32 = 0xEDB88320;
    let mut tbl = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = i as u32;
        let mut j = 0;
        while j < 8 {
            r = if r & 1 != 0 { (r >> 1) ^ POLY } else { r >> 1 };
            j += 1;
        }
        tbl[i] = r;
        i += 1;
    }
    tbl
};

/// xhshow `CRC32.crc32_js_int(signed=true)`：`(-1 ^ c ^ 0xEDB88320)` 的有符号 32 位结果。
fn crc32_js_signed(data: &[u8]) -> i64 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = (((c & 0xFF) as u8) ^ b) as usize;
        c = CRC_TABLE[idx] ^ (c >> 8);
    }
    let u = (0xFFFF_FFFFu32 ^ c) ^ 0xEDB88320u32;
    if u & 0x8000_0000 != 0 {
        u as i64 - 0x1_0000_0000
    } else {
        u as i64
    }
}

// ---------------------------------------------------------------------------
// MD5
// ---------------------------------------------------------------------------

fn md5_bytes(data: &[u8]) -> Vec<u8> {
    let mut h = Md5::new();
    h.update(data);
    h.finalize().to_vec()
}

fn md5_hex(data: &[u8]) -> String {
    hex::encode(md5_bytes(data))
}

// ---------------------------------------------------------------------------
// RC4（b1 加密，对齐 pycryptodome ARC4）
// ---------------------------------------------------------------------------

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s = [0u8; 256];
    for (i, byte) in s.iter_mut().enumerate() {
        *byte = i as u8;
    }
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let mut i_u8: u8 = 0;
    let mut j_u8: u8 = 0;
    let mut out = Vec::with_capacity(data.len());
    for &b in data {
        i_u8 = i_u8.wrapping_add(1);
        j_u8 = j_u8.wrapping_add(s[i_u8 as usize]);
        s.swap(i_u8 as usize, j_u8 as usize);
        let k = s[(s[i_u8 as usize].wrapping_add(s[j_u8 as usize])) as usize];
        out.push(b ^ k);
    }
    out
}

// ---------------------------------------------------------------------------
// b1（x-s-common 指纹）
// ---------------------------------------------------------------------------

/// 生成 b1：取指纹子集 → JSON → RC4 → url-quote → 自定 base64。
///
/// 复刻 xhshow `FingerprintGenerator.generate_b1`。b1 依赖的指纹字段大多是固定值，
/// 仅 `x36`(随机 1..=20)、`x43`(canvas hash)、`x44`(毫秒时间戳) 变化。
fn generate_b1(timestamp_ms: u64) -> String {
    let x36 = rng_range(1, 20).to_string();
    let x44 = timestamp_ms.to_string();

    // b1 指纹子集（键序与 xhshow 完全一致；x39 为整数 0）
    let mut j = String::from("{");
    j.push_str("\"x33\":\"0\"");
    j.push_str(",\"x34\":\"0\"");
    j.push_str(",\"x35\":\"0\"");
    j.push_str(&format!(",\"x36\":\"{x36}\""));
    j.push_str(",\"x37\":\"0|0|0|0|0|0|0|0|0|1|0|0|0|0|0|0|0|0|1|0|0|0|0|0\"");
    j.push_str(",\"x38\":\"0|0|1|0|1|0|0|0|0|0|1|0|1|0|1|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0|0\"");
    j.push_str(",\"x39\":0");
    j.push_str(",\"x42\":\"3.4.4\"");
    j.push_str(&format!(",\"x43\":\"{CANVAS_HASH}\""));
    j.push_str(&format!(",\"x44\":\"{x44}\""));
    j.push_str(",\"x45\":\"__SEC_CAV__1-1-1-1-1|__SEC_WSA__|\"");
    j.push_str(",\"x46\":\"false\"");
    j.push_str(",\"x48\":\"\"");
    j.push_str(",\"x49\":\"{list:[],type:}\"");
    j.push_str(",\"x50\":\"\"");
    j.push_str(",\"x51\":\"\"");
    j.push_str(",\"x52\":\"\"");
    j.push_str(",\"x82\":\"_0x17a2|_0x1954\"");
    j.push('}');

    let cipher = rc4(B1_SECRET_KEY, j.as_bytes());
    let quoted = quote_b1(&cipher);

    // 等价 Python `quoted.split("%")[1:]`：丢弃首个 '%' 之前的明文段，逐段取 2 hex + 余字面量
    let mut b: Vec<u8> = Vec::new();
    let mut parts = quoted.split('%');
    parts.next();
    for part in parts {
        let pb = part.as_bytes();
        if pb.len() >= 2 {
            b.push((hex_val(pb[0]) << 4) | hex_val(pb[1]));
            for &c in &pb[2..] {
                b.push(c);
            }
        }
    }
    encode(&b)
}

/// b1 的 url-quote：保留 `A-Za-z0-9 _.-~ ! * ' ( )`，其余 `%XX`（大写）。
fn quote_b1(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for &b in input {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'_'
            | b'.'
            | b'-'
            | b'~'
            | b'!'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'A'..=b'F' => c - b'A' + 10,
        b'a'..=b'f' => c - b'a' + 10,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// trace id / cookie 工具
// ---------------------------------------------------------------------------

/// 生成 16 位十六进制 `x-b3-traceid`（对齐 xhshow `generate_b3_trace_id`）。
pub fn b3_trace_id() -> String {
    (0..16)
        .map(|_| HEX_CHARS[(next_u32() as usize) % 16] as char)
        .collect()
}

/// 生成简单 trace_id（时间戳+随机，对齐 `xhs_sign.get_trace_id` 的用途）。
pub fn trace_id() -> String {
    b3_trace_id()
}

/// 从 Cookie 串中提取某个键的值。
pub fn extract_cookie(cookie_str: &str, key: &str) -> Option<String> {
    for part in cookie_str.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == key {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 内部小工具
// ---------------------------------------------------------------------------

/// 无符号整数的低 `len` 字节（小端）。
fn le_bytes(val: u64, len: usize) -> Vec<u8> {
    (0..len).map(|i| ((val >> (8 * i)) & 0xFF) as u8).collect()
}

/// `quote(v, safe=",")`：保留字母数字与 `_.-~,`，其余 `%XX`（大写），按 UTF-8 字节处理。
fn quote_safe_comma(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' | b',' => {
                out.push(b as char)
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{b:02X}"));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- 算法原语回归（值来自 Python xhshow 实测） --

    #[test]
    fn md5_matches_python() {
        assert_eq!(md5_hex(b"hello"), "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn crc32_js_matches_python() {
        assert_eq!(crc32_js_signed(b"hello"), -609737306);
        assert_eq!(crc32_js_signed(b"ab"), 1933298509);
        assert_eq!(crc32_js_signed(b""), -306674912);
    }

    #[test]
    fn custom_base64_encode_matches_python() {
        assert_eq!(encode(b"hello"), "yBpVJBu=");
        assert_eq!(encode(b"ABC"), "cLQe");
        assert_eq!(encode(b"A"), "cc==");
        assert_eq!(encode(b"AB"), "cLH=");
        assert_eq!(encode(b""), "");
    }

    #[test]
    fn x3_base64_encode_matches_python() {
        assert_eq!(encode_x3(b"ABC"), "vnzq");
        assert_eq!(encode_x3(b"hello"), "Jb5ZBbl=");
    }

    #[test]
    fn custom_hash_v2_matches_python() {
        let input: Vec<u8> = (0..8u8).collect();
        assert_eq!(
            custom_hash_v2(&input),
            [239, 246, 89, 36, 44, 11, 46, 222, 16, 8, 29, 152, 1, 107, 141, 193]
        );
    }

    #[test]
    fn xor_transform_first_bytes() {
        // key 前 4 字节 = 71 a3 02 25 → XOR [0,1,2,3] = [113,162,0,38]（Python 实测）
        let got = xor_transform_array(&[0, 1, 2, 3]);
        assert_eq!(got, vec![113, 162, 0, 38]);
    }

    #[test]
    fn extract_api_path_handles_all_forms() {
        assert_eq!(extract_api_path("/api/x{\"a\":1}"), "/api/x");
        assert_eq!(extract_api_path("/api/x?num=30"), "/api/x");
        assert_eq!(extract_api_path("/api/x"), "/api/x");
    }

    #[test]
    fn quote_safe_comma_keeps_comma() {
        assert_eq!(quote_safe_comma("jpg,webp,avif"), "jpg,webp,avif");
        assert_eq!(quote_safe_comma("a b"), "a%20b");
        assert_eq!(quote_safe_comma("a=b"), "a%3Db");
    }

    // -- x-s 端到端：固定随机性下逐字节对齐 Python 参考（golden） --

    #[test]
    fn sign_xs_matches_python_reference() {
        // 与 MediaCrawler/.venv xhshow 在固定 rng(seed=0x11223344, offset=min, seq=min,
        // win=min) 下产出的 x-s 完全一致；证明 CRC32/MD5/custom_hash_v2/base64/xor/a3 全链路正确。
        let content = "/api/sns/web/v1/search/notes{\"keyword\":\"猫爬架\",\"page\":1,\
                       \"page_size\":20,\"search_id\":\"ABC\",\"sort\":\"general\",\"note_type\":0}";
        let a1 = "1900000000000abcdef0123456789abcdef0123456789abcdef012345678";
        let rng = Randomness {
            seed: 0x11223344,
            time_offset: 10,
            sequence: 15,
            window_props: 1000,
        };
        let xs = sign_xs_with_randomness(content, a1, 1_764_900_000.0, &rng);
        assert_eq!(
            xs,
            "XYS_2UQhPsHCH0c1Pjh9HjIj2erjwjQhyoPTqBPT49pjHjIj2eHjwjQgynEDJ74AHjIj2ePjwjQTJdPI\
             PAZlg94aGLTlqgzB8d8mP08BJFTrzemk8emP4f49pLMawepnJ04x2bSpyFLUy0pO+FDF8b4mprStcF8w\
             cd+ILgSIpnpMadkkJp8LaebjP0+H4BYSz0YUGdSy4r8dysT3/Mc7JSpypB8pzgQVJ9FFzp8O4r8B4MZ\
             EPpYin/+HPnYNafldPpz+c9EIqMQCLDkcpnbLP9lr8LT/Jfznnfl0yLLIaSQQyAmOarEaLSz+q9EawB\
             8YaozU8rTiy7pay7bbcd89zaHVHdWFH0ijHdF="
        );
    }

    // -- 全套签名头格式校验 --

    #[test]
    fn sign_headers_have_sane_format() {
        let input = SignInput::Post {
            uri: "/api/sns/web/v1/search/notes",
            body_json: r#"{"keyword":"测试","page":1,"page_size":20}"#,
        };
        let cookie =
            "a1=1900000000000abcdef0123456789abcdef0123456789abcdef012345678; web_session=x";
        let h = sign(&input, cookie, 1_764_900_000.0);
        assert!(h.x_s.starts_with("XYS_"), "x-s 应以 XYS_ 开头: {}", h.x_s);
        assert!(h.x_s.len() > 200, "x-s 长度应 > 200: {}", h.x_s.len());
        assert!(h.x_t.chars().all(|c| c.is_ascii_digit()), "x-t 应为纯数字");
        assert_eq!(h.x_t.len(), 13, "x-t 应为 13 位毫秒时间戳: {}", h.x_t);
        assert_eq!(h.x_t, "1764900000000");
        assert!(!h.x_s_common.is_empty(), "x-s-common 非空");
        assert!(
            h.x_s_common.len() > 200,
            "x-s-common 长度应 > 200: {}",
            h.x_s_common.len()
        );
        assert_eq!(h.x_b3_traceid.len(), 16, "x-b3-traceid 应为 16 位");
        assert!(h
            .x_b3_traceid
            .chars()
            .all(|c| HEX_CHARS.contains(&(c as u8))));
    }

    #[test]
    fn sign_get_content_string_encodes_with_comma_safe() {
        let input = SignInput::Get {
            uri: "/api/sns/web/v2/comment/page",
            params: vec![("image_formats", "jpg,webp,avif"), ("note_id", "abc")],
        };
        let cs = input.content_string();
        assert_eq!(
            cs,
            "/api/sns/web/v2/comment/page?image_formats=jpg,webp,avif&note_id=abc"
        );
        assert!(!input.is_post());
    }

    #[test]
    fn extract_cookie_finds_a1() {
        let c = "gid=xyz; a1=1900abc; web_session=040069";
        assert_eq!(extract_cookie(c, "a1").as_deref(), Some("1900abc"));
        assert_eq!(extract_cookie(c, "missing"), None);
    }

    #[test]
    fn rc4_roundtrips() {
        let key = b"secret";
        let plain = b"hello world";
        let cipher = rc4(key, plain);
        assert_ne!(cipher, plain);
        assert_eq!(rc4(key, &cipher), plain);
    }

    #[test]
    fn b3_trace_id_shape() {
        let id = b3_trace_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
