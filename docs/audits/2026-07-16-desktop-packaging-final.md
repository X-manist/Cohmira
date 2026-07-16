# 桌面端最终打包核验

核验时间：2026-07-16 12:20–12:45（Asia/Shanghai）

## 结论

- 员工端 macOS arm64 `.app` 已从包含最终调度器与统一主题的当前源码重新构建，包内运营 helper、公开配置模板和 Tauri 主程序均已核验；隔离启动冒烟与 ad-hoc 签名校验通过。
- 员工端 Windows x64 GUI 与运营 helper 已由当前源码在 macOS 上通过 `cargo-xwin` 重新交叉编译为 PE。Windows 完整离线 runtime 仍需在 Windows 主机准备和执行，因此本轮没有生成或宣称新的 NSIS 安装器。
- 老板端 macOS arm64 `.app` 包含 Rust GUI 与 Rust server 两个 Mach-O。发现顶层 bundle 未封装签名后，已补做本地 ad-hoc 深度签名并通过校验；隔离启动和 SQLite 初始化通过。
- 老板端现有 Windows x64 GUI/Server PE 已核验为当前源码之后生成的 x86-64 Windows 可执行文件。未在 macOS 上声称 Windows 运行时启动结果或正式安装器结果。
- 所有敏感值核验只记录计数，不记录值。员工私有 `config.json` 在 macOS 与 Windows 构建前后指纹和元数据保持不变。

## 员工端 macOS

最终应用：`src/src-tauri/target/release/bundle/macos/商媒运营助手.app`

| 项目 | 结果 |
| --- | --- |
| 当前代码构建 | 通过；Tauri CLI 2.11.4、Rust 1.92.0、Node 22.22.0；`beforeBuildCommand` 完成前端生产构建、CSS 校验和 `yunying-ops-mcp --features mcp --release` |
| 主程序 | `Contents/MacOS/yunying-desktop`，arm64 Mach-O，110,360,448 bytes，2026-07-16 12:32:12 |
| 运营 helper | `Contents/Resources/bin/yunying-ops-mcp`，arm64 Mach-O，17,808,272 bytes，2026-07-16 12:29:06；与 `src/target/release/yunying-ops-mcp` 逐字一致且有执行权限 |
| 辅助 runtime | `Contents/Resources/bin/uv`，arm64 Mach-O且有执行权限；OpenMontage 配置、Remotion composer 和 requirements 均存在；`ffmpeg-runtime` 与 `python-runtime` 在 macOS 包中只有平台说明占位文件，不宣称完整离线视频剪辑 runtime |
| 包结构 | 1,538 个文件，196,956,160 bytes（磁盘占用）；Info.plist identifier 为 `com.agentscompany.yunyingagent.desktop`，版本 `0.1.0` |
| 配置安全 | 包内 `Contents/Resources/config.json` 与 `src/config.json.example` 逐字一致；配置资源测试 3/3 通过 |
| 私密值扫描 | 从本机私有配置识别 2 个已配置敏感值，对 `.app` 1,538 个文件进行精确字节扫描，命中 0；扫描未输出敏感值本身 |
| 签名 | 本地 ad-hoc，`codesign --verify --deep --strict` 通过；未做 Apple Developer ID 签名或公证 |
| 隔离启动 | 使用临时 HOME/TMPDIR，并显式关闭真实图片、视频、采集和发布开关；进程 8 秒后仍存活，panic/fatal 模式 0，随后由核验脚本发送 TERM（退出码 143 为预期） |

现有 `src/src-tauri/target/release/bundle/dmg/商媒运营助手_0.1.0_aarch64.dmg` 的时间为 2026-07-15 15:30:27，早于本轮最终代码与 `.app`，因此不是本轮最终交付物。

## 员工端 Windows

| 产物/检查 | 结果 |
| --- | --- |
| GUI PE | `src/src-tauri/target/x86_64-pc-windows-msvc/release/yunying-desktop.exe`；PE32+ GUI x86-64，118,299,648 bytes，2026-07-16 12:37:27；当前源码交叉编译成功 |
| 运营 helper PE | `src/target/x86_64-pc-windows-msvc/release/yunying-ops-mcp.exe`；PE32+ console x86-64，16,217,600 bytes，2026-07-16 12:38:40；当前源码交叉编译成功 |
| 松散配置 | `src/src-tauri/target/x86_64-pc-windows-msvc/release/config.json` 与公开模板逐字一致 |
| 构建隔离 | 私有配置与 macOS helper 的构建前后指纹一致，交叉编译未污染 macOS runtime |
| 完整离线 runtime | 未通过：macOS 工作区内只有占位文件，缺少 Windows `ffmpeg/ffprobe/ffplay.exe`、嵌入式 Python 和锁定依赖；准备脚本按设计要求在 Windows 上完成兼容性冒烟 |
| 旧 NSIS | `src/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/九伴智能_0.1.0_x64-setup.exe` 为 2026-07-13 21:42:33，早于最终调度器和主题源码；不作为本轮产物，不宣称可交付 |

交叉编译仅证明当前 Rust/Tauri Windows 目标可编译并生成正确 PE 类型；本机是 macOS，无法替代 Windows 原生启动、WebView2、安装/卸载、运行库与代码签名验收。

## 老板端 macOS

最终应用：`boss/desktop/src-tauri/target/release/bundle/macos/运营老板助手.app`

| 项目 | 结果 |
| --- | --- |
| 主程序 | `Contents/MacOS/boss-desktop`，arm64 Mach-O；最终 ad-hoc 签名后 13,845,504 bytes |
| Rust server | `Contents/MacOS/boss-server`，arm64 Mach-O；最终 ad-hoc 签名后 6,893,520 bytes |
| 包内容 | Info.plist、图标、两个 Rust Mach-O 与签名资源，共 5 个文件；无 `.py`、Python/Node 可执行文件、JS/TS 源码、数据库、`.env` 或 `config.json` |
| 运行时标记 | 两个 macOS 二进制及两个 Windows PE 中 `python.exe`、`node.exe`、Python shebang、Electron Framework 标记均为 0 |
| 签名修复 | 初检发现只有链接器 ad-hoc 签名，顶层 bundle 未封装，深度校验失败；执行本地 ad-hoc 深度签名后，identifier 为 `com.agentscompany.yunyingagent.boss`，`codesign --verify --deep --strict` 通过 |
| 隔离启动 | 临时 HOME/TMPDIR、临时 SQLite 路径、随机 loopback 端口；进程 8 秒后仍存活，panic/fatal 模式 0，SQLite 文件成功创建，随后由核验脚本发送 TERM |
| 私密值扫描 | 从老板端本地环境识别 1 个已配置敏感值，对最终 macOS 包与两个 Windows PE 共 7 个文件精确扫描，命中 0；扫描未输出值本身 |

ad-hoc 签名只用于保证本地 bundle 完整性，不替代 Apple Developer ID、hardened runtime 权限审核、公证和发布签名。

## 老板端 Windows

| 产物 | 结果 |
| --- | --- |
| `boss/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/boss-desktop.exe` | PE32+ GUI x86-64，13,780,992 bytes，2026-07-16 12:20:20；没有更新于该 PE 的老板前端源码 |
| `boss/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/boss-server.exe` | PE32+ console x86-64，6,183,424 bytes，2026-07-16 02:02:22；没有更新于该 PE 的 server/lib Rust 源码 |

这两个文件是可核验的当前 Windows PE，不是 Windows 安装器。正式 Windows 发布仍需 Windows runner 上的原生启动与 NSIS、WebView2、安装/卸载及 Authenticode 签名验收。

## 最终交付边界

本轮可直接核验的最终桌面交付物是两个 macOS `.app`，以及员工端当前源码的 Windows GUI/Helper PE、老板端当前 Windows GUI/Server PE。以下内容没有被夸大为已通过：

- 员工端旧 DMG 与旧 NSIS；
- 员工端 macOS 包中的完整 FFmpeg/Python 离线剪辑 runtime（直接供应商图片/视频能力与该离线剪辑 runtime 是不同路径）；
- Windows 完整离线 runtime 和原生启动/安装；
- Apple 公证、Developer ID 与 Windows Authenticode；
- 应用内 Browser 截图（按本轮要求未使用 Browser、Chrome 或 Playwright）。
