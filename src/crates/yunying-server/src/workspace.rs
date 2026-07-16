use std::path::{Path, PathBuf};

use serde_json::Value;

const DEFAULT_SPACE_ID: &str = "default";
const LEGACY_WORKSPACE_DIRS: &[&str] = &[
    "knowledge",
    "manuscripts",
    "advisors",
    "archives",
    "chatrooms",
    "skills",
    "media",
];

#[derive(Debug, Clone)]
pub struct WorkspacePaths {
    pub workspace_root: PathBuf,
    pub active_space_id: String,
    pub spaces_root: PathBuf,
    pub base: PathBuf,
    pub manuscripts: PathBuf,
    pub media: PathBuf,
    pub cover: PathBuf,
    pub knowledge: PathBuf,
    pub skills: PathBuf,
    pub redclaw: PathBuf,
}

pub fn resolve(settings: &Value) -> WorkspacePaths {
    let workspace_root = workspace_root(settings);
    let active_space_id = active_space_id(settings);
    let spaces_root = workspace_root.join("spaces");
    let candidate = spaces_root.join(&active_space_id);
    let base = if active_space_id == DEFAULT_SPACE_ID
        && !candidate.exists()
        && has_legacy_workspace_content(&workspace_root)
    {
        workspace_root.clone()
    } else {
        candidate
    };

    WorkspacePaths {
        workspace_root,
        active_space_id,
        spaces_root,
        manuscripts: base.join("manuscripts"),
        media: base.join("media"),
        cover: base.join("cover"),
        knowledge: base.join("knowledge"),
        skills: base.join("skills"),
        redclaw: base.join("redclaw"),
        base,
    }
}

pub fn workspace_root(settings: &Value) -> PathBuf {
    settings
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(expand_home)
        .unwrap_or_else(|| home_dir().join(".redconvert"))
}

pub fn active_space_id(settings: &Value) -> String {
    settings
        .get("active_space_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_SPACE_ID)
        .to_string()
}

fn has_legacy_workspace_content(root: &Path) -> bool {
    LEGACY_WORKSPACE_DIRS
        .iter()
        .any(|dir_name| root.join(dir_name).exists())
}

fn expand_home(value: &str) -> PathBuf {
    if value == "~" {
        return home_dir();
    }
    if let Some(relative) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        return home_dir().join(relative);
    }
    PathBuf::from(value)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn configured_workspace_uses_active_space() {
        let temp = tempfile::tempdir().unwrap();
        let settings = json!({
            "workspace_dir": temp.path().to_string_lossy(),
            "active_space_id": "space-a"
        });
        let paths = resolve(&settings);
        assert_eq!(paths.base, temp.path().join("spaces/space-a"));
        assert_eq!(paths.media, temp.path().join("spaces/space-a/media"));
    }

    #[test]
    fn default_space_keeps_legacy_layout() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("manuscripts")).unwrap();
        let settings = json!({
            "workspace_dir": temp.path().to_string_lossy(),
            "active_space_id": "default"
        });
        let paths = resolve(&settings);
        assert_eq!(paths.base, temp.path());
    }
}
