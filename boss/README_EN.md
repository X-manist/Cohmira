# Cohmira Command

<p align="center">
  <img src="../branding/cohmira/cohmira-command.svg" alt="Cohmira Command" width="720" />
</p>

**Language: [简体中文](./README.md) | English**

`boss/` is the management client for Cohmira. It gives owners, managers, and finance leads one place to review weekly employee work, operating data, accounting, invoices, and Actual Budget integration status.

[Platform overview](../README_EN.md) · [Employee client guide](../src/README_EN.md)

## Current Architecture

The boss client uses **Tauri 2 + Rust + TypeScript/Vite**:

- `desktop/src-tauri/` is the only active local backend and desktop runtime. The ledger, employee reports, owner reviews, invoice drafts, Actual export/sync jobs, and SQLite access are implemented in Rust.
- `desktop/src/` contains the shared WebView/browser UI.
- The Tauri process starts a Rust HTTP service from the same `BossCore`. `desktop/src-tauri/src/bin/boss-server.rs` provides the browser-development and standalone server entry point. Both expose `POST /api/boss/tool`.
- SQLite is bundled through `rusqlite`; users do not need a separate SQLite installation.
- Tauri 2 builds native macOS and Windows applications. Historical `electron/` and `dist-electron/` files are not active runtime entry points.
- The old Python `server.py` backend has been removed. Development, tests, desktop runtime, and release bundles use the Rust backend and do not require Python.

## Supported Features

| Module | Current capability | Status |
| --- | --- | --- |
| Today overview | Owner actions, weekly team progress, operating net, pending accounting reviews, and management prompts | Rust tool driven |
| Boss AI | Calls `boss.owner_context` and returns a structured management summary | Rule-based summary today; independent model inference remains optional work |
| Employee reports | Week navigation, work events, output, quality, cost, blockers, next-week plan, and owner reviews | Employee app can explicitly report completed work and read feedback |
| Operating ledger | SQLite transactions, manual entry, review confirmation, monthly totals, categories, and pagination | Rust + bundled SQLite |
| Invoices | Parse text or external `ai_ocr_json`, store drafts, and create reviewed ledger entries | Binary images/PDFs are stored safely and marked for OCR/review |
| Actual Budget | Integration status, dry run, auditable jobs, and CSV/JSONL export | Optional; the local ledger works without Actual |
| Employee binding | Owner code, employee records, and query tools | Production identity and organization authorization remain to be added |
| Desktop | Native Tauri 2 window and OS application-data directory | macOS and Windows builds supported |

## Employee-to-Boss Workflow

The employee app in `../src/` executes work. The boss app reviews and manages the result through these Rust tool contracts:

- `employee.report_work_event`: report task output, quality, cost, and exceptions.
- `boss.employee_work_report`: read work by employee and week.
- `boss.save_week_review`: save owner review status and follow-up requests.
- `employee.list_week_reviews`: let the employee read owner feedback.

Only work with a successful execution record can be explicitly reported. Draft, failed, cancelled, and unexecuted tasks are not submitted. Stable event IDs make repeated reports idempotent.

For same-machine use, the boss desktop listens on `127.0.0.1:8787`. Cross-device use must place the Rust server behind HTTPS and configure separate owner and employee tokens of at least 32 bytes. The fixed demo owner code and demo login are not production authentication.

## Directory Layout

```text
boss/
├── desktop/
│   ├── src/                         # TypeScript/Vite UI
│   ├── src-tauri/
│   │   ├── src/lib.rs               # BossCore and tool dispatch
│   │   ├── src/main.rs              # Tauri entry point
│   │   ├── src/bin/boss-server.rs   # Rust HTTP server
│   │   ├── Cargo.toml
│   │   └── tauri.conf.json
│   └── package.json
├── mcps/boss-accounting-mcp/        # Legacy-data migration notes only
├── third_party/actual/               # Optional Actual Budget source
├── docs/
└── README.md
```

## Requirements

- Node.js 22 and npm.
- Rust 1.88 or newer and Cargo, following `desktop/src-tauri/Cargo.toml`.
- Tauri 2 system dependencies: Xcode Command Line Tools on macOS; Microsoft C++ Build Tools and WebView2 on Windows.
- Optional: Actual Budget Server, Sync ID, credential, and default account.

Running, testing, and packaging the boss client does not require Python.

## Install Dependencies

```bash
cd boss/desktop
npm ci
```

## Start the Desktop App

Use the same command on macOS and Windows:

```bash
cd boss/desktop
npm run tauri:dev
```

This starts Vite, compiles the Rust core, and opens the native Tauri window. Desktop calls go directly to `call_boss_tool`; no Python service is started.

## Browser Development

Start the Rust server and Vite separately.

Terminal 1:

```bash
cd boss/desktop/src-tauri
cargo run --bin boss-server
```

Terminal 2:

```bash
cd boss/desktop
npm run dev
```

Open `http://127.0.0.1:5181`. The Rust server listens on `127.0.0.1:8787` by default. `BOSS_SERVER_URL` changes the Vite proxy target and `BOSS_SERVER_ADDR` changes the Rust listen address.

## Employee Synchronization

For same-machine use, point the employee app at `http://127.0.0.1:8787` and configure `BOSS_OWNER_CODE`, `EMPLOYEE_ID`, and `EMPLOYEE_NAME`. `EMPLOYEE_ROLE` is optional.

For remote deployment, put the Rust server behind a trusted HTTPS reverse proxy and configure:

```text
BOSS_SERVER_ADDR=0.0.0.0:8787
BOSS_ALLOW_REMOTE=1
BOSS_SERVER_TOKEN=<random owner token, at least 32 bytes>
BOSS_EMPLOYEE_TOKEN=<different employee token, at least 32 bytes>
```

Employees must receive only `BOSS_EMPLOYEE_TOKEN`. The server rejects unauthenticated requests and prevents employee tokens from calling owner, ledger, or invoice tools. The employee app rejects remote plaintext HTTP endpoints.

## Build macOS and Windows Apps

Run this on the target operating system:

```bash
cd boss/desktop
npm ci
npm run tauri:build
```

Tauri writes platform bundles under `desktop/src-tauri/target/release/bundle/`. Build macOS and Windows artifacts on their native runners. Production distribution also requires platform code signing, Apple notarization, and release certificates.

`.github/workflows/boss-desktop-check.yml` tests the frontend and Rust core on `macos-14` and `windows-2022`, then creates unsigned `.app` and NSIS artifacts.

## Tests

```bash
cd boss/desktop
npm run build
npm run audit:e2e

cd src-tauri
cargo test --all-targets
cargo check --all-targets
```

## Data Location

The Tauri app stores `boss-accounting.sqlite` in the OS application-data directory. Browser development uses the platform data directory. Tests can override it explicitly:

```bash
BOSS_ACCOUNTING_DB=/absolute/path/boss-accounting.sqlite cargo run --bin boss-server
```

Never commit production databases, invoice originals, exports, or Actual credentials.

## Actual Budget (Optional)

The local SQLite ledger, invoices, reports, and exports work without Actual. A real connection typically needs:

```text
ACTUAL_SERVER_URL
ACTUAL_SYNC_ID
ACTUAL_PASSWORD or ACTUAL_TOKEN
ACTUAL_DEFAULT_ACCOUNT_ID
```

The Rust core manages integration status, approval gates, import files, and auditable sync jobs. It does not embed or silently modify Actual Server. Only approved transactions can sync; incomplete configuration returns a dry-run or queued state. See [docs/actual-env-setup.md](./docs/actual-env-setup.md).

## Invoice Recognition Boundary

- Text invoices and structured external `ai_ocr_json` are supported.
- Binary images and PDFs are stored safely and marked for OCR/review; the Rust backend does not guess missing fields.
- Human review is required before final posting or Actual synchronization.
- A future Rust OCR library or external vision adapter must preserve source, confidence, and audit metadata.

## Remaining Production Work

- Production login, organization membership, per-employee credentials, authorization, and tenant isolation.
- Offline queueing, retry, conflict handling, and notifications for cross-device synchronization.
- Stable image/PDF OCR or vision-model integration.
- Production Actual connector retries and audit hardening.
- macOS signing/notarization and Windows installer signing.

## Security

- Boss AI must call the corresponding Rust tool before answering employee, income, expense, profit, invoice, or Actual questions.
- Data not returned by tools must remain unknown or `needs_review`.
- Production deployments must replace the fixed demo owner code and frontend demo login.
- Remote Rust server deployments require TLS, rate limiting, access audit, and token rotation.
