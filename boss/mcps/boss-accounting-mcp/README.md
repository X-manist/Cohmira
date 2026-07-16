# Boss Accounting 旧数据迁移说明

老板端早期 Python MCP/CLI 后端已经移除。本目录仅保留旧本地 SQLite、忽略配置和数据迁移说明，不提供可运行服务，也不是任何前端请求的后端。

老板端不要求安装 Python、Python SQLite 驱动、`pdftotext` 或 Tesseract。

## 当前实现位置

唯一有效的本地后端位于：

```text
boss/desktop/src-tauri/
├── src/lib.rs               # BossCore、SQLite 与工具分发
├── src/main.rs              # Tauri 2 桌面命令
└── src/bin/boss-server.rs   # 浏览器开发 Rust server
```

Rust 核心使用 `rusqlite` bundled SQLite，并由两种入口复用：

- Tauri 2 桌面端调用 `call_boss_tool`，并自动启动同一个 Rust HTTP 服务供员工端连接。
- 浏览器开发端调用 `POST /api/boss/tool`。

两种入口返回相同的结构化工具结果，不通过 Python 桥接。

## 开发与测试

桌面开发：

```bash
cd boss/desktop
npm ci
npm run tauri:dev
```

macOS / Windows 构建：

```bash
npm run tauri:build
```

Rust 测试：

```bash
cd boss/desktop/src-tauri
cargo test
```

浏览器开发 server：

```bash
cd boss/desktop/src-tauri
cargo run --bin boss-server
```

默认监听 `127.0.0.1:8787`。再在 `boss/desktop` 运行 `npm run dev`，Vite 会把 `/api/boss/tool` 代理到 Rust server。

## Rust 工具能力

Rust 核心承接原型中的老板上下文、员工周报、账本、发票和 Actual 能力，并新增老板/员工本机审阅闭环：

| 工具 | 用途 |
| --- | --- |
| `boss.dashboard_summary` | 老板视角的员工、经营与待处理摘要 |
| `boss.owner_context` | 老板 AI 的员工、账本、发票、绑定和 Actual 全上下文 |
| `boss.employee_work_report` | 按员工与周区间读取工作记录 |
| `employee.report_work_event` | 员工写入工作事件 |
| `boss.save_week_review` | 老板保存周审阅和补充要求 |
| `employee.list_week_reviews` | 员工读取老板审阅 |
| `ledger.*` | 流水新增、列表、审批与报表 |
| `invoice.*` | 文本/外部 OCR 结果解析、草稿和入账 |
| `actual.*` | 集成状态、审批门禁、导出和可审计同步作业 |
| `owner.*` | 老板码和员工绑定数据 |

工具返回 `null`、`unknown` 或 `needs_review` 的字段时，AI 不得自行补全。

## 旧数据与配置

- 本目录内旧 `.env`、`boss-accounting.config.json` 和 `data/boss_accounting.sqlite` 不再被默认运行入口读取。
- Rust 桌面版默认使用操作系统 app-data 下的 `boss-accounting.sqlite`；可用 `BOSS_ACCOUNTING_DB` 显式覆盖。
- 如需验证旧数据库兼容性，应先备份，再把 `BOSS_ACCOUNTING_DB` 指向副本；不要直接修改唯一的生产数据文件。
- Actual 的新环境变量流程见 [`boss/docs/actual-env-setup.md`](../../docs/actual-env-setup.md)。

## 尚未完成的生产能力

员工端任务中心已支持显式上报真实已完成任务和读取本周老板反馈，但以下生产能力仍待完成：

- 正式登录、按员工签发凭据、组织授权与租户隔离。
- 离线队列、自动重试、冲突处理和消息通知。
- HTTPS 反向代理、限流、审计与凭据轮换的标准部署。

在这些能力完成前，固定老板码和本地种子数据只用于开发验证。
