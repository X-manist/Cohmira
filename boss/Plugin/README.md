# 商媒运营助手 Chrome 插件

这个目录是 Boss 端副本里的浏览器插件说明。当前正式插件源码和验收入口以员工端为准：

```text
../../src/Plugin
```

插件负责把小红书、YouTube、公众号、普通网页、网页图片和选中文字采集到商媒运营助手桌面端知识库和素材库。AI 编排和业务决策仍在桌面端完成。

## 加载方式

```bash
cd /Volumes/macsoftware/codes/agentscompany/yunyingagent/src/Plugin
pnpm install
pnpm build
pnpm verify
```

然后在 Chrome 或 Edge 中：

1. 打开 `chrome://extensions` 或 `edge://extensions`。
2. 开启开发者模式。
3. 点击“加载已解压的扩展程序”。
4. 选择 `src/Plugin/dist/extension`。

## 验收边界

- 商媒运营助手桌面端必须已经启动。
- 插件采集后，桌面端本地桥 `http://127.0.0.1:23456/status` 应返回 `{"status":"ok"}`。
- 真实浏览器控制验收必须看到 MCP / native host socket 返回 `tools/list`、`tabs.list`、`tab.info`、DOM 查询和至少一个交互动作。
- 采集任务成功不等于运营链路完成；端到端测试仍以 `../../src/README.md` 里的 macOS 流程为准。
