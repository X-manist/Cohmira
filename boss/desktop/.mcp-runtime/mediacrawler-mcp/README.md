# MediaCrawler MCP for 九伴智能

Safe stdio MCP server for a local MediaCrawler FastAPI service.

九伴智能桌面端内置此 MCP，并通常通过 `operations-mcp` 聚合调用；用户不需要在 UI 里手动添加这个工具。真实采集仍需要本地 MediaCrawler API、平台登录态和操作者确认。

The server exposes the local API used by MediaCrawler WebUI:

- `health` -> `GET /api/health`
- `env_check` -> `GET /api/env/check`
- `list_platforms` -> `GET /api/config/platforms`
- `start_task` -> `POST /api/crawler/start`
- `stop_task` -> `POST /api/crawler/stop`
- `get_status` -> `GET /api/crawler/status`
- `get_logs` -> `GET /api/crawler/logs`
- `list_data_files` -> `GET /api/data/files`
- `read_data_file` -> `GET /api/data/files/{file_path}?preview=true`

`start_task` is safe by default. It returns a plan and does not call the backend unless `confirm=true` is explicitly provided. `dryRun=false` without `confirm=true` is rejected. This MCP does not bypass MediaCrawler login, target platform risk controls, terms of service, robots.txt, or rate limits.

## Operational fit

This MCP is suitable as the local MediaCrawler entry point for operations workflows such as viral content analysis, paid traffic research, and creator-library building when the caller treats it as a bounded local crawler controller:

- Use `start_task` in dry-run mode first to inspect the exact MediaCrawler payload.
- Use `max_notes_count`/`max_comments_count` for bounded collection.
- Use `confirm=true` only after the operator has approved the target platform, mode, keywords or IDs, and limits.
- Use `list_data_files` and `read_data_file` to hand collected output back to 九伴智能 / Goose for analysis.

MediaCrawler's current HTTP data preview API lists `json`, `csv`, `xlsx`, and `xls` files. If downstream MCP consumers need to read output through `read_data_file`, prefer `save_option=json`, `save_option=csv`, or `save_option=excel` in `start_task`. The MediaCrawler backend can write `jsonl`, but its preview route does not currently list JSONL files.

Common aliases are exposed in `tools/list`: `loginType`, `crawlerType`, `specifiedIds`, `creatorIds`, `startPage`, `enableComments`, `enableSubComments`, `saveOption`, `maxNotesCount`, `maxCommentsCount`, `base_url`, and `timeout_ms`.

## Usage

Start MediaCrawler API separately:

```sh
cd ../../MediaCrawler
uv run uvicorn api.main:app --port 8080 --reload
```

Run the MCP server:

```sh
cd mcps/mediacrawler-mcp
npm start
```

Configure a different API URL with:

```sh
MEDIACRAWLER_API_URL=http://127.0.0.1:8080 npm start
```

## Standalone MCP config

九伴智能 desktop builds the equivalent config automatically from `Beav/desktop/electron/core/mcpStore.ts`. The manual config below is only for standalone development or external MCP clients.

```json
{
  "mcpServers": {
    "mediacrawler": {
      "command": "node",
      "args": ["/Volumes/macsoftware/codes/agentscompany/yunyingagent/mcps/mediacrawler-mcp/src/index.js"],
      "env": {
        "MEDIACRAWLER_API_URL": "http://127.0.0.1:8080"
      }
    }
  }
}
```

## Tests

```sh
npm test
```
