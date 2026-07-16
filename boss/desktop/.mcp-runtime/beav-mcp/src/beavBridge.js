'use strict';

const DEFAULT_BRIDGE_URL = 'http://127.0.0.1:23456';
const DEFAULT_BRIDGE_PATH = '/mcp/beav';
const DEFAULT_TIMEOUT_MS = 15000;

class BeavBridgeError extends Error {
  constructor(message, details = {}) {
    super(message);
    this.name = 'BeavBridgeError';
    this.code = details.code || 'BEAV_BRIDGE_ERROR';
    this.details = details;
  }
}

function normalizeBaseUrl(input) {
  return String(input || DEFAULT_BRIDGE_URL).replace(/\/+$/g, '');
}

function buildBridgeConfig(env = process.env) {
  return {
    baseUrl: normalizeBaseUrl(env.BEAV_BRIDGE_URL || env.BEAV_LOCAL_BRIDGE_URL),
    path: String(env.BEAV_BRIDGE_PATH || DEFAULT_BRIDGE_PATH),
    token: String(env.BEAV_BRIDGE_TOKEN || env.BEAV_LOCAL_BRIDGE_TOKEN || ''),
    timeoutMs: Number(env.BEAV_BRIDGE_TIMEOUT_MS || DEFAULT_TIMEOUT_MS) || DEFAULT_TIMEOUT_MS,
  };
}

class BeavBridgeClient {
  constructor(config = {}) {
    const defaults = buildBridgeConfig();
    this.baseUrl = normalizeBaseUrl(config.baseUrl || defaults.baseUrl);
    this.path = String(config.path || defaults.path || DEFAULT_BRIDGE_PATH);
    this.token = String(config.token || defaults.token || '');
    this.timeoutMs = Number(config.timeoutMs || defaults.timeoutMs || DEFAULT_TIMEOUT_MS);
  }

  get endpoint() {
    return `${this.baseUrl}${this.path.startsWith('/') ? this.path : `/${this.path}`}`;
  }

  async callAction(action, payload = {}) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), this.timeoutMs);
    const body = {
      action,
      payload,
      source: 'beav-mcp',
    };

    try {
      const headers = {
        'content-type': 'application/json',
        accept: 'application/json',
      };
      if (this.token) headers.authorization = `Bearer ${this.token}`;

      const response = await fetch(this.endpoint, {
        method: 'POST',
        headers,
        body: JSON.stringify(body),
        signal: controller.signal,
      });

      const text = await response.text();
      let parsed = null;
      if (text) {
        try {
          parsed = JSON.parse(text);
        } catch {
          parsed = { rawText: text };
        }
      }

      if (!response.ok) {
        const bridgeError = parsed && typeof parsed === 'object' ? parsed.error : null;
        if (bridgeError && typeof bridgeError === 'object') {
          throw new BeavBridgeError(String(bridgeError.message || `Beav bridge returned HTTP ${response.status}`), {
            code: String(bridgeError.code || 'BEAV_BRIDGE_HTTP_ERROR'),
            status: response.status,
            endpoint: this.endpoint,
            response: parsed,
          });
        }
        throw new BeavBridgeError(`Beav bridge returned HTTP ${response.status}`, {
          code: 'BEAV_BRIDGE_HTTP_ERROR',
          status: response.status,
          endpoint: this.endpoint,
          response: parsed,
        });
      }

      if (parsed && typeof parsed === 'object' && parsed.error) {
        throw new BeavBridgeError(String(parsed.error.message || parsed.error || 'Beav bridge action failed'), {
          code: String(parsed.error.code || 'BEAV_BRIDGE_ACTION_ERROR'),
          endpoint: this.endpoint,
          response: parsed,
        });
      }

      if (parsed && typeof parsed === 'object' && Object.prototype.hasOwnProperty.call(parsed, 'result')) {
        return parsed.result;
      }
      return parsed;
    } catch (error) {
      if (error instanceof BeavBridgeError) throw error;
      if (error && error.name === 'AbortError') {
        throw new BeavBridgeError(`Beav bridge timed out after ${this.timeoutMs}ms`, {
          code: 'BEAV_BRIDGE_TIMEOUT',
          endpoint: this.endpoint,
          timeoutMs: this.timeoutMs,
        });
      }
      throw new BeavBridgeError('Beav local bridge is not reachable. Start Beav Desktop or configure BEAV_BRIDGE_URL.', {
        code: 'BEAV_BRIDGE_UNAVAILABLE',
        endpoint: this.endpoint,
        cause: error instanceof Error ? error.message : String(error),
      });
    } finally {
      clearTimeout(timer);
    }
  }

  listWorkspaces() {
    return this.callAction('app_cli', {
      command: 'spaces list',
      payload: {},
    });
  }

  navigate(args) {
    return this.callAction('navigate', args);
  }

  runAppCommand(command, payload = {}) {
    return this.callAction('app_cli', { command, payload });
  }

  createNote(args) {
    return this.runAppCommand(`manuscripts write --path ${quoteArg(args.path)}`, {
      content: args.content,
      metadata: args.metadata || {},
    });
  }

  callTool(name, args = {}) {
    return this.callAction('tool_call', { name, arguments: args });
  }
}

function quoteArg(value) {
  const text = String(value || '');
  return `"${text.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

function bridgeErrorPayload(error, context = {}) {
  if (error instanceof BeavBridgeError) {
    return {
      ok: false,
      error: {
        code: error.code,
        message: error.message,
        ...error.details,
      },
      context,
    };
  }
  return {
    ok: false,
    error: {
      code: 'BEAV_MCP_ERROR',
      message: error instanceof Error ? error.message : String(error),
    },
    context,
  };
}

module.exports = {
  DEFAULT_BRIDGE_URL,
  DEFAULT_BRIDGE_PATH,
  BeavBridgeClient,
  BeavBridgeError,
  buildBridgeConfig,
  bridgeErrorPayload,
  quoteArg,
};
