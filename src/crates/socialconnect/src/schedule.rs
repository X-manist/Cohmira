//! 多平台批量发布排期。兼容原 `--schedule "YYYY-MM-DD HH:MM"`。

/// 解析排期字符串为本地时间。格式：`YYYY-MM-DD HH:MM`。
pub fn parse_schedule(raw: &str) -> anyhow::Result<chrono::NaiveDateTime> {
    Ok(chrono::NaiveDateTime::parse_from_str(
        raw.trim(),
        "%Y-%m-%d %H:%M",
    )?)
}
