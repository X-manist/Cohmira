"use strict";

const {
  buildCommand,
  checkAccount,
  createStructuredError,
  listPlatforms,
  loginPrepare,
  sanitizeValue,
  uploadNote,
  uploadVideo,
} = require("./commands");

const PROTOCOL_VERSION = "2024-11-05";

function createServer(options = {}) {
  const runner = options.runner;
  return {
    async handle(message) {
      if (!message || message.jsonrpc !== "2.0") {
        return jsonRpcError(message && message.id, -32600, "Invalid Request");
      }

      try {
        if (message.method === "initialize") {
          return jsonRpcResult(message.id, {
            protocolVersion: PROTOCOL_VERSION,
            capabilities: {
              tools: {},
            },
            serverInfo: {
              name: "social-connection-mcp",
              version: "0.1.0",
            },
          });
        }

        if (message.method === "notifications/initialized") {
          return null;
        }

        if (message.method === "tools/list") {
          return jsonRpcResult(message.id, { tools: toolDefinitions() });
        }

        if (message.method === "tools/call") {
          return jsonRpcResult(message.id, await handleToolCall(message.params || {}, runner));
        }

        return jsonRpcError(message.id, -32601, `Method not found: ${message.method}`);
      } catch (error) {
        return toolErrorResponse(message.id, error);
      }
    },
  };
}

async function handleToolCall(params, runner) {
  const name = params.name;
  const args = params.arguments || {};
  switch (name) {
    case "list_platforms":
      return toolJsonResult({ platforms: listPlatforms() });
    case "build_command":
      return toolJsonResult(buildCommand(args));
    case "check_account":
      return toolJsonResult(await checkAccount(args, runner));
    case "login_prepare":
      return toolJsonResult(await loginPrepare(args));
    case "upload_video":
      return toolJsonResult(await uploadVideo(args, runner));
    case "upload_note":
      return toolJsonResult(await uploadNote(args, runner));
    default:
      throw createStructuredError("UNKNOWN_TOOL", `Unknown tool: ${name}`, { tool: name });
  }
}

function toolJsonResult(value) {
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify(sanitizeValue(value), null, 2),
      },
    ],
  };
}

function toolErrorResponse(id, error) {
  const payload = {
    error: {
      code: error.code || "INTERNAL_ERROR",
      message: error.message || String(error),
      details: sanitizeValue(error.details || {}),
    },
  };
  return jsonRpcResult(id, {
    isError: true,
    content: [
      {
        type: "text",
        text: JSON.stringify(payload, null, 2),
      },
    ],
  });
}

function jsonRpcResult(id, result) {
  return {
    jsonrpc: "2.0",
    id,
    result,
  };
}

function jsonRpcError(id, code, message) {
  return {
    jsonrpc: "2.0",
    id: id === undefined ? null : id,
    error: {
      code,
      message,
    },
  };
}

function toolDefinitions() {
  const basePlatformProperties = {
    platform: {
      type: "string",
      enum: listPlatforms().map((platform) => platform.id),
      description: "sau platform id.",
    },
    account: {
      type: "string",
      description: "User-defined sau account name.",
    },
    sauBin: {
      type: "string",
      description: "Optional sau executable path. Defaults to SOCIAL_CONNECTION_SAU_BIN or sau.",
    },
    sau_bin: {
      type: "string",
      description: "Alias for sauBin.",
    },
  };

  return [
    {
      name: "list_platforms",
      description: "List sau platforms and MCP-supported operations.",
      inputSchema: {
        type: "object",
        additionalProperties: false,
        properties: {},
      },
    },
    {
      name: "build_command",
      description: "Build a sau CLI command without executing it.",
      inputSchema: {
        type: "object",
        additionalProperties: false,
        required: ["platform", "account"],
        anyOf: [{ required: ["operation"] }, { required: ["action"] }],
        properties: buildCommandProperties(basePlatformProperties),
      },
    },
    {
      name: "check_account",
      description: "Run or plan `sau <platform> check --account <name>`. This never publishes content.",
      inputSchema: {
        type: "object",
        additionalProperties: false,
        required: ["platform", "account"],
        properties: {
          ...basePlatformProperties,
          dryRun: {
            type: "boolean",
            description: "When true, only return the check command. Defaults to false.",
          },
          timeoutMs: { type: "integer", minimum: 1 },
        },
      },
    },
    {
      name: "login_prepare",
      description: "Return a sau login command and human guidance. It does not execute interactive login.",
      inputSchema: {
        type: "object",
        additionalProperties: false,
        required: ["platform", "account"],
        properties: {
          ...basePlatformProperties,
          headless: { type: "boolean" },
          headed: { type: "boolean" },
          debug: { type: "boolean" },
        },
      },
    },
    {
      name: "upload_video",
      description:
        "Build or run a sau video upload. Defaults to dry-run; real upload requires confirm=true and dryRun=false.",
      inputSchema: uploadVideoSchema(basePlatformProperties),
    },
    {
      name: "upload_note",
      description:
        "Build or run a sau note/image upload for douyin, kuaishou, or xiaohongshu. Defaults to dry-run; real upload requires confirm=true and dryRun=false.",
      inputSchema: uploadNoteSchema(basePlatformProperties),
    },
  ];
}

function buildCommandProperties(basePlatformProperties) {
  return {
    ...basePlatformProperties,
    operation: {
      type: "string",
      enum: ["check", "login", "upload-video", "upload-note"],
    },
    action: {
      type: "string",
      enum: ["check", "login", "upload-video", "upload-note"],
      description: "Alias for operation.",
    },
    file: { type: "string" },
    title: { type: "string" },
    desc: { type: "string" },
    tags: {
      oneOf: [{ type: "string" }, { type: "array", items: { type: "string" } }],
    },
    schedule: { type: "string", description: "Schedule string accepted by sau, for example 2026-03-24 21:30." },
    thumbnail: { type: "string" },
    thumbnailLandscape: { type: "string" },
    thumbnailPortrait: { type: "string" },
    productLink: { type: "string" },
    productTitle: { type: "string" },
    tid: {
      oneOf: [{ type: "integer" }, { type: "string" }],
      description: "Required for bilibili upload-video.",
    },
    shortTitle: { type: "string" },
    category: { type: "string" },
    draft: { type: "boolean" },
    playlist: { type: "string" },
    visibility: { type: "string", enum: ["public", "unlisted", "private"] },
    images: { type: "array", items: { type: "string" }, minItems: 1 },
    note: { type: "string" },
    bgm: { type: "string", description: "Douyin-only BGM search term." },
    headless: { type: "boolean" },
    headed: { type: "boolean" },
    debug: { type: "boolean" },
    dryRun: { type: "boolean", description: "Execution-control field accepted by check/upload wrappers." },
    confirm: { type: "boolean", description: "Execution-control field accepted by upload wrappers." },
    timeoutMs: { type: "integer", minimum: 1 },
  };
}

function uploadVideoSchema(basePlatformProperties) {
  return {
    type: "object",
    additionalProperties: false,
    required: ["platform", "account", "file", "title"],
    properties: {
      ...basePlatformProperties,
      file: { type: "string" },
      title: { type: "string" },
      desc: { type: "string" },
      tags: {
        oneOf: [{ type: "string" }, { type: "array", items: { type: "string" } }],
      },
      schedule: { type: "string", description: "Schedule string accepted by sau, for example 2026-03-24 21:30." },
      thumbnail: { type: "string" },
      thumbnailLandscape: { type: "string" },
      thumbnailPortrait: { type: "string" },
      productLink: { type: "string" },
      productTitle: { type: "string" },
      tid: {
        oneOf: [{ type: "integer" }, { type: "string" }],
        description: "Required for bilibili.",
      },
      shortTitle: { type: "string" },
      category: { type: "string" },
      draft: { type: "boolean" },
      playlist: { type: "string" },
      visibility: { type: "string", enum: ["public", "unlisted", "private"] },
      headless: { type: "boolean" },
      headed: { type: "boolean" },
      debug: { type: "boolean" },
      dryRun: { type: "boolean", description: "Defaults to true for upload tools." },
      confirm: { type: "boolean", description: "Must be true with dryRun=false to execute upload." },
      timeoutMs: { type: "integer", minimum: 1 },
    },
  };
}

function uploadNoteSchema(basePlatformProperties) {
  return {
    type: "object",
    additionalProperties: false,
    required: ["platform", "account", "images", "title"],
    properties: {
      ...basePlatformProperties,
      images: { type: "array", items: { type: "string" }, minItems: 1 },
      title: { type: "string" },
      note: { type: "string" },
      tags: {
        oneOf: [{ type: "string" }, { type: "array", items: { type: "string" } }],
      },
      schedule: { type: "string" },
      bgm: { type: "string", description: "Douyin-only BGM search term." },
      headless: { type: "boolean" },
      headed: { type: "boolean" },
      debug: { type: "boolean" },
      dryRun: { type: "boolean", description: "Defaults to true for upload tools." },
      confirm: { type: "boolean", description: "Must be true with dryRun=false to execute upload." },
      timeoutMs: { type: "integer", minimum: 1 },
    },
  };
}

module.exports = {
  createServer,
  handleToolCall,
  toolDefinitions,
};
