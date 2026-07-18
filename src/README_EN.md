# Cohmira Workspace

<p align="center">
  <img src="../branding/cohmira/cohmira-workspace.svg" alt="Cohmira Workspace" width="720" />
</p>

**Language: [简体中文](./README.md) | English**

`src/` is the employee client of the Cohmira e-commerce and social media Agent platform. It is designed for operations, content, advertising, customer service, and other execution roles, combining Agent conversations, task queues, material collection, content production, social accounts, and publishing tools in one macOS/Windows desktop application.

[Platform overview](../README_EN.md) · [Boss client guide](../boss/readme_en.md)

## Supported Features

| Module | Current capabilities |
| --- | --- |
| Operations hub | Goose-based Agent sessions, tool calls, task decomposition, execution progress, and result delivery |
| Task queue | Review active, pending, and completed operations tasks and retain results for later analysis |
| Knowledge and materials | Manage knowledge, web/comment/text materials, images, videos, audio, and local files |
| Local documents | Read-only Rust parsing for PDF, DOCX/DOCM, PPTX/PPTM, and XLSX/XLSM without Python, Office, LibreOffice, or Poppler |
| Content production | Manuscripts, image-post and video scripts, covers, image/video generation, and timeline-based video editing |
| Team results | Aggregate employee or Agent task output, media artifacts, and historical results |
| Social accounts | Account pool, QR code/browser login, cookie persistence, status validation, and account switching |
| Material collection | Unified Rust `mediacrawler` interface with priority support for Xiaohongshu, Douyin, Bilibili, and phased adapters for other platforms |
| Content publishing | Image-post/video publication plans, account preflight checks, scheduling parameters, `dry_run`, and explicit confirmation gates |
| Plugins and skills | Built-in plugins, skill discovery, plugin settings, and OpenMontage content/video workflows |
| Local data | Tauri/Rust local services, SQLite/file assets, and OS application data directories without requiring all business data to be uploaded to a central cloud |

### Social Media Capability Scope

- Account login and account-pool support cover Xiaohongshu, Douyin, Kuaishou, Bilibili, WeChat Channels, and YouTube.
- Video publishing adapters exist for all six platforms listed above.
- Image-post publishing currently focuses on Xiaohongshu, Douyin, and Kuaishou; confirm the actual adapter status before using other platforms.
- The collection layer contains platform code for Xiaohongshu, Douyin, Bilibili, Kuaishou, Weibo, Baidu Tieba, and Zhihu. Completion and authentication requirements vary, so the presence of an adapter does not mean it has passed production acceptance.
- Login, anti-abuse, and publishing pages change over time. Re-run account-login and small-scope publishing tests before upgrades or production use.

### Document Reading Scope

- Chat attachments are locally parsed for text, table cells, slides, and speaker notes. Knowledge-base documents can be parsed on demand by the same read-only tool. The Agent receives only authorized paths and bounded extracted text.
- The parser does not execute macros, embedded objects, or external links. Each source file is limited to 100 MB, with additional OOXML expansion and compression-ratio limits.
- Scanned PDFs without embedded text require separate OCR. Legacy `.doc` and `.ppt` files must first be saved as `.docx` or `.pptx`; `.xls`, `.xlsb`, and `.ods` files must be saved as `.xlsx`. This feature provides semantic reading, not pixel-perfect Microsoft Office rendering.

## Technical Architecture

```text
desktop/ React + Vite
        │ Tauri IPC
src-tauri/ Tauri 2 desktop shell
        │
crates/yunying-server   Local service, database, IPC, and Goose bridge
crates/yunying-ops      Operations tools and MCP
crates/mediacrawler     Multi-platform material collection
crates/socialconnect    Accounts, login, scheduling, and publishing
builtin-plugins/        Optional plugins such as OpenMontage
```

## Platform Support

| Platform | Source development | Installer artifact | Notes |
| --- | --- | --- | --- |
| macOS Apple Silicon | Supported | `src-tauri/target/release/bundle/dmg/商媒运营助手_0.1.0_aarch64.dmg` | A DMG exists in the current workspace; an unsigned development build may require using “Open” from the context menu |
| Windows x64 | Supported | `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/商媒运营助手_0.1.0_x64-setup.exe` | This is currently a test installer rather than a complete offline runtime bundle |

The paths above refer to artifacts already present in the current workspace. A clean checkout will not contain `target/` and must rebuild them.

### Windows Installer Limitations

The current NSIS script packages the main application, `yunying-ops-mcp.exe`, configuration, and built-in plugins. It does not yet package complete Windows-native `uv.exe`, `ffmpeg.exe`, and `ffprobe.exe` runtimes. As a result:

- The Agent, base UI, Rust backend, and packaged Rust tools can be used for Windows testing.
- Python plugins, video editing, and features that depend on FFmpeg may be unavailable.
- Before shipping a production Windows release, include the native runtime for the target architecture and repeat full-machine acceptance testing.

## Requirements

General requirements:

- Node.js 20 or newer.
- npm; `desktop/package-lock.json` is the reproducible dependency-installation entry point.
- Rust 1.92; the repository's `rust-toolchain.toml` selects this toolchain automatically.
- Tauri CLI 2.x.
- Chrome or Edge for some QR-code login and CDP automation flows.
- Network access to the configured LLM, image, and video providers.

Additional macOS requirements:

- Xcode Command Line Tools: `xcode-select --install`.
- An arm64 toolchain and matching runtime when building the Apple Silicon package.

Additional Windows requirements:

- Visual Studio Build Tools 2022 with “Desktop development with C++”.
- Microsoft Edge WebView2 Runtime.
- NSIS only when using the repository's custom `.nsi` installer script.

Install Tauri CLI the first time:

```bash
cargo install tauri-cli --version "^2" --locked
```

## Configuration

The employee client reads model, collection, social, image, video, and safety settings from `config.json`. On a new environment, copy the example only when the file does not already exist:

```bash
cd src
test -f config.json || cp config.json.example config.json
```

Windows PowerShell:

```powershell
cd src
if (-not (Test-Path config.json)) { Copy-Item config.json.example config.json }
```

At minimum, review:

- `goose.provider`, `goose.model`, `goose.base_url`, and `goose.api_key`.
- Configure `image` when using image generation and `video` when using video generation.
- Keep `safety.run_real_crawler`, `run_real_publish`, `run_real_image`, and `run_real_video` set to `false` until the corresponding test flow has passed.

Do not commit real API keys, cookies, account files, or production configuration.

Tauri adds only workspace `media`, `cover`, and `manuscripts` directories to its read-only preview scope. `knowledge`, `redclaw/uploads`, and other knowledge or staged-document directories are not exposed to the WebView. Spaces under the default `~/.redconvert` root can switch directly; restart after changing `workspace_dir` or switching spaces in a custom workspace so permissions do not accumulate across old directories.

### Connect to the Boss Client

The Task Center can explicitly report a genuinely completed task to the boss client and read the current week's review. The employee Rust backend sends these requests, so the renderer never receives the employee token. Draft, failed, cancelled, and unexecuted tasks are never reported automatically.

Configure the process environment before starting the employee client:

```bash
export BOSS_SERVER_URL="http://127.0.0.1:8787"
export BOSS_OWNER_CODE="BOSS-7429"
export BOSS_EMPLOYEE_TOKEN="replace-with-employee-token-at-least-32-bytes"
export EMPLOYEE_ID="employee-001"
export EMPLOYEE_NAME="Employee Name"
export EMPLOYEE_ROLE="Content Operations"
```

`BOSS_SERVER_URL` defaults to the local Rust server at `http://127.0.0.1:8787`, and `EMPLOYEE_ROLE` defaults to `员工`. The token is optional for a loopback connection. A remote server must use HTTPS and an employee-scoped `BOSS_EMPLOYEE_TOKEN` of at least 32 bytes that only authorizes `employee.*`; never distribute the owner's management token. There is no offline queue yet. A failed report remains retryable, and retrying the same completed task idempotently updates its event.

## Run from Source

### 1. Install Frontend Dependencies

```bash
cd src/desktop
npm ci
```

### 2. Start the Vite Frontend

Terminal one:

```bash
cd src/desktop
npm run dev
```

The frontend listens on `http://127.0.0.1:5173` by default. The Tauri configuration loads it through `http://localhost:5173`.

### 3. Start the Desktop Client

Terminal two:

```bash
cd src/src-tauri
cargo tauri dev
```

Development mode starts the Tauri window, embeds the Rust service, and searches for the local `yunying-ops-mcp`, browser, plugins, and media runtimes as needed. Missing optional runtimes produce warnings and disable the corresponding plugin or video feature, but the base UI can still start.

## Build and Install

### Basic Checks

```bash
cd src/desktop
npm run build

cd ..
./scripts/check.sh check
```

The frontend production build and Rust `check` currently complete successfully. The strict frontend type-checking command is:

```bash
cd src/desktop
npm run typecheck
```

Strict type checking is not yet fully green on the current refactor branch. Known issues are concentrated in IPC type declarations, official-auth interfaces, and the vendored Freecut video editor. These errors were not introduced by the README work. `npm run build` still produces `desktop-dist/`, but the type errors should be resolved before a production release.

### Build on macOS

```bash
cd src/src-tauri
cargo tauri build
```

After a successful build, `.app` and `.dmg` outputs are normally generated under `src-tauri/target/release/bundle/`. Open the DMG and drag “商媒运营助手” into Applications.

### Build on Windows

Run in Windows PowerShell:

```powershell
cd src\desktop
npm ci
npm run build

cd ..\src-tauri
cargo tauri build
```

To use the current custom NSIS script after preparing the x64 application and helper binaries:

```powershell
cd src\src-tauri\installer\windows
makensis cohmira-installer.nsi
```

The installer runs with current-user privileges, installs to `%LOCALAPPDATA%\Programs\商媒运营助手`, and creates Start menu, desktop, and uninstall shortcuts.

### Current Packaging Configuration Note

`src-tauri/tauri.conf.json` now uses `scripts/prepare-tauri-build.mjs` to build the frontend and `yunying-ops-mcp` on either platform, so packaging no longer depends on the old `Beav/desktop` pre-build path. A base Tauri installer can be generated without FFmpeg; video-editing features will then report a missing `ffmpeg-runtime` warning at runtime. `src-tauri/src/main.rs` retains the old location only as a development compatibility fallback.

CI adds the current platform's `uv` executable through `astral-sh/setup-uv`. A complete video-capable package must also provide platform-native `ffmpeg`, `ffprobe`, and required dynamic libraries as Tauri resources and be validated on the target operating system.

## Recommended First-Run Procedure

1. Configure a working model provider and complete one normal Agent conversation.
2. Create a test profile on the Social Accounts page, then verify QR scanning, mobile confirmation, cookie persistence, and persistence after restarting the application.
3. Inspect the collection or publishing plan with `dry_run` before enabling any real write operation.
4. Complete one small-scope image-post or video publication using a test account.
5. Confirm that task queues, team results, knowledge, manuscripts, and media artifacts receive the expected output.
6. Produce a delivery installer only after the target platform, model, FFmpeg runtime, and plugin runtime have all passed validation.

## Common Validation Commands

```bash
# Frontend production build
cd src/desktop
npm run build

# Rust project checks
cd src
./scripts/check.sh check

# Complete tests for the first-party Rust modules; takes longer
./scripts/check.sh test

# Tracks current refactor type debt and may fail at this stage
cd desktop
npm run typecheck

# Build only the operations MCP helper
cd ..
cargo build -p yunying-ops --bin yunying-ops-mcp --features mcp
```

## Data and Security

- Application data, databases, account cookies, and plugin settings are written to OS application-data directories and should not be placed in the source tree or committed to Git.
- Real social publishing requires `confirm=true` and must first pass an account check.
- Automation on external platforms may trigger CAPTCHA or security verification, so retain a manual takeover path.
- Materials produced by collection, generation, and publishing should retain source, execution, and human-review records for later boss-client auditing.
