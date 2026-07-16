import { EventEmitter } from 'node:events';
import { spawn, type ChildProcess } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import {
  buildGooseHeaders,
  buildGooseReplyBody,
  buildGooseSidecarCommand,
  buildGooseUrl,
  createGooseUserTextMessage,
  selectGooseReplyEndpoint,
} from './config.ts';
import { GooseBridgeTaskQueue } from './queue.ts';
import {
  GooseSseParser,
  normalizeGooseSseFrame,
} from './sse.ts';
import type {
  GooseBridgeConfig,
  GooseBridgeTaskSnapshot,
  GooseNormalizedEvent,
  GooseSidecarCommand,
  GooseMessage,
  GooseSessionMapping,
} from './types.ts';

export interface GooseBridgeServiceStatus {
  running: boolean;
  pid?: number;
  command: GooseSidecarCommand;
  baseUrl: string;
  queue: GooseBridgeTaskSnapshot[];
  lastError: string | null;
  sessionMappings: GooseSessionMapping[];
}

export interface GooseBridgeSendInput {
  sessionId?: string;
  requestId?: string;
  text?: string;
  message?: GooseMessage;
  overrideConversation?: GooseMessage[];
  config?: GooseBridgeConfig;
}

export interface GooseBridgeSendResult {
  success: true;
  requestId: string;
  sessionId?: string;
  eventCount: number;
}

type SpawnImpl = typeof spawn;
type EventSink = (event: GooseNormalizedEvent) => void;

const DEFAULT_CONFIG: GooseBridgeConfig = {
  host: '127.0.0.1',
  port: 3000,
  tls: false,
  useSessionEvents: true,
  longTermMemory: {
    enabled: true,
  },
  mcp: [],
};

function mergeConfig(base: GooseBridgeConfig, override?: GooseBridgeConfig): GooseBridgeConfig {
  return {
    ...base,
    ...(override || {}),
    env: {
      ...(base.env || {}),
      ...(override?.env || {}),
    },
    mcp: override?.mcp || base.mcp || [],
    longTermMemory: {
      ...(base.longTermMemory || {}),
      ...(override?.longTermMemory || {}),
    },
  };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function buildBaseUrl(config: GooseBridgeConfig): string {
  const protocol = config.tls === false ? 'http' : 'https';
  return config.baseUrl || `${protocol}://${config.host || '127.0.0.1'}:${config.port || 3000}`;
}

function normalizePathForGoose(value?: string): string {
  const trimmed = String(value || '').trim();
  return trimmed || process.cwd();
}

function extractGooseSessionId(value: unknown): string {
  if (!value || typeof value !== 'object') return '';
  const record = value as Record<string, unknown>;
  return String(record.id || record.session_id || record.sessionId || '').trim();
}

function resolveProviderRuntimeConfig(config: GooseBridgeConfig): {
  provider: string;
  model: string;
  contextLimit?: number;
  requestParams?: Record<string, unknown>;
} | null {
  const provider = String(config.provider || config.env?.GOOSE_PROVIDER || process.env.GOOSE_PROVIDER || '').trim();
  const model = String(config.model || config.env?.GOOSE_MODEL || process.env.GOOSE_MODEL || '').trim();
  if (!provider || !model) return null;
  return {
    provider,
    model,
    contextLimit: config.contextLimit,
    requestParams: config.requestParams,
  };
}

async function parseSseResponse(
  response: Response,
  emit: EventSink,
): Promise<number> {
  const parser = new GooseSseParser();
  let eventCount = 0;
  const emitFrame = (chunk: string | Uint8Array) => {
    for (const frame of parser.push(chunk)) {
      emit(normalizeGooseSseFrame(frame));
      eventCount += 1;
    }
  };

  if (response.body && typeof response.body.getReader === 'function') {
    const reader = response.body.getReader();
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value) emitFrame(value);
    }
  } else {
    const text = await response.text();
    emitFrame(text);
  }

  for (const frame of parser.flush()) {
    emit(normalizeGooseSseFrame(frame));
    eventCount += 1;
  }
  return eventCount;
}

function isTerminalEvent(event: GooseNormalizedEvent, requestId: string): boolean {
  const eventRequestId = 'requestId' in event ? event.requestId : undefined;
  const chatRequestId = 'chatRequestId' in event ? event.chatRequestId : undefined;
  const matchesRequest = !eventRequestId && !chatRequestId
    ? false
    : eventRequestId === requestId || chatRequestId === requestId;
  return matchesRequest && (event.kind === 'finish' || event.kind === 'error');
}

async function parseSessionEventStream(
  response: Response,
  requestId: string,
  emit: EventSink,
): Promise<number> {
  const parser = new GooseSseParser();
  let eventCount = 0;
  let done = false;
  const emitFrame = (chunk: string | Uint8Array) => {
    for (const frame of parser.push(chunk)) {
      const event = normalizeGooseSseFrame(frame);
      emit(event);
      eventCount += 1;
      if (isTerminalEvent(event, requestId)) {
        done = true;
        break;
      }
    }
  };

  if (response.body && typeof response.body.getReader === 'function') {
    const reader = response.body.getReader();
    try {
      while (!done) {
        const { done: streamDone, value } = await reader.read();
        if (streamDone) break;
        if (value) emitFrame(value);
      }
    } finally {
      await reader.cancel().catch(() => undefined);
    }
  } else {
    const text = await response.text();
    emitFrame(text);
  }

  if (!done) {
    for (const frame of parser.flush()) {
      const event = normalizeGooseSseFrame(frame);
      emit(event);
      eventCount += 1;
      if (isTerminalEvent(event, requestId)) break;
    }
  }
  return eventCount;
}

export class GooseBridgeService extends EventEmitter {
  private config: GooseBridgeConfig;
  private readonly queue = new GooseBridgeTaskQueue();
  private readonly fetchImpl: typeof fetch;
  private readonly spawnImpl: SpawnImpl;
  private child: ChildProcess | null = null;
  private lastError: string | null = null;
  private readonly sessionMappings = new Map<string, GooseSessionMapping>();

  constructor(options: {
    config?: GooseBridgeConfig;
    fetchImpl?: typeof fetch;
    spawnImpl?: SpawnImpl;
  } = {}) {
    super();
    this.config = mergeConfig(DEFAULT_CONFIG, options.config);
    this.fetchImpl = options.fetchImpl || fetch;
    this.spawnImpl = options.spawnImpl || spawn;
  }

  getStatus(): GooseBridgeServiceStatus {
    return {
      running: Boolean(this.child && !this.child.killed),
      pid: this.child?.pid,
      command: buildGooseSidecarCommand(this.config),
      baseUrl: buildBaseUrl(this.config),
      queue: this.queue.snapshot(),
      lastError: this.lastError,
      sessionMappings: Array.from(this.sessionMappings.values()),
    };
  }

  start(config?: GooseBridgeConfig): GooseBridgeServiceStatus {
    this.config = mergeConfig(this.config, config);
    if (this.child && !this.child.killed) {
      return this.getStatus();
    }

    const command = buildGooseSidecarCommand(this.config);
    const child = this.spawnImpl(command.command, command.args, {
      cwd: command.cwd,
      env: command.env,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    this.child = child;
    this.lastError = null;

    child.stderr?.on('data', (chunk) => {
      const text = Buffer.from(chunk).toString('utf8').trim();
      if (text) {
        this.emit('log', { level: 'error', text });
      }
    });
    child.stdout?.on('data', (chunk) => {
      const text = Buffer.from(chunk).toString('utf8').trim();
      if (text) {
        this.emit('log', { level: 'info', text });
      }
    });
    child.on('error', (error) => {
      this.lastError = errorMessage(error);
      this.emit('error-event', { message: this.lastError });
    });
    child.on('exit', (code, signal) => {
      this.emit('exit', { code, signal });
      if (this.child === child) {
        this.child = null;
      }
    });

    return this.getStatus();
  }

  stop(): GooseBridgeServiceStatus {
    if (this.child && !this.child.killed) {
      this.child.kill('SIGTERM');
    }
    this.child = null;
    return this.getStatus();
  }

  async sendMessage(input: GooseBridgeSendInput): Promise<GooseBridgeSendResult> {
    const requestId = String(input.requestId || randomUUID()).trim();
    return this.queue.enqueue(requestId, async (signal) => {
      const config = mergeConfig(this.config, input.config);
      const appSessionId = String(input.sessionId || '').trim();
      const gooseSessionId = await this.resolveGooseSessionId(config, appSessionId);
      const endpoint = selectGooseReplyEndpoint(config, gooseSessionId);
      const message = input.message || createGooseUserTextMessage(String(input.text || ''));
      const body = buildGooseReplyBody({
        sessionId: gooseSessionId,
        requestId,
        message,
        overrideConversation: input.overrideConversation,
      }, endpoint);

      if (endpoint.kind === 'session') {
        const eventsController = new AbortController();
        const eventsPromise = this.fetchImpl(endpoint.eventsUrl, {
          method: 'GET',
          headers: buildGooseHeaders(config),
          signal: eventsController.signal,
        }).then(async (eventsResponse) => {
          if (!eventsResponse.ok) {
            const text = await eventsResponse.text().catch(() => '');
            throw new Error(`goose events failed (${eventsResponse.status}): ${text || eventsResponse.statusText}`);
          }
          return parseSessionEventStream(eventsResponse, requestId, (event) => {
            this.emit('event', { ...event, sessionId: appSessionId || gooseSessionId });
          });
        });

        signal.addEventListener('abort', () => {
          eventsController.abort();
        }, { once: true });

        const replyResponse = await this.fetchImpl(endpoint.replyUrl, {
          method: endpoint.method,
          headers: buildGooseHeaders(config),
          body: JSON.stringify(body),
          signal,
        });
        if (!replyResponse.ok) {
          eventsController.abort();
          await eventsPromise.catch(() => 0);
          const text = await replyResponse.text().catch(() => '');
          throw new Error(`goose reply failed (${replyResponse.status}): ${text || replyResponse.statusText}`);
        }
        await replyResponse.json().catch(() => null);
        const eventCount = await eventsPromise.finally(() => {
          eventsController.abort();
        });
        return {
          success: true,
          requestId,
          sessionId: appSessionId || gooseSessionId,
          eventCount,
        };
      }

      const response = await this.fetchImpl(endpoint.replyUrl, {
        method: endpoint.method,
        headers: buildGooseHeaders(config),
        body: JSON.stringify(body),
        signal,
      });
      if (!response.ok) {
        const text = await response.text().catch(() => '');
        throw new Error(`goose reply failed (${response.status}): ${text || response.statusText}`);
      }

      const contentType = response.headers.get('content-type') || '';
      let eventCount = 0;
      if (contentType.includes('text/event-stream')) {
        eventCount = await parseSseResponse(response, (event) => {
          this.emit('event', { ...event, sessionId: appSessionId || gooseSessionId });
        });
      } else {
        const json = await response.json().catch(() => null);
        this.emit('event', {
          kind: 'unknown',
          requestId,
          raw: { response: json },
        });
        eventCount = 1;
      }

      return {
        success: true,
        requestId,
        sessionId: appSessionId || gooseSessionId || undefined,
        eventCount,
      };
    });
  }

  async startSession(input: { sessionId: string; config?: GooseBridgeConfig }): Promise<{ appSessionId: string; gooseSessionId: string }> {
    const config = mergeConfig(this.config, input.config);
    const appSessionId = String(input.sessionId || '').trim();
    if (!appSessionId) {
      throw new Error('sessionId is required');
    }
    const gooseSessionId = await this.resolveGooseSessionId(config, appSessionId);
    return { appSessionId, gooseSessionId };
  }

  private async resolveGooseSessionId(config: GooseBridgeConfig, appSessionId: string): Promise<string> {
    const normalizedAppSessionId = String(appSessionId || '').trim();
    if (!normalizedAppSessionId || config.useSessionEvents === false) {
      return normalizedAppSessionId;
    }

    const existing = this.sessionMappings.get(normalizedAppSessionId);
    if (existing?.gooseSessionId) {
      existing.updatedAt = Date.now();
      return existing.gooseSessionId;
    }

    const response = await this.fetchImpl(buildGooseUrl(config, '/agent/start'), {
      method: 'POST',
      headers: buildGooseHeaders(config),
      body: JSON.stringify({
        working_dir: normalizePathForGoose(config.cwd),
        ...(config.extensionOverrides ? { extension_overrides: config.extensionOverrides } : {}),
      }),
    });
    if (!response.ok) {
      const text = await response.text().catch(() => '');
      throw new Error(`goose agent start failed (${response.status}): ${text || response.statusText}`);
    }

    const json = await response.json().catch(() => null);
    const gooseSessionId = extractGooseSessionId(json);
    if (!gooseSessionId) {
      throw new Error('goose agent start failed: missing session id');
    }

    const now = Date.now();
    this.sessionMappings.set(normalizedAppSessionId, {
      appSessionId: normalizedAppSessionId,
      gooseSessionId,
      createdAt: now,
      updatedAt: now,
    });
    await this.updateProviderFromConfig(config, gooseSessionId);
    return gooseSessionId;
  }

  private async updateProviderFromConfig(config: GooseBridgeConfig, gooseSessionId: string): Promise<void> {
    const runtime = resolveProviderRuntimeConfig(config);
    if (!runtime) return;

    const response = await this.fetchImpl(buildGooseUrl(config, '/agent/update_provider'), {
      method: 'POST',
      headers: buildGooseHeaders(config),
      body: JSON.stringify({
        provider: runtime.provider,
        model: runtime.model,
        session_id: gooseSessionId,
        ...(runtime.contextLimit ? { context_limit: runtime.contextLimit } : {}),
        ...(runtime.requestParams ? { request_params: runtime.requestParams } : {}),
      }),
    });
    if (!response.ok) {
      const text = await response.text().catch(() => '');
      throw new Error(`goose update provider failed (${response.status}): ${text || response.statusText}`);
    }
  }

  async cancel(sessionId: string, config?: GooseBridgeConfig, requestId?: string): Promise<{ success: boolean; skipped?: boolean }> {
    const merged = mergeConfig(this.config, config);
    const mappedSessionId = this.sessionMappings.get(String(sessionId || '').trim())?.gooseSessionId || sessionId;
    const endpoint = selectGooseReplyEndpoint(merged, mappedSessionId);
    if (endpoint.kind !== 'session') {
      return { success: true, skipped: true };
    }
    const normalizedRequestId = String(requestId || '').trim();
    if (!normalizedRequestId) {
      return { success: true, skipped: true };
    }
    const response = await this.fetchImpl(endpoint.cancelUrl, {
      method: 'POST',
      headers: buildGooseHeaders(merged),
      body: JSON.stringify({ request_id: normalizedRequestId }),
    });
    if (!response.ok) {
      const text = await response.text().catch(() => '');
      throw new Error(`goose cancel failed (${response.status}): ${text || response.statusText}`);
    }
    return { success: true };
  }

  cancelTask(requestId: string): boolean {
    const normalized = String(requestId || '').trim();
    if (!normalized) return false;
    return this.queue.cancel(normalized);
  }
}
