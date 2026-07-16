# Built-in plugins

这里是商媒运营助手随应用发布的插件源码目录。每个一级子目录都是一个完整插件，必须包含
`.plugin/plugin.json`，并把运行所需的源码、skills、docs、schemas 和资源放在插件目录内。

桌面端会把这里的插件同步到用户插件目录 `~/.agents/plugins/`。Python 插件不携带
Python 或 site-packages；运行时统一使用应用内置的 uv，按插件 requirements 安装并缓存。

插件需要用户配置时，在 manifest 的 `settings.fields` 中声明字段即可。桌面端会在
“设置 → 工具设置 → 插件工具设置”自动渲染并独立保存配置。协议说明见
[`PLUGIN_SETTINGS_PROTOCOL.md`](./PLUGIN_SETTINGS_PROTOCOL.md)。
