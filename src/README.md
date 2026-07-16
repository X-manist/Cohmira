# 商媒运营助手 · 员工工作台

<p align="center">
  <img src="../branding/cohmira/cohmira-workspace.svg" alt="商媒运营助手 · 员工工作台" width="720" />
</p>

**语言：简体中文 | [English](./README_EN.md)**

`src/` 是商媒运营助手电商自媒体 Agent 平台的员工端。它面向运营、内容、投放、客服等执行角色，把 Agent 对话、任务队列、素材采集、内容生产、社媒账号和发布工具放进同一个 macOS/Windows 桌面客户端。

[返回平台总览](../README.md) · [老板端说明](../boss/README.md)

## 支持功能

| 模块 | 当前能力 |
| --- | --- |
| 运营中枢 | 基于 Goose 的 Agent 会话、工具调用、任务拆解、执行过程和结果回传 |
| 任务队列 | 查看进行中、待处理和已完成的运营任务，沉淀可复盘的工作结果 |
| 资料与素材 | 管理知识、网页/评论/文本素材、图片、视频、音频和本地文件 |
| 内容生产 | 内容稿件、图文与视频脚本、封面、图片/视频生成和视频时间线编辑 |
| 团队成果 | 汇总员工或 Agent 的任务产出、媒体产物和历史结果 |
| 社媒账号 | 账号池、二维码/浏览器登录、Cookie 持久化、状态校验和账号切换 |
| 素材采集 | Rust `mediacrawler` 统一采集接口，覆盖小红书、抖音、B 站等平台，并保留其他平台的分阶段适配器 |
| 内容发布 | 图文/视频发布计划、账号前置检查、定时参数、`dry_run` 与人工确认安全闸 |
| 插件与技能 | 内置插件、技能发现、插件设置和 OpenMontage 内容/视频工作流 |
| 本地数据 | Tauri/Rust 本地服务、SQLite/文件资产和应用数据目录，不要求把业务数据上传到统一云端 |

### 社媒能力范围

- 账号登录和账号池覆盖：小红书、抖音、快手、B 站、微信视频号、YouTube。
- 视频发布适配器覆盖上述六个平台。
- 图文发布当前重点覆盖小红书、抖音和快手；其他平台需按适配器实际状态确认。
- 采集层包含小红书、抖音、B 站、快手、微博、贴吧、知乎等平台代码，但不同平台完成度和登录要求不同，不能把“存在适配器”理解为全部已经生产验收。
- 真实平台会调整登录、风控和发布页面，升级或正式使用前必须重新做账号登录和小流量发布测试。

## 技术结构

```text
desktop/ React + Vite
        │ Tauri IPC
src-tauri/ Tauri 2 桌面壳
        │
crates/yunying-server   本地服务、数据库、IPC、Goose 桥接
crates/yunying-ops      运营工具与 MCP
crates/mediacrawler     多平台素材采集
crates/socialconnect    账号、登录、排期与发布
builtin-plugins/        OpenMontage 等可选插件
```

## 平台支持

| 平台 | 开发运行 | 安装产物 | 说明 |
| --- | --- | --- | --- |
| macOS Apple Silicon | 支持 | `src-tauri/target/release/bundle/dmg/商媒运营助手_0.1.0_aarch64.dmg` | 当前工作区已有 DMG；未签名开发包可能需要右键选择“打开” |
| Windows x64 | 支持 | `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/商媒运营助手_0.1.0_x64-setup.exe` | 当前为测试安装包，不是完整离线运行时包 |

上表路径是当前工作区已生成的产物。全新检出不会自带 `target/`，需要重新构建。

### Windows 安装包限制

当前 NSIS 脚本会打包主程序、`yunying-ops-mcp.exe`、配置和内置插件，但尚未完整打包 Windows 原生 `uv.exe`、`ffmpeg.exe` 与 `ffprobe.exe`。因此：

- Agent、基础 UI、Rust 后端和已打包工具可用于 Windows 测试。
- Python 插件、视频剪辑或依赖 FFmpeg 的功能可能不可用。
- 对外发布 Windows 正式版前，需要补齐对应架构的原生运行时并重新做整机验收。

## 环境要求

通用要求：

- Node.js 20 或更高版本。
- npm；员工端以 `desktop/package-lock.json` 为可复现安装入口。
- Rust 1.92；仓库的 `rust-toolchain.toml` 会自动选择该工具链。
- Tauri CLI 2.x。
- Chrome 或 Edge，用于部分社媒二维码登录和 CDP 自动化。
- 可访问所配置的 LLM、图片和视频服务。

macOS 还需要：

- Xcode Command Line Tools：`xcode-select --install`。
- 生成 Apple Silicon 安装包时使用 arm64 工具链和运行时。

Windows 还需要：

- Visual Studio Build Tools 2022，并安装“使用 C++ 的桌面开发”。
- Microsoft Edge WebView2 Runtime。
- NSIS；仅在使用仓库自定义 `.nsi` 脚本时需要。

首次安装 Tauri CLI：

```bash
cargo install tauri-cli --version "^2" --locked
```

## 配置

员工端从 `config.json` 读取模型、采集、社媒、图片、视频和安全开关。新环境中仅在文件不存在时复制示例：

```bash
cd src
test -f config.json || cp config.json.example config.json
```

Windows PowerShell：

```powershell
cd src
if (-not (Test-Path config.json)) { Copy-Item config.json.example config.json }
```

最少需要检查：

- `goose.provider`、`goose.model`、`goose.base_url` 和 `goose.api_key`。
- 使用生图时配置 `image`；使用视频生成时配置 `video`。
- `safety.run_real_crawler`、`run_real_publish`、`run_real_image`、`run_real_video` 默认应保持 `false`，完成测试后再按需开启。

不要提交真实 API Key、Cookie、账号文件或生产环境配置。

### 连接老板端

任务中心支持把真实“已完成”任务显式上报到老板端，并读取本周老板反馈。请求由员工端 Rust 后端发出，前端不会读取员工令牌；草稿、失败、取消和未执行任务不会自动上报。

macOS 开发环境可在启动员工端的同一终端配置：

```bash
export BOSS_SERVER_URL="http://127.0.0.1:8787"
export BOSS_OWNER_CODE="BOSS-7429"
export BOSS_EMPLOYEE_TOKEN="replace-with-employee-token-at-least-32-bytes"
export EMPLOYEE_ID="employee-001"
export EMPLOYEE_NAME="员工姓名"
export EMPLOYEE_ROLE="内容运营"
```

Windows PowerShell：

```powershell
$env:BOSS_SERVER_URL = "http://127.0.0.1:8787"
$env:BOSS_OWNER_CODE = "BOSS-7429"
$env:BOSS_EMPLOYEE_TOKEN = "replace-with-employee-token-at-least-32-bytes"
$env:EMPLOYEE_ID = "employee-001"
$env:EMPLOYEE_NAME = "员工姓名"
$env:EMPLOYEE_ROLE = "内容运营"
```

`BOSS_SERVER_URL` 默认是本机 `http://127.0.0.1:8787`，`EMPLOYEE_ROLE` 默认是“员工”。本机 loopback 连接的 token 可选；远程老板端必须使用 HTTPS，并配置至少 32 字节、只允许 `employee.*` 工具的 `BOSS_EMPLOYEE_TOKEN`。不要把老板管理令牌分发给员工。当前没有离线队列，连接失败时任务不会被标记为已上报，可在恢复连接后重试同一任务（事件 ID 幂等更新）。

## 从源码运行

### 1. 安装前端依赖

```bash
cd src/desktop
npm ci
```

### 2. 启动 Vite 前端

终端一：

```bash
cd src/desktop
npm run dev
```

前端默认监听 `http://127.0.0.1:5173`；Tauri 配置使用 `http://localhost:5173` 加载它。

### 3. 启动桌面客户端

终端二：

```bash
cd src/src-tauri
cargo tauri dev
```

开发模式会启动 Tauri 窗口、嵌入 Rust 服务，并按需查找本地 `yunying-ops-mcp`、浏览器、插件和媒体运行时。缺少可选运行时时，日志会显示警告，对应插件或视频能力不可用，但基础 UI 仍可启动。

## 构建与安装

### 基础检查

```bash
cd src/desktop
npm run build

cd ..
./scripts/check.sh check
```

当前前端生产构建和 Rust `check` 可以完成。严格类型检查命令为：

```bash
cd src/desktop
npm run typecheck
```

当前重构分支的严格类型检查尚未全绿，已知问题集中在 IPC 类型声明、官方认证接口和 vendored Freecut 视频编辑代码。该问题不由 README 修改引入；`npm run build` 仍可生成 `desktop-dist/`，但正式发版前应先清零类型错误。

### macOS 构建

```bash
cd src/src-tauri
cargo tauri build
```

成功后通常在 `src-tauri/target/release/bundle/` 下生成 `.app` 和 `.dmg`。打开 DMG 后，将“商媒运营助手”拖入“应用程序”即可安装。

### Windows 本机构建

在 Windows PowerShell 中执行：

```powershell
cd src\desktop
npm ci
npm run build

cd ..\src-tauri
cargo tauri build
```

如需使用当前自定义 NSIS 脚本，在准备好 x64 主程序和 helper 后执行：

```powershell
cd src\src-tauri\installer\windows
makensis cohmira-installer.nsi
```

安装器采用当前用户权限，默认安装到 `%LOCALAPPDATA%\Programs\商媒运营助手`，并创建开始菜单、桌面快捷方式和卸载入口。

### 当前打包配置注意事项

`src-tauri/tauri.conf.json` 现在通过 `scripts/prepare-tauri-build.mjs` 跨平台构建前端和 `yunying-ops-mcp`，不再依赖旧的 `Beav/desktop` 打包前置路径。基础 Tauri 安装包可以在没有 FFmpeg 的情况下生成；此时视频剪辑能力会在运行时显示缺少 `ffmpeg-runtime` 的警告。`src-tauri/src/main.rs` 仍保留旧路径作为开发环境兼容回退。

CI 会通过 `astral-sh/setup-uv` 将当前平台的 `uv` 可执行文件加入安装包。需要完整视频能力时，还应在打包前向 Tauri 资源中加入同平台的 `ffmpeg`、`ffprobe` 及所需动态库，并在目标系统完成验收。

## 首次使用建议

1. 先配置可用的模型 Provider，并完成一次普通 Agent 对话。
2. 在“社媒账号”中创建测试 profile，完成扫码、手机确认、Cookie 保存和重新启动后的持久化检查。
3. 先用 `dry_run` 查看采集或发布计划，不要直接启用真实写操作。
4. 用测试账号、小范围素材完成一次图文或视频发布。
5. 检查任务队列、团队成果、资料库、稿件和媒体产物是否完整回流。
6. 确认目标平台、模型、FFmpeg 和插件运行时都通过后，再制作交付安装包。

## 常用验证命令

```bash
# 前端生产构建
cd src/desktop
npm run build

# Rust 自研模块检查
cd src
./scripts/check.sh check

# 完整自研模块测试；耗时更长
./scripts/check.sh test

# 当前用于跟踪重构类型债务，现阶段可能失败
cd desktop
npm run typecheck

# 只构建运营 MCP helper
cd ..
cargo build -p yunying-ops --bin yunying-ops-mcp --features mcp
```

## 数据与安全

- 应用数据、数据库、账号 Cookie 和插件设置写入操作系统应用数据目录，不应放进源码目录或提交到 Git。
- 社媒真实发布默认要求 `confirm=true`，并强制先通过账号检查。
- 对外部平台的自动化操作可能触发验证码或安全验证，应保留人工接管能力。
- 采集、生成和发布产生的素材要保留来源、执行记录和人工复核状态，便于老板端后续审计。
