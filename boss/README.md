# 商媒运营助手 · 老板指挥台

<p align="center">
  <img src="../branding/cohmira/cohmira-command.svg" alt="商媒运营助手 · 老板指挥台" width="720" />
</p>

**语言：简体中文 | [English](./README_EN.md)**

`boss/` 是商媒运营助手的老板端。它面向老板、管理者和财务负责人，用统一界面查看员工每周工作、经营数据、账本、发票与 Actual Budget 同步状态。

[返回平台总览](../README.md) · [员工端说明](../src/README.md)

## 当前架构

老板端采用 **Tauri 2 + Rust + TypeScript/Vite**：

- `desktop/src-tauri/` 是唯一的本地后端和桌面运行核心。账本、员工周报、老板审阅、发票草稿、Actual 导出/同步作业和 SQLite 访问全部由 Rust 实现。
- `desktop/src/` 是共用的 WebView/浏览器界面；Tauri 桌面版与浏览器开发版复用同一套页面和 Rust 工具契约。
- Tauri 桌面进程会用同一个 `BossCore` 自动启动 Rust HTTP 服务；`desktop/src-tauri/src/bin/boss-server.rs` 是浏览器开发和独立部署入口，二者都提供 `POST /api/boss/tool`。
- SQLite 通过 `rusqlite` 的 bundled SQLite 构建，不要求用户单独安装 SQLite，也不要求 Python。
- Tauri 2 负责 macOS 和 Windows 桌面打包；仓库内旧 `electron/`、`dist-electron/` 文件仅为历史遗留，不是当前老板端运行入口。
- 旧 Python `server.py` 已从仓库移除；开发、测试、桌面运行和安装包都只使用 Rust 后端。

## 支持功能

| 模块 | 当前能力 | 状态说明 |
| --- | --- | --- |
| 今日总览 | 待老板处理事项、本周团队进度、经营净额、账务待复核和 AI 管理提示 | Rust 工具驱动 |
| 老板 AI | 调用 `boss.owner_context` 读取本周员工、账本、发票、绑定和 Actual 状态，再生成结构化管理摘要 | 当前是规则化摘要，独立模型推理待接入 |
| 员工周报 | 周区间切换、员工进度、工作事件、素材、质量、成本、阻塞、下周计划和老板审阅 | 员工任务中心已能显式上报已完成任务并读取老板反馈；离线队列与正式账号体系待完成 |
| 经营账本 | SQLite 流水加载、手工新增、待复核确认、月度净额、分类和分页筛选 | Rust + SQLite，本机持久化 |
| 发票与凭证 | 解析票据文本或外部 `ai_ocr_json`、保存草稿并转为账本交易 | 图片/PDF 二进制安全落盘并标记待 OCR，不猜测字段 |
| Actual Budget | 集成状态、dry-run、可审计同步作业和 CSV/JSONL 导出 | 可选集成；Rust 不内嵌 Actual Server |
| 员工绑定 | 老板码、绑定数据结构和查询工具 | 正式账号、组织授权和远端绑定 API 待完成 |
| 桌面端 | Tauri 2 原生窗口和系统应用数据目录 | 支持从源码构建 macOS、Windows 版本 |

## 与员工端的关系

员工端 `../src/` 负责真实执行，老板端负责检查、审阅和经营管理。Rust 核心已提供以下闭环工具契约：

- `employee.report_work_event`：员工上报任务、素材、质量、成本和异常事件。
- `boss.employee_work_report`：老板按员工与周区间读取工作记录。
- `boss.save_week_review`：老板保存周审阅和补充要求。
- `employee.list_week_reviews`：员工读取老板审阅。

员工端 `../src/` 的任务中心已经调用这组契约：只有具备成功执行记录的真实任务可以由员工显式上报，草稿、失败、取消和未执行任务不会自动提交；员工也可以读取本周老板反馈。上报使用稳定事件 ID，多次提交会幂等更新同一条记录。

同机使用时，老板桌面自动监听 `127.0.0.1:8787`。跨设备使用时必须通过 HTTPS 反向代理访问 Rust server，并配置两个不同且至少 32 字节的令牌：`BOSS_SERVER_TOKEN` 拥有老板工具权限，`BOSS_EMPLOYEE_TOKEN` 只能调用 `employee.*`。当前仍缺正式账号、按员工签发的独立凭据、组织授权、离线队列、消息推送和完整冲突处理，因此不能把固定老板码和演示登录视为生产认证。

## 目录结构

```text
boss/
├── desktop/
│   ├── src/                         # TypeScript/Vite 共用界面
│   ├── src-tauri/
│   │   ├── src/lib.rs               # BossCore 与工具分发
│   │   ├── src/main.rs              # Tauri 2 入口与 call_boss_tool 命令
│   │   ├── src/bin/boss-server.rs   # 浏览器开发用 Rust HTTP server
│   │   ├── Cargo.toml
│   │   └── tauri.conf.json
│   └── package.json
├── mcps/boss-accounting-mcp/        # 旧本地数据与配置迁移说明
├── third_party/actual/               # 可选 Actual Budget 源码
├── docs/
│   ├── boss-product-plan.md
│   ├── accounting-open-source-research.md
│   └── actual-env-setup.md
└── README.md
```

## 环境要求

- Node.js 22 和 npm。
- Rust 1.88 或更高版本与 Cargo（以 `desktop/src-tauri/Cargo.toml` 的 `rust-version` 为准）。
- Tauri 2 的系统依赖：macOS 需要 Xcode Command Line Tools；Windows 需要 Microsoft C++ Build Tools 和 WebView2。
- 可选：Actual Budget Server、Sync ID、密码或令牌、默认账户。

老板端运行、测试和打包均不需要 Python。

## 安装前端依赖

```bash
cd boss/desktop
npm ci
```

## 启动桌面版

macOS 与 Windows 均使用同一命令：

```bash
cd boss/desktop
npm run tauri:dev
```

它会启动 Vite 页面、编译 Rust 核心并打开 Tauri 2 原生窗口。桌面运行时直接调用 `call_boss_tool`，不会启动 Python 服务。

## 浏览器开发

浏览器模式仍使用同一个 Rust 核心，但需要分别启动 Rust server 和 Vite：

终端 1：

```bash
cd boss/desktop/src-tauri
cargo run --bin boss-server
```

终端 2：

```bash
cd boss/desktop
npm run dev
```

Rust server 默认监听 `127.0.0.1:8787`。然后打开 `http://127.0.0.1:5181`；Vite 把 `/api/boss/tool` 代理到本地 Rust server。可通过 `BOSS_SERVER_URL` 覆盖代理目标，通过 `BOSS_SERVER_ADDR` 覆盖 server 监听地址。

## 员工端同步

老板桌面启动后，同机员工端可直接连接 `http://127.0.0.1:8787`。员工端需要配置 `BOSS_OWNER_CODE`、`EMPLOYEE_ID`、`EMPLOYEE_NAME`，可选配置 `EMPLOYEE_ROLE`；具体命令见 [`../src/README.md`](../src/README.md)。

跨设备部署时，Rust server 本身只提供 HTTP，应放在受信任的 HTTPS 反向代理后，并显式配置：

```text
BOSS_SERVER_ADDR=0.0.0.0:8787
BOSS_ALLOW_REMOTE=1
BOSS_SERVER_TOKEN=<至少 32 字节的老板随机令牌>
BOSS_EMPLOYEE_TOKEN=<另一个至少 32 字节的员工随机令牌>
```

员工端只配置 `BOSS_EMPLOYEE_TOKEN`，不得持有老板令牌。服务端会对无令牌请求返回 401，并拒绝员工令牌访问账本、发票和老板工具。远程员工端还会拒绝明文 HTTP 地址。

## 构建 macOS / Windows 桌面安装包

在目标操作系统上执行：

```bash
cd boss/desktop
npm ci
npm run tauri:build
```

Tauri 会在 `desktop/src-tauri/target/release/bundle/` 下生成当前平台的安装产物。macOS 与 Windows 应分别在对应系统或对应 CI runner 上构建；正式分发还需要配置各平台的代码签名、公证和发布证书。

`.github/workflows/boss-desktop-check.yml` 会在 `macos-14` 和 `windows-2022` 上执行前端构建、Rust 测试，并分别生成无签名 `.app` 与 NSIS 安装包作为 CI 制品。

## Rust 测试

```bash
cd boss/desktop/src-tauri
cargo test
```

需要检查所有目标能否编译时再执行：

```bash
cargo check --all-targets
```

## 数据位置

Tauri 桌面版默认把 `boss-accounting.sqlite` 放在操作系统分配的应用数据目录，浏览器开发 server 使用平台数据目录。开发或测试时可显式覆盖：

```bash
BOSS_ACCOUNTING_DB=/absolute/path/boss-accounting.sqlite cargo run --bin boss-server
```

不要把真实 SQLite 数据库、票据原文件、导出文件或 Actual 凭据提交到版本库。

## 配置 Actual Budget（可选）

不配置 Actual 时，本地 SQLite 账本、发票草稿、报表和导出仍可使用。真实连接通常需要：

```text
ACTUAL_SERVER_URL
ACTUAL_SYNC_ID
ACTUAL_PASSWORD 或 ACTUAL_TOKEN
ACTUAL_DEFAULT_ACCOUNT_ID
```

Rust 核心只管理集成状态、审批门禁、导入文件和可审计同步作业；它不会内嵌或偷偷修改 Actual Server。只有已审批交易可以进入同步，未完成配置时返回 dry-run 或 queued 状态。完整步骤见 [docs/actual-env-setup.md](./docs/actual-env-setup.md)。

## 发票识别边界

- 可直接解析文本票据以及外部 OCR/视觉服务返回的结构化 `ai_ocr_json`。
- 图片/PDF 等二进制会安全保存并标记为待 OCR/待复核，不依赖 Python 或 Tesseract。
- 未识别字段必须留空，人工复核后才能正式入账或同步 Actual。
- 后续可接入本地 Rust OCR 库或外部视觉模型，但适配器必须保留来源、置信度和审计记录。

## 当前待完成项

- 接入生产级登录、账号、组织、按员工签发凭据、老板码授权和租户隔离。
- 在现有显式上报与反馈读取之上增加离线队列、自动重试、冲突处理和消息推送。
- 将老板审阅和管理决策通过生产同步通道主动推送到员工端。
- 接入稳定的图片/PDF OCR 或视觉模型适配器。
- 在现有双平台编译 CI 上接入 macOS 签名/公证、Windows 签名和安装包发布。
- 对 Actual 真实写入继续完善连接器、失败重试和审计日志。

## 数据与安全

- 老板 AI 回答员工、收入、支出、利润、发票或 Actual 问题前必须调用对应 Rust 工具。
- 工具未返回的数据不得由 AI 推测；所有待确认内容都应进入 `needs_review`。
- 生产环境不能继续使用固定老板码或前端演示登录，必须接入正式认证、授权和租户隔离。
- Rust server 的远程模式已强制 Bearer 分权认证，但仍必须部署 TLS、限流、访问审计和正式的按员工凭据轮换机制。
