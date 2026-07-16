# Actual Budget 环境变量配置流程

更新时间：2026-07-15

老板端不直接修改 Actual 数据库。当前链路是：

1. Tauri 2 / Rust 核心把发票、流水和复核状态写入自己的 SQLite。
2. 只有已审批、币种为 CNY 的交易可以进入 Actual 同步。
3. 配置完整时，Rust 进程调用外部 Actual CLI；配置不完整时写入幂等 queued 作业。
4. 无论是否配置 Actual，都可以由 Rust 生成 CSV/JSONL 导入文件。

旧 Python `server.py` 已从仓库移除，以下流程不需要 Python。

## 配置文件位置

Rust 核心使用 `dotenvy` 非覆盖式加载以下位置的 `.env`：

1. 启动命令的当前目录下 `.env`
2. `boss/.env`
3. `boss/desktop/.env`

已经设置的进程环境变量优先，不会被 `.env` 覆盖。桌面开发推荐创建：

```text
boss/desktop/.env
```

真实 `.env` 包含服务密码或 token，不得提交到版本库。

## 环境变量

| 字段 | 必填 | 用途 |
| --- | --- | --- |
| `BOSS_ACTUAL_ROOT` | 否 | Actual 源码位置，默认识别 `boss/third_party/actual` |
| `ACTUAL_CLI_BIN` | 真实同步必填 | `actual` 可执行文件或源码 CLI 入口路径 |
| `ACTUAL_SERVER_URL` | 真实同步必填 | Actual Server 地址，例如 `http://localhost:5006` |
| `ACTUAL_PASSWORD` | 二选一 | Actual Server 密码 |
| `ACTUAL_TOKEN` | 二选一 | 可替代密码的 Actual token |
| `ACTUAL_SYNC_ID` | 真实同步必填 | 当前预算的 Sync ID |
| `ACTUAL_DEFAULT_ACCOUNT_ID` | 建议填 | 流水默认写入的 Actual 账户 ID |
| `ACTUAL_DATA_DIR` | 否 | Actual CLI 本地缓存目录 |
| `BOSS_ACCOUNTING_DB` | 否 | 覆盖老板端 SQLite 路径，适合开发和测试 |

最小示例：

```bash
BOSS_ACTUAL_ROOT=../third_party/actual
ACTUAL_CLI_BIN=actual
ACTUAL_SERVER_URL=http://localhost:5006
ACTUAL_PASSWORD=你的Actual服务器密码
ACTUAL_SYNC_ID=从Actual设置页复制的SyncID
ACTUAL_DEFAULT_ACCOUNT_ID=目标账户ID
ACTUAL_DATA_DIR=./data/actual-cli-cache
```

旧变量名 `ACTUAL_SESSION_TOKEN` 不再读取；Rust 核心使用 `ACTUAL_TOKEN`。

## 第一步：准备 Actual Server

### 方案 A：本机运行

适合开发测试：

```bash
npm install --location=global @actual-app/sync-server
mkdir -p ~/actual-server-data
cd ~/actual-server-data
actual-server
```

打开 `http://localhost:5006`，首次进入时设置的 server 密码填写到 `ACTUAL_PASSWORD`。

### 方案 B：托管或自托管

PikaPods 或 Docker 部署也可以使用。`ACTUAL_SERVER_URL` 应填写最终可访问的 Actual URL；生产环境建议使用 HTTPS。Rust 老板端不负责启动或内嵌 Actual Server。

## 第二步：创建预算并获取 Sync ID

1. 登录 Actual Server，创建或打开预算。
2. 建立接收老板端流水的账户，例如“运营现金账户”。
3. 在当前预算的 `Settings` → `Show advanced settings` 找到 `Sync ID`。
4. 把它写入 `ACTUAL_SYNC_ID`。

`Budget ID` 与 `Sync ID` 不同；同步必须使用 Sync ID。

## 第三步：准备 Actual CLI

全局安装：

```bash
npm install --location=global @actual-app/cli
actual --help
```

然后配置：

```bash
ACTUAL_CLI_BIN=actual
```

也可以使用 `boss/third_party/actual` 的源码 CLI：

```bash
cd boss/third_party/actual
corepack enable
yarn install
yarn build:cli
```

再把 `ACTUAL_CLI_BIN` 指向生成的 CLI 入口。Actual CLI 是可选的第三方进程；老板端自己的后端、SQLite 和同步编排均为 Rust，不依赖 Python。

## 第四步：获取默认账户 ID

确保 Actual Server、Sync ID 和凭据已经配置，再运行：

```bash
actual accounts list --format table
```

找到接收老板端流水的账户 ID，写入：

```bash
ACTUAL_DEFAULT_ACCOUNT_ID=账户ID
```

调用同步工具时也可以显式传入账户 ID；未显式传入时才使用这个默认值。

## 第五步：启动 Rust 开发 server

```bash
cd boss/desktop/src-tauri
cargo run --bin boss-server
```

默认监听 `127.0.0.1:8787`，可用 `BOSS_SERVER_ADDR` 覆盖。另一个终端可以检查集成状态：

```bash
curl -s http://127.0.0.1:8787/api/boss/tool \
  -H 'content-type: application/json' \
  -d '{"name":"actual.integration_status","arguments":{"owner_code":"BOSS-7429"}}'
```

期望 readiness 字段显示源码/CLI、server URL、Sync ID、凭据和默认账户的真实配置状态。桌面模式下也可直接运行 `npm run tauri:dev`，在 Actual 设置页查看同一结果。

## 第六步：验证导出与同步门禁

先运行 Rust 测试：

```bash
cd boss/desktop/src-tauri
cargo test
```

导出 Actual 文件：

```bash
curl -s http://127.0.0.1:8787/api/boss/tool \
  -H 'content-type: application/json' \
  -d '{"name":"actual.export_import_file","arguments":{"owner_code":"BOSS-7429","month":"2026-07","format":"csv"}}'
```

同步前必须确认：

- 交易状态已经审批，不是 `needs_review`。
- 币种为 CNY。
- CLI、Server URL、Sync ID、密码/token 和账户 ID 完整。
- 同一交易重复提交时命中幂等作业，不产生重复流水。

配置不完整时，Rust 核心应返回 dry-run/queued 或清晰的 readiness 状态，而不是伪造同步成功。

## 常见错误

| 现象 | 原因 | 处理 |
| --- | --- | --- |
| `source_present=false` | `BOSS_ACTUAL_ROOT` 指错 | 确认目录存在并包含 Actual 源码 |
| CLI 不可用 | `ACTUAL_CLI_BIN` 缺失或路径错误 | 运行对应 CLI 的 `--help` 验证 |
| `server_url_configured=false` | 未设置 `ACTUAL_SERVER_URL` | 本地通常填 `http://localhost:5006` |
| `credential_configured=false` | 密码/token 缺失 | 设置 `ACTUAL_PASSWORD` 或 `ACTUAL_TOKEN` |
| 找不到预算或账户 | Sync ID 错或 CLI 未连接 | 重新核对预算 Sync ID，并列出 accounts |
| 同步进入 queued | readiness 不完整 | 按状态字段补齐 CLI、server、凭据、Sync ID 和账户 |
| 交易被拒绝 | 未审批或不是 CNY | 先人工复核并确认币种 |
| `.env` 没生效 | 启动目录不在加载范围或已有环境变量覆盖 | 使用 `boss/desktop/.env`，检查进程环境优先级 |

## 参考来源

- Actual API 文档：<https://actualbudget.org/docs/api/>
- Actual CLI 文档：<https://actualbudget.org/docs/api/cli/>
- Actual Settings / Sync ID：<https://actualbudget.org/docs/settings/>
- Actual Server CLI：<https://actualbudget.org/docs/install/cli-tool/>
- Actual Docker：<https://actualbudget.org/docs/install/docker/>
- PikaPods 部署 Actual：<https://actualbudget.org/docs/install/pikapods/>
