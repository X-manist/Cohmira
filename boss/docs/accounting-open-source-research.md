# 老板端记账底座调研

首次检查：2026-07-01  
架构更新：2026-07-15

## 结论

老板端第一版不应该直接嵌入完整财务系统。更稳的做法是先在老板端建立自己的工具契约和轻量台账：AI 所有记账回答必须调用工具，交易、发票、员工成本先写入可审计数据库。后续如果要接成熟系统，再按业务复杂度挂接开源记账/会计软件的 API。

推荐路径：

1. 选定底座：Actual Budget。原因是 MIT、local-first、官方 API，对老板端统一 UI 和二次开发较友好。
2. 当前源码位置：`/Volumes/macsoftware/codes/agentscompany/yunyingagent/boss/third_party/actual`，也可通过 `BOSS_ACTUAL_ROOT` 覆盖。
3. 近期：老板端不直接嵌 Actual 页面。`boss/desktop/src-tauri` 的 Rust 核心暴露统一工具，用老板端 UI 展示流水、报表、发票草稿和 Actual 同步作业。
4. 当前 Rust adapter 负责集成状态、审批门禁、CSV/JSONL 导出和可审计同步作业。只有已审批交易可以同步；未配置 Actual server、Sync ID、凭据或账户时保持 dry-run/queued，不在 Rust 进程中内嵌 Actual Server。
5. 正式发票、应收应付、总账复杂度上来后，再评估 Bigcapital 或 Frappe Books 作为第二阶段替代。

Actual 源码已由用户复制到 `boss/third_party/actual`；这是当前默认主源码位置。

旧 Python `server.py` 已从仓库移除。老板端本地后端已全部迁移到 Rust，SQLite 通过 `rusqlite` bundled 构建，不要求 Python。

## 候选方案

| 项目 | 当前状态 | 更适合什么 | 接入方式 | 老板端判断 |
| --- | --- | --- | --- | --- |
| Actual Budget | GitHub API 检查为活跃，约 27.3k stars，MIT | 本地优先预算、现金流、个人/小团队支出管理 | 官方 API 和 CLI 可操作本地数据 | 已选为第一阶段底座；老板端保留统一 UI，通过 adapter 调用 |
| Firefly III | GitHub API 检查为活跃，约 23.9k stars，AGPL-3.0 | 自托管个人/小企业资金流、预算、分类、规则 | REST JSON API，Data Importer 支持 CSV/CAMT 等导入 | 适合自托管资金流与报表，API 友好 |
| Frappe Books | 官网显示桌面优先、SQLite、本地运行，约 4.7k stars，AGPL-3.0 | 桌面会计、发票、P&L、总账、资产负债表 | 本地 SQLite 文件，后续需要封装 adapter | 适合正式会计语义，但自动化接口需要二次封装 |
| Bigcapital | GitHub README 显示开源会计/库存、Docker、自托管和 API | 中小企业在线会计、库存、发票、智能报表 | API Reference + Docker 部署 | 更像老板端后续正式财务后端候选 |
| ERPNext | Frappe 官方文档称 GPLv3 开源，Accounting 是核心模块 | 公司级 ERP：会计、库存、采购、销售、HR | Frappe/ERPNext API | 功能最全，但首版过重 |
| GnuCash | 官方定位为免费会计软件，支持复式记账和报表 | 桌面本地传统会计 | 文件/导入导出，自动化成本较高 | 适合人工会计，不适合作为 AI 工具优先底座 |
| Akaunting | 官方称开源在线会计，GitHub API 当前 license 为 NOASSERTION | 小企业在线发票和支出 | RESTful API，Laravel/Vue | 可观察，但许可/商业边界要单独复核 |
| Maybe Finance | GitHub 页面显示 2025-07-27 archived | 个人财务 UI 参考 | Fork 自行维护 | 不建议作为主底座 |

## 对老板端的产品含义

- 员工管理和经营问答不等于财务系统，必须有独立的员工工作数据源。
- 账本工具必须返回结构化数据：交易 ID、来源、状态、金额、类目、凭证路径。
- 发票识别不能让模型直接“看图猜账”。正确链路是：视觉/OCR 工具识别字段，账本工具生成待复核草稿，老板确认后入账。
- Actual Budget 接入通过 Rust 工具分发层暴露一致契约：Tauri 桌面调用 `call_boss_tool`，浏览器开发调用 `POST /api/boss/tool`；前端和 AI 不直接读写第三方数据库。
- 上传发票工具支持文本和外部 `ai_ocr_json`。图片/PDF 等二进制只安全落盘并进入 `needs_ai_ocr_adapter` / `needs_review`，不依赖 Python、`pdftotext` 或 Tesseract。
- macOS 与 Windows 复用同一个 Tauri 2 + Rust 代码库；正式发布前仍需分别完成签名、公证和对应平台构建验证。

## 来源

- Actual Budget 官网说明本地优先、可自托管同步、API：<https://actualbudget.org/>
- Actual Budget API 文档：<https://actualbudget.org/docs/api/>
- Firefly III 官网/文档/API：<https://www.firefly-iii.org/>、<https://docs.firefly-iii.org/>、<https://api-docs.firefly-iii.org/>
- Frappe Books 官网说明桌面优先、本地 SQLite、会计报表：<https://frappe.io/books>
- Bigcapital GitHub README 说明开源会计/库存、Docker、自托管、API：<https://github.com/bigcapitalhq/bigcapital>
- ERPNext 会计页与开源说明：<https://frappe.io/erpnext/open-source-accounting>、<https://docs.frappe.io/erpnext/open-source>
- GnuCash 官网与功能页：<https://gnucash.org/>、<https://www.gnucash.org/features.phtml>
- Akaunting 官网与 GitHub：<https://akaunting.com/>、<https://github.com/akaunting/akaunting>
- Maybe Finance GitHub archive 状态：<https://github.com/maybe-finance/maybe>
