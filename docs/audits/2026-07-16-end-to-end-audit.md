# 智能体员工端与老板端端到端审核

审核日期：2026-07-16（Asia/Shanghai）

## 结论

员工端与老板端的核心业务链路已经完成本机端到端审核。真实图片、真实视频、公开内容采集、一次性定时任务、长周期任务、报告落盘、员工周报、老板审阅、记账、发票、老板 AI、员工绑定、设置和跨端同步均有可复核证据。

老板端后端和桌面壳为 Rust/Tauri。macOS 应用运行包由 `boss-desktop`、`boss-server` 两个 Rust Mach-O 及图标、plist、签名元数据构成；Windows 产物为 x86-64 Rust PE，不携带 Python、Node 或 Electron runtime。

这不是“绝对零缺陷”的数学保证。微博游客验证、快手空结果、真实公开发布、未配置的 Actual 直连、正式签名/公证，以及本轮不可用的应用内 Browser 均在“外部边界”中如实列出，没有标成通过。

## 验收结果

| 范围 | 结果 | 证据 |
| --- | --- | --- |
| 员工端功能点击 | 既有完整 UI 审核 59/59 断言、24 次坐标点击、10 个主导航、19 张截图；console/runtime/network/a11y/contrast/overflow 均为 0 问题 | `src/e2e-artifacts/2026-07-16-employee-acceptance/logs/ui-browser-results.json` |
| 8 类运营业务 | 8/8 工作包完成；所有当时被安全门阻断的外部步骤均没有伪造成功 | `src/e2e-artifacts/2026-07-16-employee-acceptance/logs/live-8-scenarios.log` |
| 真实图片 | 1 次供应商请求成功；PNG 768×1024、1,110,161 bytes、61.608 秒；原图人工检查通过 | `src/e2e-artifacts/2026-07-16-real-business/generation-report.json` |
| 真实视频 | 1 次供应商请求成功；H.264 1280×720、5.041667 秒、121 帧、4,458,684 bytes；121 帧均不同，无黑场/冻结 | `src/e2e-artifacts/2026-07-16-real-business/generation-report.json` |
| 真实采集 | Bilibili Rust MCP 返回 1 条可核验公开结果；微博明确报告游客验证；快手 0 条按失败记录，不把空结果算通过 | `src/e2e-artifacts/2026-07-16-real-business/crawler-final-review.md` |
| 定时与长周期 | Rust/Tokio 真墙钟 +1 分钟偏差 +4ms，+5 分钟偏差 +3ms；两次均经 Goose 调用 `create_note`，落盘 Markdown、0600 回执和唯一 SQLite start/completed 轨迹 | `src/e2e-artifacts/2026-07-16-real-business/scheduler/REPORT.md` |
| 老板端 UI | 既有 16/16 场景连续两次通过，12 张截图；桌面/移动导航、周报、记账、票据、AI、绑定与设置均覆盖 | `boss/desktop/e2e-artifacts/final-audit/report.json` |
| 员工与老板同步 | 6/6 步骤连续两次通过：员工上报与幂等更新、老板读取与审阅、员工读回 | `boss/desktop/e2e-artifacts/cross-app-sync/report.json` |
| 深浅主题 | 两端首帧主题初始化、全业务语义表面、状态色/按钮/侧栏 WCAG AA、窄窗口布局与回归扫描通过 | `src/e2e-artifacts/2026-07-16-real-business/theme/REPORT.md`、`boss/desktop/e2e-artifacts/theme/REPORT.md` |
| Rust | 员工自研工作区 287 passed、0 failed、28 ignored；Ops 全 feature/target 17 passed、0 failed、1 ignored；老板后端 10 passed，严格 Clippy 通过 | 本轮最终终端回归 |
| 前端 | 员工 132 个文件、654 项测试；两端类型检查与生产构建通过；两端生产依赖审计均为 0 vulnerabilities | 本轮最终终端回归 |
| 桌面产物 | 员工 macOS `.app` 用当前源码重建并隔离启动通过，1,538 文件私密值命中 0；员工 Windows GUI/helper PE 已刷新。老板 macOS `.app` 与 Windows GUI/server PE 均为当前 Rust/Tauri 源码 | `docs/audits/2026-07-16-desktop-packaging-final.md` |

## 关键修复

1. 新增真正执行的 Rust 定时/长周期调度器：SQLite 持久化、实例租约、执行 CAS、取消排空、崩溃后人工复核、日/周时区计算和真实回执。
2. 调度任务默认拒绝工具访问，只允许任务白名单；图片、视频、发布、记账等高风险工具必须显式授权，非幂等任务不会在崩溃后自动重放。
3. 修复停机认领竞态和全局并发上限；自动扫描与手动立即运行共享容量，不会每 500ms 持续堆积任务。
4. 本地生成 bridge 改为随机 loopback 端口、每次启动随机 bearer、全路由鉴权；bridge 和必需 MCP 未就绪时调度器不会启动。
5. 正式应用数据目录的 `config.json` 通过 `YUNYING_CONFIG_PATH` 显式传给 Rust/MCP，任意 cwd 下不会回退到错误配置。
6. 真实图片测试发现并修复 catalog 尺寸元数据：`size` 保存实际像素，`requestedSize` 保留请求规格，旧数据自动迁移。
7. Bilibili WBI 空结果增加结构化 SSR 回退；微博修复 `containerid` URL 编码并把游客 HTML 明确分类，不再误报 JSON decode。
8. `create_note` 使用 UUID、完整 Markdown、原子无覆盖发布、碰撞重试和路径防护；调度成功必须验证工具返回和实体文件。
9. 两端深色主题统一到语义 token；修复员工 Workboard 窄窗口裁切、稿件弹层白面、RedClaw 抽屉单列，以及老板端原本没有真实主题状态的问题。
10. 老板端继续覆盖重复发票幂等、陈旧响应不得回退已入账状态、流水复核、Actual 状态检查和员工周报审阅持久化。

## 安全与副作用

- 真实付费调用严格限制为图片 1 次、视频 1 次；没有为了报告重复调用供应商。
- 私有 `src/config.json` 的安全开关在磁盘上保持关闭；真实测试只用对应子进程临时环境开关，退出即消失。
- 没有执行真实公开发布。发布能力仍要求平台账号、素材、显式安全开关和人工确认。
- 最终调度证据目录按当前配置中的非空 secret/key/token/cookie/password 精确扫描为 0 命中；回执权限为 0600。

## 外部边界

- 微博在无登录态返回游客验证页；快手游客接口本轮返回 0 条。两者均保留失败结果，不能据此宣称对应平台真实采集通过。
- Actual Budget 未配置 server/sync ID/credential/account 时，只验收状态检查、队列和导出；不宣称已真实直连入账。
- 调度定义与执行历史跨重启持久化，但应用进程完全退出期间不会执行；错过且未认领的任务会在下次启动恢复扫描。
- 应用内 Browser 本轮返回空实例列表，因此新主题和自动化抽屉没有新增点击截图。此前功能截图仍有效，但不作为新主题的视觉证据；新 E2E 脚本已加入深色计算样式与截图步骤。
- macOS 包目前为 ad-hoc 签名，尚未 Developer ID 签名或公证；正式 Windows 安装器仍需 Windows runner 与代码签名。
