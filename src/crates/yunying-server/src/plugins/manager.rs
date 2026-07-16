use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MANIFEST_PATHS: [&str; 4] = [
    ".plugin/plugin.json",
    ".codex-plugin/plugin.json",
    ".goose-plugin/plugin.json",
    "plugin.json",
];
const INSTALL_INDEX_FILE: &str = ".jiuban-plugin-files.json";
const INSTALL_META_FILE: &str = ".jiuban-plugin-install.json";
const COPY_IGNORES: [&str; 11] = [
    ".git",
    ".venv",
    "venv",
    "node_modules",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    "dist",
    "build",
    "release",
];

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginRuntimeManifest {
    #[serde(rename = "type", default)]
    pub runtime_type: String,
    #[serde(default)]
    pub manager: String,
    #[serde(default)]
    pub python: String,
    #[serde(default)]
    pub requirements: String,
    #[serde(default)]
    pub mcp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingOption {
    pub label: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingGroup {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingField {
    pub key: String,
    pub label: String,
    #[serde(rename = "type", default)]
    pub setting_type: String,
    #[serde(default)]
    pub description: String,
    #[serde(rename = "default", default)]
    pub default_value: Value,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub options: Vec<PluginSettingOption>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub group: String,
    #[serde(default)]
    pub advanced: bool,
    #[serde(default)]
    pub env: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingsManifest {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub groups: Vec<PluginSettingGroup>,
    #[serde(default)]
    pub fields: Vec<PluginSettingField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default = "default_plugin_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub runtime: PluginRuntimeManifest,
    #[serde(default)]
    pub skills: serde_json::Value,
    #[serde(default)]
    pub capabilities: serde_json::Value,
    #[serde(default)]
    pub settings: PluginSettingsManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub root: PathBuf,
    pub source: String,
    pub runtime_type: String,
    pub runtime_manager: String,
    pub mcp_entry: Option<PathBuf>,
    pub settings: PluginSettingsManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingsSnapshot {
    pub plugin_id: String,
    pub values: BTreeMap<String, Value>,
    pub secret_keys_set: Vec<String>,
    pub missing_required: Vec<String>,
    pub configured: bool,
    pub config_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StoredPluginSettings {
    version: u8,
    plugin_id: String,
    values: BTreeMap<String, Value>,
    updated_at: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallResult {
    #[serde(flatten)]
    pub plugin: PluginInfo,
    pub installed: bool,
    pub copied_files: usize,
    pub reused_files: usize,
    pub plugin_home: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileRecord {
    sha256: String,
    size: u64,
    mode: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FileIndex {
    version: u8,
    digest: String,
    files: BTreeMap<String, FileRecord>,
}

fn default_plugin_version() -> String {
    "0.0.0".into()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn plugin_install_root() -> PathBuf {
    std::env::var_os("JIUBAN_PLUGIN_HOME")
        .or_else(|| std::env::var_os("AGENTS_PLUGIN_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".agents").join("plugins"))
}

pub fn plugin_settings_root() -> PathBuf {
    std::env::var_os("JIUBAN_PLUGIN_SETTINGS_HOME")
        .or_else(|| std::env::var_os("AGENTS_PLUGIN_SETTINGS_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".agents").join("plugin-settings"))
}

pub fn plugin_settings_path(plugin_name: &str) -> anyhow::Result<PathBuf> {
    Ok(plugin_settings_root().join(format!("{}.json", normalize_plugin_name(plugin_name)?)))
}

pub fn builtin_plugin_roots() -> Vec<PathBuf> {
    // A packaged desktop app sets this to its sealed Resources directory. Treat
    // that explicit path as authoritative so production startup never scans
    // compile-time source paths outside the app bundle.
    if let Some(path) = std::env::var_os("JIUBAN_BUILTIN_PLUGINS_ROOT") {
        let path = PathBuf::from(path);
        if let Ok(canonical) = path.canonicalize() {
            if canonical.is_dir() {
                return vec![canonical];
            }
        }
    }

    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe() {
        if let Some(contents_dir) = executable.parent().and_then(Path::parent) {
            candidates.push(contents_dir.join("Resources").join("builtin-plugins"));
        }
        if let Some(executable_dir) = executable.parent() {
            candidates.push(executable_dir.join("resources").join("builtin-plugins"));
            candidates.push(executable_dir.join("builtin-plugins"));
        }
    }
    candidates.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("builtin-plugins"),
    );
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("builtin-plugins"));
        candidates.push(cwd.join("src").join("builtin-plugins"));
    }

    let mut seen = HashSet::new();
    candidates
        .into_iter()
        .filter_map(|path| path.canonicalize().ok())
        .filter(|path| path.is_dir() && seen.insert(path.clone()))
        .collect()
}

fn manifest_path(root: &Path) -> Option<PathBuf> {
    MANIFEST_PATHS
        .iter()
        .map(|relative| root.join(relative))
        .find(|candidate| candidate.is_file())
}

fn normalize_plugin_name(value: &str) -> anyhow::Result<String> {
    let mut normalized = String::new();
    let mut previous_separator = false;
    for character in value.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
            normalized.push(character);
            previous_separator = false;
        } else if !previous_separator {
            normalized.push('-');
            previous_separator = true;
        }
    }
    let normalized = normalized.trim_matches('-').to_string();
    if normalized.is_empty() || normalized == "." || normalized == ".." {
        anyhow::bail!("插件 manifest 缺少有效 name");
    }
    Ok(normalized)
}

pub fn read_plugin_manifest(root: &Path) -> anyhow::Result<PluginManifest> {
    let path = manifest_path(root).ok_or_else(|| {
        anyhow::anyhow!(
            "插件目录缺少 manifest；需要以下任一文件：{}",
            MANIFEST_PATHS.join(", ")
        )
    })?;
    let mut manifest: PluginManifest = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("读取插件 manifest 失败：{}", path.display()))?,
    )
    .with_context(|| format!("解析插件 manifest 失败：{}", path.display()))?;
    manifest.name = normalize_plugin_name(&manifest.name)?;
    if manifest.display_name.trim().is_empty() {
        manifest.display_name = manifest.name.clone();
    }
    if manifest.version.trim().is_empty() {
        manifest.version = default_plugin_version();
    }
    validate_settings_manifest(&manifest.settings)?;
    Ok(manifest)
}

fn validate_settings_manifest(settings: &PluginSettingsManifest) -> anyhow::Result<()> {
    let allowed_types = [
        "string",
        "multiline",
        "secret",
        "number",
        "integer",
        "boolean",
        "select",
        "path",
    ];
    let mut keys = HashSet::new();
    for field in &settings.fields {
        let key = field.key.trim();
        if key.is_empty() {
            anyhow::bail!("插件 settings.fields 存在空 key");
        }
        if !keys.insert(key.to_string()) {
            anyhow::bail!("插件 settings.fields 存在重复 key：{key}");
        }
        if !allowed_types.contains(&field.setting_type.as_str()) {
            anyhow::bail!(
                "插件设置字段 {} 使用了不支持的 type：{}",
                key,
                field.setting_type
            );
        }
        if field.setting_type == "select" && field.options.is_empty() {
            anyhow::bail!("插件 select 设置字段 {key} 缺少 options");
        }
    }
    Ok(())
}

fn list_plugin_directories(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut plugins = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && manifest_path(path).is_some())
        .collect::<Vec<_>>();
    plugins.sort();
    plugins
}

pub fn all_plugin_roots() -> Vec<PathBuf> {
    let mut roots = list_plugin_directories(&plugin_install_root());
    for builtin_root in builtin_plugin_roots() {
        roots.extend(list_plugin_directories(&builtin_root));
    }
    let mut seen = HashSet::new();
    roots.retain(|root| {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
        seen.insert(canonical)
    });
    roots
}

fn plugin_info(manifest: &PluginManifest, root: &Path, source: &str) -> PluginInfo {
    PluginInfo {
        id: manifest.name.clone(),
        name: manifest.display_name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        root: root.to_path_buf(),
        source: source.to_string(),
        runtime_type: manifest.runtime.runtime_type.clone(),
        runtime_manager: manifest.runtime.manager.clone(),
        mcp_entry: (!manifest.runtime.mcp.trim().is_empty())
            .then(|| root.join(&manifest.runtime.mcp)),
        settings: manifest.settings.clone(),
    }
}

fn should_ignore(name: &str) -> bool {
    COPY_IGNORES.contains(&name) || matches!(name, INSTALL_INDEX_FILE | INSTALL_META_FILE)
}

fn file_mode(metadata: &fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0
    }
}

fn scan_directory(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, FileRecord>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if should_ignore(&name_text) {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            scan_directory(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let relative = path
            .strip_prefix(root)?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = entry.metadata()?;
        let mut file = fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        files.insert(
            relative,
            FileRecord {
                sha256: digest_hex(hasher.finalize()),
                size: metadata.len(),
                mode: file_mode(&metadata),
            },
        );
    }
    Ok(())
}

fn scan_plugin_files(root: &Path) -> anyhow::Result<FileIndex> {
    let mut files = BTreeMap::new();
    scan_directory(root, root, &mut files)?;
    let mut digest = Sha256::new();
    for (relative, record) in &files {
        digest.update(relative.as_bytes());
        digest.update([0]);
        digest.update(record.sha256.as_bytes());
        digest.update([0]);
    }
    Ok(FileIndex {
        version: 1,
        digest: digest_hex(digest.finalize()),
        files,
    })
}

fn digest_hex(digest: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = digest.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn read_installed_index(root: &Path) -> Option<FileIndex> {
    serde_json::from_slice(&fs::read(root.join(INSTALL_INDEX_FILE)).ok()?).ok()
}

fn set_file_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

fn write_json_file<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.write_all(b"\n")?;
    Ok(())
}

pub fn install_plugin_directory(
    source_directory: &Path,
    source: &str,
) -> anyhow::Result<PluginInstallResult> {
    let source_root = source_directory
        .canonicalize()
        .with_context(|| format!("插件目录不存在：{}", source_directory.display()))?;
    if !source_root.is_dir() {
        anyhow::bail!("插件路径不是目录：{}", source_root.display());
    }
    let manifest = read_plugin_manifest(&source_root)?;
    let plugin_home = plugin_install_root();
    fs::create_dir_all(&plugin_home)?;
    let target_root = plugin_home.join(&manifest.name);
    if source_root == target_root || source_root.starts_with(&target_root) {
        return Ok(PluginInstallResult {
            plugin: plugin_info(&manifest, &target_root, "installed"),
            installed: false,
            copied_files: 0,
            reused_files: 0,
            plugin_home,
        });
    }

    let next_index = scan_plugin_files(&source_root)?;
    let previous_index = read_installed_index(&target_root);
    if target_root.is_dir()
        && previous_index
            .as_ref()
            .is_some_and(|index| index.digest == next_index.digest)
    {
        return Ok(PluginInstallResult {
            plugin: plugin_info(&manifest, &target_root, "installed"),
            installed: false,
            copied_files: 0,
            reused_files: next_index.files.len(),
            plugin_home,
        });
    }

    let suffix = format!("{}-{}", std::process::id(), now_millis());
    let staging_root = plugin_home.join(format!(".{}-staging-{suffix}", manifest.name));
    let backup_root = plugin_home.join(format!(".{}-backup-{suffix}", manifest.name));
    let _ = fs::remove_dir_all(&staging_root);
    fs::create_dir_all(&staging_root)?;
    let mut copied_files = 0;
    let mut reused_files = 0;

    let install_result = (|| -> anyhow::Result<()> {
        for (relative, file_record) in &next_index.files {
            let source_path = source_root.join(relative);
            let target_path = staging_root.join(relative);
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let previous_path = target_root.join(relative);
            let can_reuse = previous_index
                .as_ref()
                .and_then(|index| index.files.get(relative))
                .is_some_and(|record| record.sha256 == file_record.sha256)
                && previous_path.is_file();
            if can_reuse {
                if fs::hard_link(&previous_path, &target_path).is_err() {
                    fs::copy(&previous_path, &target_path)?;
                }
                reused_files += 1;
            } else {
                fs::copy(&source_path, &target_path)?;
                copied_files += 1;
            }
            let _ = set_file_mode(&target_path, file_record.mode);
        }
        write_json_file(&staging_root.join(INSTALL_INDEX_FILE), &next_index)?;
        write_json_file(
            &staging_root.join(INSTALL_META_FILE),
            &serde_json::json!({
                "installedAtMs": now_millis(),
                "source": source,
                "sourceRoot": source_root,
                "version": manifest.version,
                "digest": next_index.digest,
            }),
        )?;

        if target_root.is_dir() {
            fs::rename(&target_root, &backup_root)?;
        }
        fs::rename(&staging_root, &target_root)?;
        let _ = fs::remove_dir_all(&backup_root);
        Ok(())
    })();

    if let Err(error) = install_result {
        let _ = fs::remove_dir_all(&staging_root);
        if !target_root.is_dir() && backup_root.is_dir() {
            let _ = fs::rename(&backup_root, &target_root);
        }
        return Err(error);
    }

    Ok(PluginInstallResult {
        plugin: plugin_info(&manifest, &target_root, "installed"),
        installed: true,
        copied_files,
        reused_files,
        plugin_home,
    })
}

pub fn sync_builtin_plugins() -> anyhow::Result<Vec<PluginInstallResult>> {
    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for builtin_root in builtin_plugin_roots() {
        for plugin_root in list_plugin_directories(&builtin_root) {
            let manifest = read_plugin_manifest(&plugin_root)?;
            if !seen.insert(manifest.name.clone()) {
                continue;
            }
            results.push(install_plugin_directory(&plugin_root, "builtin")?);
        }
    }
    Ok(results)
}

pub fn list_installed_plugins() -> anyhow::Result<Vec<PluginInfo>> {
    let mut plugins = Vec::new();
    for root in list_plugin_directories(&plugin_install_root()) {
        if let Ok(manifest) = read_plugin_manifest(&root) {
            plugins.push(plugin_info(&manifest, &root, "installed"));
        }
    }
    plugins.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(plugins)
}

pub fn resolve_plugin_root(plugin_name: &str) -> Option<PathBuf> {
    let normalized = normalize_plugin_name(plugin_name).ok()?;
    let installed = plugin_install_root().join(&normalized);
    if manifest_path(&installed).is_some() {
        return Some(installed);
    }
    for builtin_root in builtin_plugin_roots() {
        let candidate = builtin_root.join(&normalized);
        if manifest_path(&candidate).is_some() {
            return Some(candidate);
        }
    }
    None
}

fn setting_value_to_env(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Some(value.clone()),
        Value::String(_) => None,
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn coerce_setting_value(field: &PluginSettingField, value: &Value) -> anyhow::Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let coerced = match field.setting_type.as_str() {
        "string" | "multiline" | "secret" | "path" | "select" => value
            .as_str()
            .map(|value| Value::String(value.to_string()))
            .ok_or_else(|| anyhow::anyhow!("字段 {} 必须是字符串", field.key))?,
        "boolean" => {
            let parsed = value.as_bool().or_else(|| {
                value
                    .as_str()
                    .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
                        "true" | "1" | "yes" | "on" => Some(true),
                        "false" | "0" | "no" | "off" => Some(false),
                        _ => None,
                    })
            });
            Value::Bool(parsed.ok_or_else(|| anyhow::anyhow!("字段 {} 必须是布尔值", field.key))?)
        }
        "integer" => {
            let parsed = value
                .as_i64()
                .or_else(|| {
                    value
                        .as_str()
                        .and_then(|value| value.trim().parse::<i64>().ok())
                })
                .ok_or_else(|| anyhow::anyhow!("字段 {} 必须是整数", field.key))?;
            if field.min.is_some_and(|min| (parsed as f64) < min)
                || field.max.is_some_and(|max| (parsed as f64) > max)
            {
                anyhow::bail!("字段 {} 超出允许范围", field.key);
            }
            Value::Number(parsed.into())
        }
        "number" => {
            let parsed = value
                .as_f64()
                .or_else(|| {
                    value
                        .as_str()
                        .and_then(|value| value.trim().parse::<f64>().ok())
                })
                .filter(|value| value.is_finite())
                .ok_or_else(|| anyhow::anyhow!("字段 {} 必须是数字", field.key))?;
            if field.min.is_some_and(|min| parsed < min)
                || field.max.is_some_and(|max| parsed > max)
            {
                anyhow::bail!("字段 {} 超出允许范围", field.key);
            }
            Value::Number(
                serde_json::Number::from_f64(parsed)
                    .ok_or_else(|| anyhow::anyhow!("字段 {} 不是有效数字", field.key))?,
            )
        }
        _ => anyhow::bail!("字段 {} 使用了不支持的类型", field.key),
    };
    if field.setting_type == "select" && !field.options.iter().any(|option| option.value == coerced)
    {
        anyhow::bail!("字段 {} 的值不在 options 中", field.key);
    }
    Ok(coerced)
}

fn setting_is_present(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::String(value)) => !value.trim().is_empty(),
        Some(_) => true,
    }
}

fn read_plugin_settings_raw(
    plugin_name: &str,
) -> anyhow::Result<(PluginManifest, PathBuf, BTreeMap<String, Value>)> {
    let normalized = normalize_plugin_name(plugin_name)?;
    let plugin_root = resolve_plugin_root(&normalized)
        .ok_or_else(|| anyhow::anyhow!("插件未安装：{normalized}"))?;
    let manifest = read_plugin_manifest(&plugin_root)?;
    let config_path = plugin_settings_path(&normalized)?;
    let stored = if config_path.is_file() {
        serde_json::from_slice::<StoredPluginSettings>(
            &fs::read(&config_path)
                .with_context(|| format!("读取插件设置失败：{}", config_path.display()))?,
        )
        .with_context(|| format!("解析插件设置失败：{}", config_path.display()))?
    } else {
        StoredPluginSettings::default()
    };

    let mut values = BTreeMap::new();
    for field in &manifest.settings.fields {
        if let Some(value) = stored.values.get(&field.key) {
            if let Ok(value) = coerce_setting_value(field, value) {
                if !value.is_null() {
                    values.insert(field.key.clone(), value);
                    continue;
                }
            }
        }
        if !field.default_value.is_null() {
            let value = coerce_setting_value(field, &field.default_value)?;
            values.insert(field.key.clone(), value);
        }
    }
    Ok((manifest, config_path, values))
}

fn settings_snapshot(
    manifest: &PluginManifest,
    config_path: PathBuf,
    values: &BTreeMap<String, Value>,
) -> PluginSettingsSnapshot {
    let secret_keys = manifest
        .settings
        .fields
        .iter()
        .filter(|field| field.setting_type == "secret")
        .map(|field| field.key.clone())
        .collect::<HashSet<_>>();
    let secret_keys_set = secret_keys
        .iter()
        .filter(|key| setting_is_present(values.get(*key)))
        .cloned()
        .collect::<Vec<_>>();
    let missing_required = manifest
        .settings
        .fields
        .iter()
        .filter(|field| field.required && !setting_is_present(values.get(&field.key)))
        .map(|field| field.key.clone())
        .collect::<Vec<_>>();
    let redacted_values = values
        .iter()
        .filter(|(key, _)| !secret_keys.contains(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    PluginSettingsSnapshot {
        plugin_id: manifest.name.clone(),
        values: redacted_values,
        secret_keys_set,
        configured: missing_required.is_empty(),
        missing_required,
        config_path,
    }
}

pub fn read_plugin_settings(plugin_name: &str) -> anyhow::Result<PluginSettingsSnapshot> {
    let (manifest, config_path, values) = read_plugin_settings_raw(plugin_name)?;
    Ok(settings_snapshot(&manifest, config_path, &values))
}

pub fn save_plugin_settings(
    plugin_name: &str,
    patch: &Value,
    clear_secret_keys: &[String],
) -> anyhow::Result<PluginSettingsSnapshot> {
    let (manifest, config_path, mut values) = read_plugin_settings_raw(plugin_name)?;
    let fields = manifest
        .settings
        .fields
        .iter()
        .map(|field| (field.key.as_str(), field))
        .collect::<BTreeMap<_, _>>();
    let patch = patch
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("values 必须是对象"))?;
    for (key, value) in patch {
        let field = fields
            .get(key.as_str())
            .ok_or_else(|| anyhow::anyhow!("未知插件设置字段：{key}"))?;
        if field.setting_type == "secret" && value.as_str().is_some_and(|value| value.is_empty()) {
            continue;
        }
        let value = coerce_setting_value(field, value)?;
        if value.is_null() {
            values.remove(key);
        } else {
            values.insert(key.clone(), value);
        }
    }
    for key in clear_secret_keys {
        if fields
            .get(key.as_str())
            .is_some_and(|field| field.setting_type == "secret")
        {
            values.remove(key);
        }
    }

    let missing_required = manifest
        .settings
        .fields
        .iter()
        .filter(|field| field.required && !setting_is_present(values.get(&field.key)))
        .map(|field| field.label.clone())
        .collect::<Vec<_>>();
    if !missing_required.is_empty() {
        anyhow::bail!("以下必填设置尚未填写：{}", missing_required.join("、"));
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stored = StoredPluginSettings {
        version: 1,
        plugin_id: manifest.name.clone(),
        values: values.clone(),
        updated_at: now_millis(),
    };
    let temporary = config_path.with_extension(format!("json.tmp-{}", now_millis()));
    fs::write(&temporary, serde_json::to_vec_pretty(&stored)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    if config_path.exists() {
        fs::remove_file(&config_path)?;
    }
    fs::rename(&temporary, &config_path)?;
    Ok(settings_snapshot(&manifest, config_path, &values))
}

pub fn plugin_runtime_settings_env(plugin_name: &str) -> anyhow::Result<HashMap<String, String>> {
    let (manifest, config_path, values) = read_plugin_settings_raw(plugin_name)?;
    let mut env = HashMap::new();
    env.insert("JIUBAN_PLUGIN_ID".into(), manifest.name.clone());
    env.insert(
        "JIUBAN_PLUGIN_SETTINGS_FILE".into(),
        config_path.to_string_lossy().into_owned(),
    );
    for field in &manifest.settings.fields {
        if field.env.trim().is_empty() {
            continue;
        }
        if let Some(value) = values.get(&field.key).and_then(setting_value_to_env) {
            env.insert(field.env.clone(), value);
        }
    }
    Ok(env)
}

pub fn remove_installed_plugin(plugin_name: &str) -> anyhow::Result<bool> {
    let normalized = normalize_plugin_name(plugin_name)?;
    let root = plugin_install_root().join(normalized);
    if !root.is_dir() || manifest_path(&root).is_none() {
        return Ok(false);
    }
    fs::remove_dir_all(root)?;
    Ok(true)
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn normalizes_plugin_names() {
        assert_eq!(
            normalize_plugin_name("Open Montage").unwrap(),
            "open-montage"
        );
        assert!(normalize_plugin_name("..").is_err());
    }

    #[test]
    fn encodes_digest_bytes_as_lowercase_hex() {
        assert_eq!(digest_hex([0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    #[test]
    fn explicit_builtin_plugin_root_is_authoritative() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let explicit_root = temp.path().join("builtin-plugins");
        fs::create_dir_all(&explicit_root).unwrap();

        let previous = std::env::var_os("JIUBAN_BUILTIN_PLUGINS_ROOT");
        std::env::set_var("JIUBAN_BUILTIN_PLUGINS_ROOT", &explicit_root);
        let roots = builtin_plugin_roots();
        if let Some(previous) = previous {
            std::env::set_var("JIUBAN_BUILTIN_PLUGINS_ROOT", previous);
        } else {
            std::env::remove_var("JIUBAN_BUILTIN_PLUGINS_ROOT");
        }

        assert_eq!(roots, vec![explicit_root.canonicalize().unwrap()]);
    }

    #[test]
    fn repeated_install_reuses_unchanged_files() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let plugin_home = temp.path().join("plugins");
        fs::create_dir_all(source.join(".plugin")).unwrap();
        fs::write(
            source.join(".plugin/plugin.json"),
            r#"{"name":"demo","version":"1.0.0","runtime":{"type":"python","manager":"uv","mcp":"mcp/server.py"}}"#,
        )
        .unwrap();
        fs::write(source.join("README.md"), "demo\n").unwrap();

        let previous = std::env::var_os("JIUBAN_PLUGIN_HOME");
        std::env::set_var("JIUBAN_PLUGIN_HOME", &plugin_home);
        let first = install_plugin_directory(&source, "test").unwrap();
        let second = install_plugin_directory(&source, "test").unwrap();
        if let Some(previous) = previous {
            std::env::set_var("JIUBAN_PLUGIN_HOME", previous);
        } else {
            std::env::remove_var("JIUBAN_PLUGIN_HOME");
        }

        assert!(first.installed);
        assert!(first.copied_files >= 2);
        assert!(!second.installed);
        assert_eq!(second.copied_files, 0);
        assert_eq!(second.reused_files, first.copied_files);
    }

    #[test]
    fn plugin_settings_are_validated_redacted_and_exported_to_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let plugin_home = temp.path().join("plugins");
        let settings_home = temp.path().join("plugin-settings");
        fs::create_dir_all(source.join(".plugin")).unwrap();
        fs::write(
            source.join(".plugin/plugin.json"),
            r#"{
              "name":"demo",
              "version":"1.0.0",
              "settings":{
                "version":1,
                "fields":[
                  {"key":"model","label":"Model","type":"string","default":"v1","env":"DEMO_MODEL"},
                  {"key":"token","label":"Token","type":"secret","required":true,"env":"DEMO_TOKEN"}
                ]
              }
            }"#,
        )
        .unwrap();

        let previous_plugin_home = std::env::var_os("JIUBAN_PLUGIN_HOME");
        let previous_settings_home = std::env::var_os("JIUBAN_PLUGIN_SETTINGS_HOME");
        std::env::set_var("JIUBAN_PLUGIN_HOME", &plugin_home);
        std::env::set_var("JIUBAN_PLUGIN_SETTINGS_HOME", &settings_home);
        install_plugin_directory(&source, "test").unwrap();
        let snapshot = save_plugin_settings(
            "demo",
            &serde_json::json!({ "model": "v2", "token": "secret-value" }),
            &[],
        )
        .unwrap();
        let env = plugin_runtime_settings_env("demo").unwrap();

        if let Some(previous) = previous_plugin_home {
            std::env::set_var("JIUBAN_PLUGIN_HOME", previous);
        } else {
            std::env::remove_var("JIUBAN_PLUGIN_HOME");
        }
        if let Some(previous) = previous_settings_home {
            std::env::set_var("JIUBAN_PLUGIN_SETTINGS_HOME", previous);
        } else {
            std::env::remove_var("JIUBAN_PLUGIN_SETTINGS_HOME");
        }

        assert!(snapshot.configured);
        assert_eq!(snapshot.values.get("model"), Some(&serde_json::json!("v2")));
        assert!(!snapshot.values.contains_key("token"));
        assert_eq!(snapshot.secret_keys_set, vec!["token"]);
        assert_eq!(env.get("DEMO_MODEL").map(String::as_str), Some("v2"));
        assert_eq!(
            env.get("DEMO_TOKEN").map(String::as_str),
            Some("secret-value")
        );
        assert!(snapshot.config_path.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&snapshot.config_path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn openmontage_builtin_manifest_exposes_nine_settings_fields() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("builtin-plugins/openmontage");
        let manifest = read_plugin_manifest(&root).unwrap();
        assert_eq!(manifest.name, "openmontage");
        assert_eq!(manifest.version, "0.3.2");
        assert_eq!(manifest.settings.fields.len(), 9);
        let env_names = manifest
            .settings
            .fields
            .iter()
            .map(|field| field.env.as_str())
            .collect::<HashSet<_>>();
        assert!(env_names.contains("OPENMONTAGE_LANGUAGE"));
        assert!(env_names.contains("OPENMONTAGE_DEFAULT_ASPECT_RATIO"));
        assert!(env_names.contains("OPENMONTAGE_DEFAULT_RESOLUTION"));
        assert!(env_names.contains("OPENMONTAGE_MCP_TIMEOUT_MS"));
    }
}
