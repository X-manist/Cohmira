//! 九伴插件运行时。
//!
//! 插件本体统一安装到 `~/.agents/plugins/<plugin-id>`；内置插件从应用资源目录或
//! workspace 的 `builtin-plugins/` 增量同步。插件可以携带 skills、文档、Python 工具、
//! MCP 入口与静态资源，不要求主程序把这些内容重新编译成 Rust。

pub mod bridge;
pub mod manager;
pub mod runtime;

pub use manager::{
    all_plugin_roots, builtin_plugin_roots, install_plugin_directory, list_installed_plugins,
    plugin_install_root, plugin_runtime_settings_env, plugin_settings_path, plugin_settings_root,
    read_plugin_settings, remove_installed_plugin, resolve_plugin_root, save_plugin_settings,
    sync_builtin_plugins, PluginInfo, PluginInstallResult, PluginManifest, PluginSettingField,
    PluginSettingGroup, PluginSettingOption, PluginSettingsManifest, PluginSettingsSnapshot,
};
pub use runtime::mount_openmontage;
