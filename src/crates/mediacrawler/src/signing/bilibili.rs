//! bilibili wbi 签名（`w_rid`）。
//!
//! 移植自 `MediaCrawler/media_platform/bilibili/help.py` 的 `BilibiliSign`。
//! 参考逆向文档：<https://socialsisteryi.github.io/bilibili-API-collect/docs/misc/sign/wbi.html>。

use md5::{Digest, Md5};

/// wbi 签名器。由 img_key 与 sub_key（从 nav 接口的 `wbi_img.img_url`/`sub_url` 末段提取）构造。
pub struct WbiSign {
    img_key: String,
    sub_key: String,
}

/// 签名结果。
#[derive(Debug, Clone)]
pub struct SignedQuery {
    /// 含 `wts` 与 `w_rid` 的完整查询串（已按字典序排序、quote_plus 编码）。
    pub query: String,
    /// `w_rid`（md5(query + salt) 的 32 位小写 hex）。
    pub w_rid: String,
}

impl WbiSign {
    /// 固定置换表（与 Python 版一致，勿改）。
    const MAP_TABLE: [usize; 64] = [
        46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19,
        29, 28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4,
        22, 25, 54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
    ];

    pub fn new(img_key: impl Into<String>, sub_key: impl Into<String>) -> Self {
        Self {
            img_key: img_key.into(),
            sub_key: sub_key.into(),
        }
    }

    /// 取盐：mixin = img_key + sub_key，按置换表取字符，截前 32 位。
    pub fn salt(&self) -> String {
        let mixin: String = format!("{}{}", self.img_key, self.sub_key);
        let chars: Vec<char> = mixin.chars().collect();
        let mut salt = String::with_capacity(32);
        for &idx in Self::MAP_TABLE.iter() {
            if idx < chars.len() {
                salt.push(chars[idx]);
            }
        }
        salt.chars().take(32).collect()
    }

    /// 对请求参数签名。
    ///
    /// - `params`：原始参数（键值对），会被就地追加 `wts` 与 `w_rid`。
    /// - `wts`：unix 秒级时间戳。
    ///
    /// 行为对齐 Python：追加 `wts` → 按键字典序排序 → 过滤值中的 `!'()*` →
    /// quote_plus 编码拼 query → `w_rid = md5(query + salt)`。
    pub fn sign(&self, params: &mut Vec<(String, String)>, wts: i64) -> SignedQuery {
        params.push(("wts".to_string(), wts.to_string()));
        params.sort_by(|a, b| a.0.cmp(&b.0));

        let filtered: Vec<(String, String)> = params
            .iter()
            .map(|(k, v)| {
                let v: String = v
                    .chars()
                    .filter(|c| !matches!(c, '!' | '\'' | '(' | ')' | '*'))
                    .collect();
                (k.clone(), v)
            })
            .collect();

        let query = urlencode(&filtered);
        let salt = self.salt();
        let w_rid = {
            let mut h = Md5::new();
            h.update(query.as_bytes());
            h.update(salt.as_bytes());
            hex::encode(h.finalize())
        };

        let mut full = query;
        full.push_str("&w_rid=");
        full.push_str(&w_rid);
        SignedQuery { query: full, w_rid }
    }
}

/// 匹配 Python `urllib.parse.urlencode`（quote_plus）的查询串编码：
/// 按 (k,v) 顺序拼接 `k=v`，以 `&` 连接；键值做 quote_plus（空格→`+`，非保留字符→`%XX` 大写）。
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
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // bilibili-API-collect 官方文档示例键（用于回归 salt 与签名格式）。
    const DOC_IMG_KEY: &str = "7cd084941338484aae1ad9425b84077c";
    const DOC_SUB_KEY: &str = "4932caff0ff746eab6f01bf08b70ac45";

    #[test]
    fn salt_is_32_chars_and_deterministic() {
        let s = WbiSign::new(DOC_IMG_KEY, DOC_SUB_KEY);
        let salt = s.salt();
        assert_eq!(salt.len(), 32, "salt 必须为 32 字符");
        assert!(
            salt.chars().all(|c| c.is_ascii_hexdigit()),
            "salt 来自 hex 键，应为 hex"
        );
        assert_eq!(salt, s.salt(), "salt 应确定性");
    }

    #[test]
    fn sign_adds_wts_and_wrid_sorted() {
        let s = WbiSign::new(DOC_IMG_KEY, DOC_SUB_KEY);
        let mut params = vec![
            ("foo".to_string(), "bar".to_string()),
            ("ps".to_string(), "20".to_string()),
        ];
        let signed = s.sign(&mut params, 1_702_204_800_i64);
        assert_eq!(signed.w_rid.len(), 32);
        assert!(signed.w_rid.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(signed.query.contains("wts=1702204800"));
        assert!(signed.query.contains(&format!("w_rid={}", signed.w_rid)));
        // query = urlencode(排序后[foo,ps,wts]) + "&w_rid=..."（w_rid 计算后追加，匹配 Python）
        let keys: Vec<&str> = signed
            .query
            .split('&')
            .map(|kv| kv.split('=').next().unwrap())
            .collect();
        assert_eq!(keys, vec!["foo", "ps", "wts", "w_rid"]);
    }

    #[test]
    fn sign_strips_special_chars_from_values() {
        let s = WbiSign::new(DOC_IMG_KEY, DOC_SUB_KEY);
        let mut params = vec![("k".to_string(), "a!b'c(d)e*f".to_string())];
        let signed = s.sign(&mut params, 1);
        // !'()* 被过滤 → abcdef；字典序 k 最前
        assert!(
            signed.query.starts_with("k=abcdef"),
            "query was: {}",
            signed.query
        );
    }

    #[test]
    fn quote_plus_encodes_space_and_special() {
        assert_eq!(quote_plus("a b"), "a+b");
        assert_eq!(quote_plus("A1-_.~"), "A1-_.~");
        assert_eq!(quote_plus("a/b"), "a%2Fb");
        assert_eq!(quote_plus("a&b"), "a%26b");
    }
}
