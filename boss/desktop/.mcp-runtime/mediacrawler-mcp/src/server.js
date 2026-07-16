"use strict";

const DEFAULT_BASE_URL = "http://127.0.0.1:8080";
const DEFAULT_TIMEOUT_MS = 30000;

const COMPLIANCE_NOTICE = [
  "MediaCrawler MCP is for local, non-commercial learning and research use only.",
  "Respect each target platform's terms, robots.txt, rate limits, login and risk-control flows.",
  "Do not use it for large-scale crawling, illegal collection, account abuse, or privacy-invasive tasks.",
].join(" ");

const PLATFORMS = ["xhs", "dy", "ks", "bili", "wb", "tieba", "zhihu"];
const LOGIN_TYPES = ["qrcode", "phone", "cookie"];
const CRAWLER_TYPES = ["search", "detail", "creator"];
const SAVE_OPTIONS = ["jsonl", "json", "csv", "excel", "sqlite", "db", "mongodb"];
const DATA_FILE_TYPES = ["json", "csv", "xlsx", "xls"];

class McpError extends Error {
  constructor(code, message, data) {
    super(message);
    this.code = code;
    this.data = data;
  }
}

class ToolExecutionError extends Error {
  constructor(message, data) {
    super(message);
    this.data = data;
  }
}

class MediaCrawlerClient {
  constructor(options = {}) {
    this.baseUrl = normalizeBaseUrl(options.baseUrl || DEFAULT_BASE_URL);
    this.fetchImpl = options.fetchImpl || globalThis.fetch;
    this.timeoutMs = options.timeoutMs || DEFAULT_TIMEOUT_MS;
    if (typeof this.fetchImpl !== "function") {
      throw new Error("fetch is not available. Use Node.js >= 18 or inject fetchImpl.");
    }
  }

  async health() {
    return this.request("GET", "/api/health");
  }

  async envCheck() {
    return this.request("GET", "/api/env/check");
  }

  async listPlatforms() {
    return this.request("GET", "/api/config/platforms");
  }

  async startTask(payload) {
    return this.request("POST", "/api/crawler/start", payload);
  }

  async stopTask() {
    return this.request("POST", "/api/crawler/stop", {});
  }

  async getStatus() {
    return this.request("GET", "/api/crawler/status");
  }

  async getLogs(limit) {
    const query = Number.isInteger(limit) ? `?limit=${encodeURIComponent(String(limit))}` : "";
    return this.request("GET", `/api/crawler/logs${query}`);
  }

  async listDataFiles(filters = {}) {
    const params = new URLSearchParams();
    if (filters.platform) {
      params.set("platform", filters.platform);
    }
    if (filters.fileType) {
      params.set("file_type", filters.fileType);
    }
    const query = params.toString();
    return this.request("GET", `/api/data/files${query ? `?${query}` : ""}`);
  }

  async readDataFile(filePath, options = {}) {
    const params = new URLSearchParams();
    params.set("preview", "true");
    params.set("limit", String(options.limit || 100));
    return this.request("GET", `/api/data/files/${encodePath(filePath)}?${params.toString()}`);
  }

  async request(method, path, body) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), this.timeoutMs);
    const url = `${this.baseUrl}${path}`;
    const request = {
      method,
      path,
      baseUrl: this.baseUrl,
      url,
    };
    try {
      const response = await this.fetchImpl(url, {
        method,
        headers: body === undefined ? undefined : { "content-type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: controller.signal,
      });
      const text = await response.text();
      const data = text ? parseJsonResponse(text) : null;
      if (!response.ok) {
        throw new ToolExecutionError(`MediaCrawler API returned HTTP ${response.status}`, {
          kind: "http_error",
          calledBackend: true,
          request,
          httpStatus: response.status,
          response: data,
        });
      }
      return data;
    } catch (error) {
      if (error.name === "AbortError") {
        throw new ToolExecutionError("MediaCrawler API request timed out", {
          kind: "timeout",
          calledBackend: false,
          request,
          timeoutMs: this.timeoutMs,
        });
      }
      if (error instanceof ToolExecutionError) {
        throw error;
      }
      throw new ToolExecutionError("MediaCrawler API request failed", {
        kind: "network_error",
        calledBackend: false,
        request,
        error: error.message,
      });
    } finally {
      clearTimeout(timeout);
    }
  }
}

function createMcpServer(options = {}) {
  const env = options.env || process.env;
  const clientFactory = options.clientFactory || ((args = {}) => new MediaCrawlerClient({
    baseUrl: defaultIfBlank(pickAlias(args, ["baseUrl", "base_url"]), env.MEDIACRAWLER_API_URL || DEFAULT_BASE_URL),
    timeoutMs: normalizeOptionalInteger(
      defaultIfBlank(pickAlias(args, ["timeoutMs", "timeout_ms"]), env.MEDIACRAWLER_API_TIMEOUT_MS),
      "timeoutMs",
      1000,
      300000,
    ) || DEFAULT_TIMEOUT_MS,
    fetchImpl: options.fetchImpl,
  }));

  return {
    async handleMessage(message) {
      let response;
      try {
        response = await handleJsonRpcMessage(message, clientFactory);
      } catch (error) {
        response = jsonRpcError(message && message.id !== undefined ? message.id : null, error);
      }
      return response;
    },
  };
}

async function handleJsonRpcMessage(message, clientFactory) {
  if (!message || message.jsonrpc !== "2.0" || typeof message.method !== "string") {
    throw new McpError(-32600, "Invalid Request");
  }

  const id = message.id;
  if (id === undefined) {
    if (message.method === "notifications/initialized") {
      return null;
    }
    return null;
  }

  if (message.method === "initialize") {
    return jsonRpcResult(id, {
      protocolVersion: chooseProtocolVersion(message.params && message.params.protocolVersion),
      capabilities: {
        tools: {},
      },
      serverInfo: {
        name: "mediacrawler-mcp",
        version: "0.1.0",
      },
      instructions: COMPLIANCE_NOTICE,
    });
  }

  if (message.method === "tools/list") {
    return jsonRpcResult(id, { tools: getTools() });
  }

  if (message.method === "tools/call") {
    const params = message.params || {};
    if (!params.name || typeof params.name !== "string") {
      throw new McpError(-32602, "tools/call requires params.name");
    }
    const result = await callTool(params.name, params.arguments || {}, clientFactory);
    return jsonRpcResult(id, result);
  }

  throw new McpError(-32601, `Method not found: ${message.method}`);
}

async function callTool(name, args, clientFactory) {
  if (!isPlainObject(args)) {
    return toolError("Tool arguments must be an object", { calledBackend: false });
  }

  try {
    switch (name) {
      case "health": {
        const client = clientFactory(args);
        const data = await client.health();
        return toolOk({ tool: name, data }, client, true);
      }
      case "env_check": {
        const client = clientFactory(args);
        const data = await client.envCheck();
        return toolOk({ tool: name, data }, client, true);
      }
      case "list_platforms": {
        const client = clientFactory(args);
        const data = await client.listPlatforms();
        return toolOk({ tool: name, data, supportedByMcp: PLATFORMS }, client, true);
      }
      case "start_task":
        return await startTask(args, clientFactory);
      case "stop_task": {
        const client = clientFactory(args);
        const data = await client.stopTask();
        return toolOk({ tool: name, data }, client, true);
      }
      case "get_status": {
        const client = clientFactory(args);
        const data = await client.getStatus();
        return toolOk({ tool: name, data }, client, true);
      }
      case "get_logs": {
        const client = clientFactory(args);
        const limit = normalizeOptionalInteger(args.limit, "limit", 1, 500);
        const data = await client.getLogs(limit === undefined ? 100 : limit);
        return toolOk({ tool: name, data }, client, true);
      }
      case "list_data_files": {
        const client = clientFactory(args);
        const filters = {
          platform: normalizeOptionalEnum(args.platform, "platform", PLATFORMS),
          fileType: normalizeOptionalEnum(pickAlias(args, ["file_type", "fileType"]), "file_type", DATA_FILE_TYPES),
        };
        const data = await client.listDataFiles(filters);
        return toolOk({
          tool: name,
          data,
          readableFileTypes: DATA_FILE_TYPES,
          note: "MediaCrawler's HTTP preview API currently exposes json/csv/xlsx/xls files. Use save_option=json/csv/excel for MCP-readable crawler outputs.",
        }, client, true);
      }
      case "read_data_file": {
        const client = clientFactory(args);
        const filePath = requireNonEmptyString(pickAlias(args, ["file_path", "filePath"]), "file_path");
        const limit = normalizeOptionalInteger(args.limit, "limit", 1, 500);
        const data = await client.readDataFile(filePath, { limit: limit === undefined ? 100 : limit });
        return toolOk({ tool: name, filePath, data }, client, true);
      }
      default:
        return toolError(`Unknown tool: ${name}`, { calledBackend: false });
    }
  } catch (error) {
    const calledBackend = error.data && typeof error.data.calledBackend === "boolean"
      ? error.data.calledBackend
      : false;
    const requestBaseUrl = error.data && error.data.request && error.data.request.baseUrl;
    return toolError(error.message, {
      calledBackend,
      baseUrl: pickAlias(args, ["baseUrl", "base_url"]) || requestBaseUrl,
      error: error.data || error.stack || error.message,
    });
  }
}

async function startTask(args, clientFactory) {
  const payload = buildStartPayload(args);
  const execute = shouldExecuteStartTask(args);
  const client = clientFactory(args);
  const plan = {
    tool: "start_task",
    dryRun: !execute,
    willCallBackend: execute,
    endpoint: "/api/crawler/start",
    payload,
    safety: {
      requiresExplicitExecution: "Set confirm=true to start the local crawler process. dryRun=false alone is rejected.",
      largeScaleGuardrail: "Use max_notes_count/max_comments_count and platform-compliant rate limits; this MCP does not bypass login or risk-control flows.",
      analysisOutput: "For downstream analysis, prefer save_option=json/csv/excel and then call list_data_files/read_data_file. The backend preview API does not currently list jsonl outputs.",
    },
  };

  if (!execute) {
    return toolOk(plan, client, false);
  }

  const data = await client.startTask(payload);
  return toolOk({ ...plan, data }, client, true);
}

function shouldExecuteStartTask(args) {
  if (args.dryRun === true && args.confirm === true) {
    throw new ToolExecutionError("Conflicting execution flags: dryRun=true cannot be combined with confirm=true");
  }
  if (args.dryRun === false && args.confirm !== true) {
    throw new ToolExecutionError("start_task real execution requires confirm=true; dryRun=false alone is not enough");
  }
  return args.confirm === true;
}

function buildStartPayload(args) {
  const platform = requireEnum(args.platform, "platform", PLATFORMS);
  const crawlerType = requireEnum(
    defaultIfBlank(pickAlias(args, ["crawler_type", "crawlerType"]), "search"),
    "crawler_type",
    CRAWLER_TYPES,
  );
  const payload = {
    platform,
    login_type: requireEnum(defaultIfBlank(pickAlias(args, ["login_type", "loginType"]), "qrcode"), "login_type", LOGIN_TYPES),
    crawler_type: crawlerType,
    keywords: normalizeListString(args.keywords),
    specified_ids: normalizeListString(pickAlias(args, ["specified_ids", "specifiedIds"])),
    creator_ids: normalizeListString(pickAlias(args, ["creator_ids", "creatorIds"])),
    start_page: normalizeOptionalInteger(pickAlias(args, ["start_page", "startPage"]), "start_page", 1, 10000) || 1,
    enable_comments: normalizeBoolean(pickAlias(args, ["enable_comments", "enableComments"]), true),
    enable_sub_comments: normalizeBoolean(pickAlias(args, ["enable_sub_comments", "enableSubComments"]), false),
    save_option: requireEnum(defaultIfBlank(pickAlias(args, ["save_option", "saveOption"]), "jsonl"), "save_option", SAVE_OPTIONS),
    cookies: typeof args.cookies === "string" ? args.cookies : "",
    headless: normalizeBoolean(args.headless, false),
  };

  const maxNotes = normalizeOptionalInteger(pickAlias(args, ["max_notes_count", "maxNotesCount"]), "max_notes_count", 1, 10000);
  if (maxNotes !== undefined) {
    payload.max_notes_count = maxNotes;
  }
  const maxComments = normalizeOptionalInteger(pickAlias(args, ["max_comments_count", "maxCommentsCount"]), "max_comments_count", 1, 10000);
  if (maxComments !== undefined) {
    payload.max_comments_count = maxComments;
  }

  if (crawlerType === "search" && !payload.keywords) {
    payload.keywords = "";
  }
  if (crawlerType === "detail" && !payload.specified_ids) {
    throw new ToolExecutionError("detail mode requires specified_ids/specifiedIds");
  }
  if (crawlerType === "creator" && !payload.creator_ids) {
    throw new ToolExecutionError("creator mode requires creator_ids/creatorIds");
  }

  return payload;
}

function getTools() {
  return [
    {
      name: "health",
      description: "Check the local MediaCrawler FastAPI health endpoint.",
      inputSchema: baseSchema({}),
    },
    {
      name: "env_check",
      description: "Run MediaCrawler's local environment check via FastAPI. This may execute `uv run main.py --help` in the MediaCrawler project.",
      inputSchema: baseSchema({}),
    },
    {
      name: "list_platforms",
      description: "List platforms exposed by the local MediaCrawler API and the MCP allow-list.",
      inputSchema: baseSchema({}),
    },
    {
      name: "start_task",
      description: "Plan or start a local MediaCrawler task. Defaults to dry-run and never starts crawling unless confirm=true is explicit.",
      inputSchema: baseSchema({
        platform: enumProperty(PLATFORMS, "Target platform: xhs, dy, ks, bili, wb, tieba, zhihu."),
        login_type: enumProperty(LOGIN_TYPES, "MediaCrawler login type. The MCP does not bypass login or platform risk controls.", "qrcode"),
        loginType: enumProperty(LOGIN_TYPES, "Alias for login_type.", "qrcode"),
        crawler_type: enumProperty(CRAWLER_TYPES, "Task type: keyword search, specific detail IDs, or creator homepage.", "search"),
        crawlerType: enumProperty(CRAWLER_TYPES, "Alias for crawler_type.", "search"),
        keywords: listStringProperty("Keywords for search mode. String or array; arrays are joined with commas."),
        specified_ids: listStringProperty("Post/video IDs for detail mode. String or array; arrays are joined with commas."),
        specifiedIds: listStringProperty("Alias for specified_ids."),
        creator_ids: listStringProperty("Creator/user IDs for creator mode. String or array; arrays are joined with commas."),
        creatorIds: listStringProperty("Alias for creator_ids."),
        start_page: intProperty("Start page for search mode.", 1, 10000, 1),
        startPage: intProperty("Alias for start_page.", 1, 10000, 1),
        enable_comments: boolProperty("Whether to collect first-level comments.", true),
        enableComments: boolProperty("Alias for enable_comments.", true),
        enable_sub_comments: boolProperty("Whether to collect second-level comments.", false),
        enableSubComments: boolProperty("Alias for enable_sub_comments.", false),
        save_option: enumProperty(SAVE_OPTIONS, "MediaCrawler save_data_option.", "jsonl"),
        saveOption: enumProperty(SAVE_OPTIONS, "Alias for save_option. Use json/csv/excel when the result must be readable through read_data_file.", "jsonl"),
        cookies: stringProperty("Optional cookies passed through to MediaCrawler when login_type=cookie. Do not provide secrets to untrusted clients."),
        headless: boolProperty("Pass headless flag to MediaCrawler.", false),
        max_notes_count: intProperty("Optional maximum item count, capped by MediaCrawler API schema.", 1, 10000),
        maxNotesCount: intProperty("Alias for max_notes_count.", 1, 10000),
        max_comments_count: intProperty("Optional maximum comments per item, capped by MediaCrawler API schema.", 1, 10000),
        maxCommentsCount: intProperty("Alias for max_comments_count.", 1, 10000),
        dryRun: boolProperty("Default true. When true, only returns the planned payload and does not call /api/crawler/start. dryRun=false alone is rejected.", true),
        confirm: boolProperty("Explicit execution flag. Set true to call /api/crawler/start; without this flag the task will not start.", false),
      }, ["platform"]),
    },
    {
      name: "stop_task",
      description: "Stop the currently running local MediaCrawler task.",
      inputSchema: baseSchema({}),
    },
    {
      name: "get_status",
      description: "Get local MediaCrawler task status.",
      inputSchema: baseSchema({}),
    },
    {
      name: "get_logs",
      description: "Get recent local MediaCrawler task logs.",
      inputSchema: baseSchema({
        limit: intProperty("Number of log entries to return.", 1, 500, 100),
      }),
    },
    {
      name: "list_data_files",
      description: "List crawler output files exposed by MediaCrawler's local data preview API for downstream analysis workflows.",
      inputSchema: baseSchema({
        platform: enumProperty(PLATFORMS, "Optional platform filter."),
        file_type: enumProperty(DATA_FILE_TYPES, "Optional file type filter. The backend data preview route currently supports json/csv/xlsx/xls."),
        fileType: enumProperty(DATA_FILE_TYPES, "Alias for file_type."),
      }),
    },
    {
      name: "read_data_file",
      description: "Read a bounded preview of one crawler output file from MediaCrawler's local data preview API.",
      inputSchema: baseSchema({
        file_path: stringProperty("Relative path returned by list_data_files, for example xhs/json/search_contents_2026-06-29.json."),
        filePath: stringProperty("Alias for file_path."),
        limit: intProperty("Maximum preview rows/items to return.", 1, 500, 100),
      }),
    },
  ];
}

function baseSchema(properties, required = []) {
  return {
    type: "object",
    additionalProperties: false,
    properties: {
      baseUrl: stringProperty("Override MediaCrawler FastAPI base URL. Defaults to MEDIACRAWLER_API_URL or http://127.0.0.1:8080."),
      base_url: stringProperty("Alias for baseUrl."),
      timeoutMs: intProperty("HTTP timeout in milliseconds.", 1000, 300000, DEFAULT_TIMEOUT_MS),
      timeout_ms: intProperty("Alias for timeoutMs.", 1000, 300000, DEFAULT_TIMEOUT_MS),
      ...properties,
    },
    required,
  };
}

function toolOk(body, client, calledBackend) {
  return {
    content: [
      {
        type: "text",
        text: JSON.stringify({
          ok: true,
          compliance: COMPLIANCE_NOTICE,
          localState: {
            baseUrl: client && client.baseUrl,
            calledBackend,
          },
          ...body,
        }, null, 2),
      },
    ],
  };
}

function toolError(message, body = {}) {
  return {
    isError: true,
    content: [
      {
        type: "text",
        text: JSON.stringify({
          ok: false,
          compliance: COMPLIANCE_NOTICE,
          message,
          localState: {
            calledBackend: Boolean(body.calledBackend),
            baseUrl: body.baseUrl,
          },
          details: body.error,
        }, null, 2),
      },
    ],
  };
}

class JsonMessageFramer {
  constructor(handlers) {
    this.onMessage = handlers.onMessage;
    this.onError = handlers.onError;
    this.buffer = Buffer.alloc(0);
  }

  push(chunk) {
    this.buffer = Buffer.concat([this.buffer, Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)]);
    this.drain();
  }

  end() {
    const rest = this.buffer.toString("utf8").trim();
    this.buffer = Buffer.alloc(0);
    if (rest) {
      this.parseAndEmit(rest);
    }
  }

  drain() {
    while (this.buffer.length > 0) {
      const text = this.buffer.toString("utf8");
      if (/^content-length:/i.test(text)) {
        const headerEnd = text.indexOf("\r\n\r\n");
        if (headerEnd === -1) {
          return;
        }
        const header = text.slice(0, headerEnd);
        const match = header.match(/content-length:\s*(\d+)/i);
        if (!match) {
          this.fail(new Error("Missing Content-Length header"));
          this.buffer = Buffer.alloc(0);
          return;
        }
        const length = Number(match[1]);
        const bodyStart = Buffer.byteLength(text.slice(0, headerEnd + 4));
        if (this.buffer.length < bodyStart + length) {
          return;
        }
        const body = this.buffer.slice(bodyStart, bodyStart + length).toString("utf8");
        this.buffer = this.buffer.slice(bodyStart + length);
        this.parseAndEmit(body);
        continue;
      }

      const newline = text.indexOf("\n");
      if (newline === -1) {
        return;
      }
      const line = text.slice(0, newline).trim();
      this.buffer = this.buffer.slice(Buffer.byteLength(text.slice(0, newline + 1)));
      if (line) {
        this.parseAndEmit(line);
      }
    }
  }

  parseAndEmit(payload) {
    try {
      const message = JSON.parse(payload);
      Promise.resolve(this.onMessage(message)).catch((error) => this.fail(error));
    } catch (error) {
      this.fail(error);
    }
  }

  fail(error) {
    if (this.onError) {
      this.onError(error);
    }
  }
}

function encodeMessage(message) {
  const body = JSON.stringify(message);
  return `Content-Length: ${Buffer.byteLength(body, "utf8")}\r\n\r\n${body}`;
}

function jsonRpcResult(id, result) {
  return { jsonrpc: "2.0", id, result };
}

function jsonRpcError(id, error) {
  return {
    jsonrpc: "2.0",
    id,
    error: {
      code: Number.isInteger(error.code) ? error.code : -32603,
      message: error.message || "Internal error",
      data: error.data,
    },
  };
}

function chooseProtocolVersion(requested) {
  return typeof requested === "string" ? requested : "2024-11-05";
}

function parseJsonResponse(text) {
  try {
    return JSON.parse(text);
  } catch (_error) {
    return { text };
  }
}

function normalizeBaseUrl(baseUrl) {
  return String(baseUrl || DEFAULT_BASE_URL).replace(/\/+$/, "");
}

function pickAlias(args, names) {
  for (const name of names) {
    if (Object.prototype.hasOwnProperty.call(args, name) && args[name] !== undefined) {
      return args[name];
    }
  }
  return undefined;
}

function defaultIfBlank(value, defaultValue) {
  return value === undefined || value === null || value === "" ? defaultValue : value;
}

function normalizeOptionalEnum(value, name, allowed) {
  if (value === undefined || value === null || value === "") {
    return undefined;
  }
  return requireEnum(value, name, allowed);
}

function requireNonEmptyString(value, name) {
  if (typeof value !== "string" || !value.trim()) {
    throw new ToolExecutionError(`${name} is required`);
  }
  return value.trim();
}

function encodePath(filePath) {
  return filePath.split("/").map((part) => encodeURIComponent(part)).join("/");
}

function normalizeListString(value) {
  if (Array.isArray(value)) {
    return value.map((item) => String(item).trim()).filter(Boolean).join(",");
  }
  if (value === undefined || value === null) {
    return "";
  }
  return String(value).trim();
}

function normalizeBoolean(value, defaultValue) {
  if (value === undefined || value === null) {
    return defaultValue;
  }
  if (typeof value === "boolean") {
    return value;
  }
  if (value === "true") {
    return true;
  }
  if (value === "false") {
    return false;
  }
  throw new ToolExecutionError(`Expected boolean value, got ${String(value)}`);
}

function normalizeOptionalInteger(value, name, min, max) {
  if (value === undefined || value === null || value === "") {
    return undefined;
  }
  const number = Number(value);
  if (!Number.isInteger(number) || number < min || number > max) {
    throw new ToolExecutionError(`${name} must be an integer between ${min} and ${max}`);
  }
  return number;
}

function requireEnum(value, name, allowed) {
  if (!allowed.includes(value)) {
    throw new ToolExecutionError(`${name} must be one of: ${allowed.join(", ")}`);
  }
  return value;
}

function isPlainObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function stringProperty(description) {
  return { type: "string", description };
}

function boolProperty(description, defaultValue) {
  return { type: "boolean", description, default: defaultValue };
}

function intProperty(description, minimum, maximum, defaultValue) {
  const schema = { type: "integer", description, minimum, maximum };
  if (defaultValue !== undefined) {
    schema.default = defaultValue;
  }
  return schema;
}

function enumProperty(values, description, defaultValue) {
  const schema = { type: "string", enum: values, description };
  if (defaultValue !== undefined) {
    schema.default = defaultValue;
  }
  return schema;
}

function listStringProperty(description) {
  return {
    anyOf: [
      { type: "string" },
      { type: "array", items: { type: "string" } },
    ],
    description,
  };
}

module.exports = {
  COMPLIANCE_NOTICE,
  DEFAULT_BASE_URL,
  MediaCrawlerClient,
  McpError,
  ToolExecutionError,
  createMcpServer,
  buildStartPayload,
  JsonMessageFramer,
  encodeMessage,
  getTools,
};
