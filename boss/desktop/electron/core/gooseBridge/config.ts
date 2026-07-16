import path from 'node:path';
import {
  type GooseBridgeConfig,
  type GooseEndpointSet,
  type GooseOpenAiEndpointConfig,
  type GooseReplyEndpoint,
  type GooseReplyRequestInput,
  type GooseSidecarCommand,
  type GooseMessage,
} from './types.ts';

const DEFAULT_HOST = '127.0.0.1';
const DEFAULT_PORT = 3000;
const DEFAULT_EXECUTABLE = 'goosed';

function trimTrailingSlashes(value: string): string {
  return value.replace(/\/+$/g, '');
}

function definedEnv(env: Record<string, string | undefined>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) {
    if (value !== undefined && value !== null) {
      result[key] = String(value);
    }
  }
  return result;
}

function normalizePrefix(prefix?: string): string {
  const trimmed = String(prefix || '').trim();
  if (!trimmed || trimmed === '/') return '';
  return `/${trimmed.replace(/^\/+|\/+$/g, '')}`;
}

function normalizeBaseUrl(config: GooseBridgeConfig): string {
  if (config.baseUrl) {
    return trimTrailingSlashes(config.baseUrl);
  }
  const protocol = config.tls === false ? 'http' : 'https';
  const host = config.host || DEFAULT_HOST;
  const port = Number.isFinite(config.port) ? Number(config.port) : DEFAULT_PORT;
  return `${protocol}://${host}:${port}`;
}

function consumePathSuffix(parts: string[], suffix: string[]): boolean {
  if (parts.length < suffix.length) return false;
  const offset = parts.length - suffix.length;
  for (let i = 0; i < suffix.length; i += 1) {
    if (parts[offset + i]?.toLowerCase() !== suffix[i]) return false;
  }
  parts.splice(offset, suffix.length);
  return true;
}

export function splitGooseOpenAiCompatibleEndpoint(rawBaseUrl?: string): GooseOpenAiEndpointConfig {
  const normalized = trimTrailingSlashes(String(rawBaseUrl || '').trim());
  if (!normalized) return {};

  try {
    const parsed = new URL(normalized);
    const parts = trimTrailingSlashes(parsed.pathname || '').split('/').filter(Boolean);
    let basePath = 'v1/chat/completions';

    if (consumePathSuffix(parts, ['v1', 'chat', 'completions'])) {
      basePath = 'v1/chat/completions';
    } else if (consumePathSuffix(parts, ['chat', 'completions'])) {
      basePath = 'chat/completions';
    } else if (consumePathSuffix(parts, ['v1', 'responses'])) {
      basePath = 'v1/responses';
    } else if (consumePathSuffix(parts, ['responses'])) {
      basePath = 'responses';
    } else if (parts[parts.length - 1]?.toLowerCase() === 'v1') {
      parts.pop();
      basePath = 'v1/chat/completions';
    }

    parsed.pathname = parts.length ? `/${parts.join('/')}` : '';
    parsed.search = '';
    parsed.hash = '';

    return {
      host: trimTrailingSlashes(parsed.toString()),
      basePath,
      baseUrl: normalized,
    };
  } catch {
    return { baseUrl: normalized };
  }
}

export function createGooseUserTextMessage(text: string, nowSeconds = Math.floor(Date.now() / 1000)): GooseMessage {
  return {
    id: null,
    role: 'user',
    created: nowSeconds,
    content: [
      {
        type: 'text',
        text,
      },
    ],
    metadata: {
      userVisible: true,
      agentVisible: true,
    },
  };
}

export function buildGooseSidecarCommand(config: GooseBridgeConfig = {}): GooseSidecarCommand {
  const command = config.executablePath || DEFAULT_EXECUTABLE;
  const args = config.commandArgs?.length ? [...config.commandArgs] : ['agent'];
  const port = Number.isFinite(config.port) ? Number(config.port) : DEFAULT_PORT;
  const host = config.host || DEFAULT_HOST;
  const tls = config.tls !== false;
  const env: Record<string, string | undefined> = {
    ...process.env,
    GOOSE_HOST: host,
    GOOSE_PORT: String(port),
    GOOSE_TLS: String(tls),
    GOOSE_SERVER__SECRET_KEY: config.secretKey,
    GOOSE_TLS_CERT_PATH: config.tlsCertPath,
    GOOSE_TLS_KEY_PATH: config.tlsKeyPath,
    GOOSE_HOME: config.gooseHome,
    ...config.env,
  };

  if (config.addBinaryDirToPath && config.executablePath) {
    const pathKey = process.platform === 'win32' ? 'Path' : 'PATH';
    env[pathKey] = `${path.dirname(config.executablePath)}${path.delimiter}${env[pathKey] || ''}`;
  }

  return {
    command,
    args,
    env: definedEnv(env),
    cwd: config.cwd,
  };
}

export function buildGooseHeaders(config: GooseBridgeConfig = {}): Record<string, string> {
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
  };
  if (config.secretKey) {
    headers['X-Secret-Key'] = config.secretKey;
  }
  return headers;
}

export function buildGooseUrl(config: GooseBridgeConfig, route: string): string {
  const baseUrl = normalizeBaseUrl(config);
  const prefix = normalizePrefix(config.endpointPrefix);
  const normalizedRoute = `/${String(route || '').replace(/^\/+/, '')}`;
  return `${baseUrl}${prefix}${normalizedRoute}`;
}

export function buildGooseEndpoints(config: GooseBridgeConfig = {}): GooseEndpointSet {
  return {
    baseUrl: normalizeBaseUrl(config),
    statusUrl: buildGooseUrl(config, '/status'),
    startAgentUrl: buildGooseUrl(config, '/agent/start'),
    resumeAgentUrl: buildGooseUrl(config, '/agent/resume'),
    toolsUrl: buildGooseUrl(config, '/agent/tools'),
    configUrl: buildGooseUrl(config, '/config'),
  };
}

export function selectGooseReplyEndpoint(
  config: GooseBridgeConfig,
  sessionId?: string,
): GooseReplyEndpoint {
  const normalizedSessionId = String(sessionId || '').trim();
  if (config.useSessionEvents !== false && normalizedSessionId) {
    const encoded = encodeURIComponent(normalizedSessionId);
    return {
      kind: 'session',
      method: 'POST',
      sessionId: normalizedSessionId,
      replyUrl: buildGooseUrl(config, `/sessions/${encoded}/reply`),
      eventsUrl: buildGooseUrl(config, `/sessions/${encoded}/events`),
      cancelUrl: buildGooseUrl(config, `/sessions/${encoded}/cancel`),
    };
  }
  return {
    kind: 'legacy',
    method: 'POST',
    replyUrl: buildGooseUrl(config, '/reply'),
  };
}

export function buildGooseReplyBody(input: GooseReplyRequestInput, endpoint: GooseReplyEndpoint): Record<string, unknown> {
  if (endpoint.kind === 'session') {
    return {
      request_id: input.requestId,
      user_message: input.message,
      ...(input.overrideConversation ? { override_conversation: input.overrideConversation } : {}),
    };
  }
  return {
    session_id: input.sessionId,
    user_message: input.message,
    ...(input.overrideConversation ? { override_conversation: input.overrideConversation } : {}),
  };
}
