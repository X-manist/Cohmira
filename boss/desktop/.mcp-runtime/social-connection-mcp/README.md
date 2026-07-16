# Social Connection MCP for 九伴智能

MCP stdio server for `social-connection/social-auto-upload` through the `sau` CLI.

九伴智能桌面端内置此 MCP，并通常通过 `operations-mcp` 聚合调用；用户不需要在 UI 里手动添加这个工具。真实发布仍需要本机 `sau` 登录 profile、素材文件和操作者确认。

The server is safe by default:

- `upload_video` and `upload_note` only build and return a command plan unless both `confirm: true` and `dryRun: false` are provided.
- `login_prepare` only returns the interactive login command and QR-code guidance.
- `check_account` may run `sau <platform> check`; pass `dryRun: true` to return only the command.
- Fields whose names look like secrets, tokens, cookies, or passwords are rejected/redacted.

## Scripts

```bash
npm test
npm start
```

Set `SOCIAL_CONNECTION_SAU_BIN` when `sau` is not on `PATH`:

```bash
SOCIAL_CONNECTION_SAU_BIN=/path/to/sau npm start
```

## Standalone MCP config

九伴智能 desktop builds the equivalent config automatically from `Beav/desktop/electron/core/mcpStore.ts`. The manual config below is only for standalone development or external MCP clients.

```json
{
  "mcpServers": {
    "social-connection": {
      "command": "node",
      "args": [
        "/Volumes/macsoftware/codes/agentscompany/yunyingagent/mcps/social-connection-mcp/index.js"
      ],
      "env": {
        "SOCIAL_CONNECTION_SAU_BIN": "sau"
      }
    }
  }
}
```
