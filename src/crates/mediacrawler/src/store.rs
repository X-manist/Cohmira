//! 结果落盘。
//!
//! 兼容原 MediaCrawler 的 `save_data_option`（json/csv/excel）。
//! v1 实现 JSON 与 CSV；excel 由 [`yunying_ops`] 在需要时再补。

use crate::model::Content;
use std::path::Path;

/// 落盘格式。
#[derive(Debug, Clone, Copy)]
pub enum SaveOption {
    Json,
    Csv,
    Excel,
}

/// 把采集结果写入文件。返回写入的绝对路径。
pub fn write(
    out_dir: &Path,
    name: &str,
    opt: SaveOption,
    contents: &[Content],
) -> anyhow::Result<std::path::PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    match opt {
        SaveOption::Json => {
            let path = out_dir.join(format!("{name}.json"));
            let s = serde_json::to_string_pretty(contents)?;
            std::fs::write(&path, s)?;
            Ok(path)
        }
        SaveOption::Csv | SaveOption::Excel => {
            // TODO(csv/excel): 实现 CSV/Excel 落盘以对齐 save_data_option。
            anyhow::bail!("csv/excel 落盘尚未实现，v1 先用 json")
        }
    }
}
