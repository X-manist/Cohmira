# 商媒运营助手插件设置协议 v1

插件不需要为常规配置重复开发桌面 UI。只要在 `.plugin/plugin.json` 中声明
`settings`，商媒运营助手就会在“设置 → 工具设置 → 插件工具设置”生成表单。

```json
{
  "settings": {
    "version": 1,
    "title": "示例插件",
    "description": "插件设置说明",
    "fields": [
      {
        "key": "provider.model",
        "label": "模型",
        "type": "select",
        "default": "model-a",
        "options": [
          { "label": "Model A", "value": "model-a" },
          { "label": "Model B", "value": "model-b" }
        ],
        "env": "EXAMPLE_MODEL"
      },
      {
        "key": "provider.apiKey",
        "label": "API Key",
        "type": "secret",
        "required": true,
        "env": "EXAMPLE_API_KEY"
      }
    ]
  }
}
```

## 字段

支持的 `type`：

- `string`：单行文本
- `multiline`：多行文本
- `secret`：密码输入；读取设置时不会把已保存值返回给渲染进程
- `number`：小数
- `integer`：整数
- `boolean`：开关
- `select`：下拉选择，必须提供 `options`
- `path`：文件或目录路径文本

通用属性包括 `key`、`label`、`description`、`default`、`required`、
`placeholder`、`advanced`、`min`、`max`、`step`、`options` 和 `env`。

`env` 非空时，主应用会在启动插件运行时前把保存值注入对应环境变量。插件同时会收到：

- `JIUBAN_PLUGIN_ID`
- `JIUBAN_PLUGIN_SETTINGS_FILE`

配置文件位于 `~/.agents/plugin-settings/<plugin-id>.json`，与插件源码分离，因此插件增量更新或
重新安装不会覆盖用户配置。敏感字段不会回传到 UI；配置文件在 Unix 系统上使用 `0600`
权限。需要系统钥匙串、OAuth、扫码登录或复杂交互的插件，可以再提供独立的自定义设置页面。

## UI 选择原则

常规参数优先使用本协议自动渲染。只有以下情况才需要插件自定义 UI：

- OAuth、扫码登录、设备授权
- 需要实时预览或图形化编辑器
- 字段之间存在复杂联动
- 需要插件自己的 WebView/MCP App 工作台

自定义 UI 不应替代 manifest 中的基础字段声明；桌面端仍需要依靠基础字段展示配置状态、
迁移配置以及在插件未启动时修改设置。
