# 老板端首版产品边界

更新时间：2026-07-15

## 当前实现

- `desktop/src/`：老板端 TypeScript/Vite 界面，包含今日总览、按周员工工作检查、老板审阅、AI 管理摘要、记账与复核、发票和绑定状态。
- `desktop/src-tauri/`：老板端唯一后端。Rust 核心负责 SQLite、员工工作事件、周审阅、账本、发票草稿、Actual 状态/导出/同步作业和工具分发。
- `desktop/src-tauri/src/main.rs`：Tauri 2 桌面入口，通过 `call_boss_tool` 调用 Rust 核心，并自动启动同一核心的 HTTP 服务供员工端同步。
- `desktop/src-tauri/src/bin/boss-server.rs`：浏览器开发用 Rust HTTP server，提供与桌面端一致的 `POST /api/boss/tool` 契约。
- `third_party/actual/`：Actual Budget 源码目录，作为可选财务集成；Rust 核心不直接修改其数据库，也不内嵌 Actual Server。
- 旧 Python `server.py` 已移除；运行、测试与桌面构建只使用 `desktop/src-tauri` Rust 核心。

## 技术边界

```text
TypeScript/Vite UI
    ├── Tauri 桌面：invoke("call_boss_tool")
    ├── 浏览器开发：POST /api/boss/tool
    └── 员工 Rust 客户端：employee.*（Bearer 分权）
                         ↓
                 同一个 Rust BossCore
                         ↓
          rusqlite bundled + 本地文件/导出
```

- 本地后端只使用 Rust，SQLite 通过 `rusqlite` bundled 构建。
- Tauri 2 从同一代码库构建 macOS 和 Windows 桌面应用。
- 桌面版数据库默认放在系统 app-data；独立 Rust server 使用平台数据目录；`BOSS_ACCOUNTING_DB` 可覆盖。
- Tauri 与独立 server 默认监听 `127.0.0.1:8787`；Vite 通过 `BOSS_SERVER_URL` 覆盖代理目标，server 通过 `BOSS_SERVER_ADDR` 覆盖监听地址。
- 非 loopback 监听必须显式启用远程模式，并配置两个不同且至少 32 字节的令牌；员工令牌只允许 `employee.*`。远程员工客户端强制使用 HTTPS。
- Python 后端已经移除，不得重新接入 Vite、Tauri 或发布包。

## 老板端和员工端的关系

Rust 核心已提供本机数据闭环：

| 工具 | 用途 |
| --- | --- |
| `employee.report_work_event` | 员工写入任务、素材、质量、成本和异常事件 |
| `boss.employee_work_report` | 老板按员工与周区间查看工作记录 |
| `boss.save_week_review` | 老板保存周审阅和补充要求 |
| `employee.list_week_reviews` | 员工读取老板审阅 |

员工端任务中心已通过 Rust HTTP 客户端调用上述工具，只允许显式上报已有成功执行记录的任务，并可读取本周老板反馈。固定老板码 `BOSS-7429`、共享员工令牌和环境变量配置仍属于开发阶段；正式账号、按员工签发凭据、组织授权、租户隔离、离线重试、冲突处理和消息推送仍是后续工作。

建议生产数据表保留以下边界：

| 表 | 关键字段 |
| --- | --- |
| owners | id, email, display_name, owner_code, created_at |
| employee_bindings | id, owner_id, employee_id, owner_code, status, bound_at |
| employee_work_events | id, employee_id, owner_id, task_type, material_ref, quality_score, cost_cents, created_at |
| weekly_reviews | id, owner_id, employee_id, week_start, review_status, comment, created_at |
| ledger_transactions | id, owner_id, type, category, amount_cents, source, evidence_path, status, created_at |
| invoice_drafts | id, owner_id, file_path, extracted_fields_json, confidence, status, created_at |

## AI 工具约束

老板端 AI 的规则应同时固化在 system prompt 和 Rust 工具层：

- 回答员工进度、质量、素材、成本时，必须调用 `boss.owner_context` 或员工数据工具。
- 回答收入、支出、利润、预算、账单时，必须调用账本工具。
- 处理发票、截图、凭证时，必须先调用发票识别工具，再生成待复核账本草稿。
- 只有已审批交易能进入 Actual 同步作业。
- 工具没有返回的字段不得补全；必须标记为 `unknown` 或 `needs_review`。

## 发票与 Actual 边界

- Rust 直接解析文本票据或外部适配器提供的 `ai_ocr_json`。
- 图片/PDF 二进制只安全落盘并标记为需要 OCR 适配器，不依赖 Python/Tesseract，也不能看图猜账。
- Actual 第一阶段只提供状态检查、CSV/JSONL 导出和可审计同步作业；未配置时保持 dry-run/queued。
- 如未来启用真实写入，仍需保留人工审批、幂等、失败重试和操作审计。

## 桌面交付

开发命令：

```bash
cd boss/desktop
npm run tauri:dev
```

在 macOS 或 Windows 目标系统上构建：

```bash
npm run tauri:build
```

Rust 验证：

```bash
cd boss/desktop/src-tauri
cargo test
cargo check --all-targets
```

安装包可以在两个平台分别生成，仓库已提供 macOS `.app` / Windows NSIS 无签名打包 CI；正式公开分发前仍需完成 Apple 签名/公证、Windows 代码签名和发布凭据配置。

## 后续接入顺序

1. 接入正式账号体系、组织授权与租户隔离，替换固定老板码和演示登录。
2. 在已实现的显式幂等上报上增加离线队列、自动重试和冲突处理。
3. 在已实现的员工主动读取反馈上增加老板审阅、补充要求和经营决策的可靠推送。
4. 接入 OCR/视觉模型 adapter，同时保存来源、置信度和人工复核状态。
5. 完成 Actual 真实连接器、失败重试和同步审计。
6. 在现有 macOS/Windows 编译 CI 上接入签名、公证与更新发布。
7. 如业务需要完整会计系统，再评估 Bigcapital 或 Frappe Books。
