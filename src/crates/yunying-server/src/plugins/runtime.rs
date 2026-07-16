use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use crate::goose_bridge::{GooseBridge, StdioMcpEnvironment};

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn find_on_path(executable: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(executable))
        .find(|candidate| candidate.is_file())
}

fn resolve_uv_bin() -> anyhow::Result<PathBuf> {
    for key in ["JIUBAN_UV_BIN", "OPENMONTAGE_UV", "UV_BIN"] {
        if let Some(path) = std::env::var_os(key).map(PathBuf::from) {
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    let executable = if cfg!(windows) { "uv.exe" } else { "uv" };
    find_on_path(executable)
        .ok_or_else(|| anyhow::anyhow!("未找到 uv；打包资源缺少 bin/{executable}"))
}

fn resolve_python_bin() -> Option<PathBuf> {
    for key in ["JIUBAN_PYTHON_BIN", "OPENMONTAGE_PYTHON", "PYTHON_BIN"] {
        if let Some(path) = std::env::var_os(key).map(PathBuf::from) {
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn runtime_executable_name(base: &str, windows: bool) -> String {
    if windows {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

pub async fn mount_openmontage(goose: &GooseBridge, settings: &Value) -> anyhow::Result<PathBuf> {
    let plugin_root = super::resolve_plugin_root("openmontage")
        .ok_or_else(|| anyhow::anyhow!("OpenMontage 插件未安装"))?;
    let manifest = super::manager::read_plugin_manifest(&plugin_root)?;
    if manifest.runtime.manager != "uv" {
        anyhow::bail!("OpenMontage runtime.manager 必须是 uv");
    }
    let python_version = if manifest.runtime.python.trim().is_empty() {
        "3.11"
    } else {
        manifest.runtime.python.trim()
    };
    let requirements = plugin_root.join(if manifest.runtime.requirements.trim().is_empty() {
        "requirements.txt"
    } else {
        manifest.runtime.requirements.trim()
    });
    let mcp_entry = plugin_root.join(&manifest.runtime.mcp);
    if !requirements.is_file() {
        anyhow::bail!(
            "OpenMontage requirements 不存在：{}",
            requirements.display()
        );
    }
    if !mcp_entry.is_file() {
        anyhow::bail!("OpenMontage MCP 入口不存在：{}", mcp_entry.display());
    }

    let runtime_root = home_dir().join(".agents").join("runtime");
    let workspace = crate::workspace::resolve(settings);
    let mut envs = HashMap::new();
    envs.insert(
        "YUNYINGAGENT_ROOT".into(),
        workspace.base.to_string_lossy().into_owned(),
    );
    envs.insert(
        "OPENMONTAGE_ROOT".into(),
        plugin_root.to_string_lossy().into_owned(),
    );
    envs.insert(
        "OPENMONTAGE_PLUGIN_ROOT".into(),
        plugin_root.to_string_lossy().into_owned(),
    );
    envs.insert(
        "OPENMONTAGE_PROJECTS_DIR".into(),
        workspace
            .base
            .join("openmontage-projects")
            .to_string_lossy()
            .into_owned(),
    );
    envs.insert(
        "OPENMONTAGE_PYTHON_VERSION".into(),
        python_version.to_string(),
    );
    envs.insert(
        "UV_CACHE_DIR".into(),
        runtime_root.join("uv-cache").to_string_lossy().into_owned(),
    );
    envs.insert(
        "UV_PYTHON_INSTALL_DIR".into(),
        runtime_root.join("python").to_string_lossy().into_owned(),
    );
    envs.insert("UV_LINK_MODE".into(), "copy".into());
    envs.insert("BEAV_BRIDGE_TIMEOUT_MS".into(), "1800000".into());
    envs.insert(
        "BEAV_MEDIA_ROOT".into(),
        workspace.media.to_string_lossy().into_owned(),
    );
    envs.insert("OPENMONTAGE_AGENT_ROLE".into(), "video-subagent".into());
    envs.insert("OPENMONTAGE_MCP_TIMEOUT_MS".into(), "1800000".into());
    envs.insert("PYTHONIOENCODING".into(), "utf-8".into());
    envs.insert("PYTHONUTF8".into(), "1".into());
    envs.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
    if let Some(ffmpeg_dir) = std::env::var_os("JIUBAN_FFMPEG_DIR").map(PathBuf::from) {
        let ffmpeg_name = runtime_executable_name("ffmpeg", cfg!(windows));
        let ffprobe_name = runtime_executable_name("ffprobe", cfg!(windows));
        envs.insert(
            "FFMPEG_BIN".into(),
            ffmpeg_dir.join(ffmpeg_name).to_string_lossy().into_owned(),
        );
        envs.insert(
            "FFPROBE_BIN".into(),
            ffmpeg_dir.join(ffprobe_name).to_string_lossy().into_owned(),
        );
        let mut path_entries = vec![ffmpeg_dir.clone()];
        if let Some(existing) = std::env::var_os("PATH") {
            path_entries.extend(std::env::split_paths(&existing));
        }
        if let Ok(path) = std::env::join_paths(path_entries) {
            envs.insert("PATH".into(), path.to_string_lossy().into_owned());
        }
        #[cfg(target_os = "macos")]
        {
            let mut library_entries = vec![ffmpeg_dir];
            if let Some(existing) = std::env::var_os("DYLD_LIBRARY_PATH") {
                library_entries.extend(std::env::split_paths(&existing));
            }
            if let Ok(path) = std::env::join_paths(library_entries) {
                envs.insert(
                    "DYLD_LIBRARY_PATH".into(),
                    path.to_string_lossy().into_owned(),
                );
            }
        }
    }
    envs.extend(super::plugin_runtime_settings_env("openmontage")?);
    // 连接信息由当前主进程为本次启动生成。只把变量名写入 Goose 配置，
    // 避免随机 bearer/端口进入持久化会话元数据。
    let mut env_keys = Vec::new();
    for key in [
        super::bridge::OPENMONTAGE_BRIDGE_URL_ENV,
        super::bridge::OPENMONTAGE_BRIDGE_PATH_ENV,
        super::bridge::OPENMONTAGE_BRIDGE_TOKEN_ENV,
    ] {
        let value = std::env::var(key)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("OpenMontage 本地桥环境缺失：{key}"))?;
        drop(value);
        envs.remove(key);
        env_keys.push(key.into());
    }
    if std::env::var("YUNYING_CONFIG_PATH").is_ok_and(|value| !value.trim().is_empty()) {
        envs.remove("YUNYING_CONFIG_PATH");
        env_keys.push("YUNYING_CONFIG_PATH".into());
    }
    let (command, args) = if let Some(python) = resolve_python_bin() {
        (
            python,
            vec!["-u".into(), mcp_entry.to_string_lossy().into_owned()],
        )
    } else {
        let uv = resolve_uv_bin()?;
        (
            uv,
            vec![
                "run".into(),
                "--managed-python".into(),
                "--python".into(),
                python_version.into(),
                "--no-project".into(),
                "--with-requirements".into(),
                requirements.to_string_lossy().into_owned(),
                mcp_entry.to_string_lossy().into_owned(),
            ],
        )
    };
    goose
        .register_stdio_mcp(
            "openmontage",
            &command.to_string_lossy(),
            args,
            StdioMcpEnvironment {
                values: envs,
                inherited_keys: env_keys,
            },
            "OpenMontage Creative Pipeline Tools MCP bridge",
            900,
        )
        .await?;
    Ok(plugin_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_names_include_windows_suffix_only_on_windows() {
        assert_eq!(runtime_executable_name("ffmpeg", true), "ffmpeg.exe");
        assert_eq!(runtime_executable_name("ffprobe", true), "ffprobe.exe");
        assert_eq!(runtime_executable_name("ffmpeg", false), "ffmpeg");
    }
}
