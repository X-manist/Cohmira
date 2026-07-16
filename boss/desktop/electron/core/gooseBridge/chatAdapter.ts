import type { GooseMessage, GooseNormalizedEvent } from './types.ts';

export type GooseRuntimeEventType =
  | 'runtime:stream-start'
  | 'runtime:text-delta'
  | 'runtime:done'
  | 'runtime:tool-start'
  | 'runtime:tool-update'
  | 'runtime:tool-end'
  | 'runtime:checkpoint';

export interface GooseRuntimeEventEnvelope {
  eventType: GooseRuntimeEventType;
  sessionId: string;
  taskId?: string | null;
  runtimeId?: string | null;
  parentRuntimeId?: string | null;
  payload?: Record<string, unknown>;
  timestamp: number;
}

export interface GooseChatEventAdapterOptions {
  sessionId: string;
  requestId?: string;
  runtimeId?: string;
  runtimeMode?: string;
}

type ToolCallSnapshot = {
  callId: string;
  name: string;
  input: Record<string, unknown>;
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value));
}

function asRecord(value: unknown): Record<string, unknown> {
  return isRecord(value) ? value : {};
}

function asText(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value == null) return '';
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}

function compactText(value: unknown): string {
  return asText(value).trim();
}

function appendDelta(current: string, incoming: string): { next: string; delta: string } {
  if (!incoming) return { next: current, delta: '' };
  if (!current) return { next: incoming, delta: incoming };
  if (incoming === current || current.endsWith(incoming)) {
    return { next: current, delta: '' };
  }
  if (incoming.startsWith(current)) {
    const delta = incoming.slice(current.length);
    return { next: incoming, delta };
  }
  return { next: `${current}${incoming}`, delta: incoming };
}

function pickFirstText(record: Record<string, unknown>, keys: string[]): string {
  for (const key of keys) {
    const text = asText(record[key]);
    if (text) return text;
  }
  return '';
}

function extractTextParts(message: GooseMessage): { responseText: string; thoughtText: string } {
  const content = Array.isArray(message.content) ? message.content : [];
  let responseText = '';
  let thoughtText = '';

  for (const rawItem of content) {
    if (!isRecord(rawItem)) continue;
    const type = String(rawItem.type || '').trim();
    const text = pickFirstText(rawItem, ['text', 'content', 'delta', 'value']);
    if (!text) continue;

    if (/think|thought|reason/i.test(type)) {
      thoughtText += text;
      continue;
    }
    if (type === 'text' || type === 'assistantText' || type === 'message' || !type) {
      responseText += text;
    }
  }

  return { responseText, thoughtText };
}

function requestIdFromEvent(event: GooseNormalizedEvent): string {
  const requestId = 'requestId' in event ? event.requestId : undefined;
  const chatRequestId = 'chatRequestId' in event ? event.chatRequestId : undefined;
  return compactText(requestId || chatRequestId);
}

function extractToolStarts(message: GooseMessage, event: GooseNormalizedEvent): ToolCallSnapshot[] {
  const content = Array.isArray(message.content) ? message.content : [];
  const tools: ToolCallSnapshot[] = [];

  for (const rawItem of content) {
    if (!isRecord(rawItem)) continue;
    const type = String(rawItem.type || '').trim();
    if (type !== 'toolRequest' && type !== 'frontendToolRequest') continue;
    const snakeToolCall = asRecord(rawItem.tool_call);
    const camelToolCall = asRecord(rawItem.toolCall);
    const toolCall = Object.keys(snakeToolCall).length ? snakeToolCall : camelToolCall;
    const callId = compactText(rawItem.id || toolCall.id || requestIdFromEvent(event) || `goose_tool_${tools.length}`);
    const name = compactText(toolCall.name || rawItem.name || rawItem.tool_name || rawItem.toolName) || 'goose_tool';
    const input = asRecord(toolCall.arguments || rawItem.arguments || rawItem.input);
    tools.push({ callId, name, input });
  }

  return tools;
}

function extractToolEnds(message: GooseMessage, event: GooseNormalizedEvent): Array<{
  callId: string;
  name: string;
  success: boolean;
  content: string;
}> {
  const content = Array.isArray(message.content) ? message.content : [];
  const tools: Array<{ callId: string; name: string; success: boolean; content: string }> = [];

  for (const rawItem of content) {
    if (!isRecord(rawItem)) continue;
    const type = String(rawItem.type || '').trim();
    if (type !== 'toolResponse' && type !== 'frontendToolResponse') continue;
    const result = rawItem.tool_result ?? rawItem.toolResult ?? rawItem.result ?? rawItem.output;
    const resultRecord = asRecord(result);
    const callId = compactText(rawItem.tool_call_id || rawItem.toolCallId || rawItem.id || requestIdFromEvent(event) || `goose_tool_result_${tools.length}`);
    const name = compactText(rawItem.name || rawItem.tool_name || rawItem.toolName) || 'goose_tool';
    const error = compactText(resultRecord.error || rawItem.error);
    const outputText = compactText(resultRecord.content || resultRecord.output || resultRecord.text || resultRecord.result || result);
    tools.push({
      callId,
      name,
      success: !error,
      content: error || outputText,
    });
  }

  return tools;
}

export class GooseChatEventAdapter {
  private readonly sessionId: string;
  private readonly requestId: string;
  private readonly runtimeId: string;
  private readonly runtimeMode: string;
  private responseText = '';
  private thoughtText = '';
  private thoughtStarted = false;
  private finished = false;
  private readonly activeTools = new Map<string, ToolCallSnapshot>();

  constructor(options: GooseChatEventAdapterOptions) {
    this.sessionId = options.sessionId;
    this.requestId = options.requestId || `goose_${Date.now()}`;
    this.runtimeId = options.runtimeId || `goose:${this.requestId}`;
    this.runtimeMode = options.runtimeMode || 'goose';
  }

  getAssistantText(): string {
    return this.responseText;
  }

  accept(event: GooseNormalizedEvent): GooseRuntimeEventEnvelope[] {
    if (this.finished && event.kind !== 'error') return [];
    if (!this.matchesRequest(event)) return [];

    if (event.kind === 'message') {
      return this.acceptMessage(event);
    }
    if (event.kind === 'finish') {
      return this.finish(String(event.reason || 'stop'));
    }
    if (event.kind === 'error') {
      return [this.checkpoint('chat.error', {
        message: 'Goose runtime error',
        raw: event.error,
        hint: '请检查 Goose runtime、MCP 配置和模型配置。',
      })];
    }
    return [];
  }

  finish(reason = 'stop'): GooseRuntimeEventEnvelope[] {
    if (this.finished) return [];
    this.finished = true;
    const events: GooseRuntimeEventEnvelope[] = [];
    if (this.thoughtStarted) {
      events.push(this.checkpoint('chat.thought_end', {}));
    }
    events.push(this.event('runtime:done', {
      status: 'completed',
      content: this.responseText,
      runtimeMode: this.runtimeMode,
      reason,
    }));
    return events;
  }

  fail(error: unknown): GooseRuntimeEventEnvelope[] {
    const message = error instanceof Error ? error.message : String(error || 'Unknown Goose runtime error');
    return [this.checkpoint('chat.error', {
      message: 'Goose runtime 不可用',
      raw: message,
      hint: '请确认 goosed 已安装、可执行文件在 PATH 中，或在设置中配置 Goose runtime 地址。',
    })];
  }

  private matchesRequest(event: GooseNormalizedEvent): boolean {
    const requestId = 'requestId' in event ? event.requestId : undefined;
    const chatRequestId = 'chatRequestId' in event ? event.chatRequestId : undefined;
    if (!requestId && !chatRequestId) return true;
    return requestId === this.requestId || chatRequestId === this.requestId;
  }

  private acceptMessage(event: Extract<GooseNormalizedEvent, { kind: 'message' }>): GooseRuntimeEventEnvelope[] {
    const events: GooseRuntimeEventEnvelope[] = [];
    const textParts = extractTextParts(event.message);

    if (textParts.thoughtText) {
      if (!this.thoughtStarted) {
        this.thoughtStarted = true;
        events.push(this.event('runtime:stream-start', {
          phase: 'thinking',
          runtimeMode: this.runtimeMode,
        }));
      }
      const thought = appendDelta(this.thoughtText, textParts.thoughtText);
      this.thoughtText = thought.next;
      if (thought.delta) {
        events.push(this.event('runtime:text-delta', {
          stream: 'thought',
          content: thought.delta,
          runtimeMode: this.runtimeMode,
        }));
      }
    }

    for (const tool of extractToolStarts(event.message, event)) {
      if (this.activeTools.has(tool.callId)) continue;
      this.activeTools.set(tool.callId, tool);
      events.push(this.event('runtime:tool-start', {
        callId: tool.callId,
        name: tool.name,
        input: tool.input,
        description: `Goose 调用工具：${tool.name}`,
      }));
    }

    for (const tool of extractToolEnds(event.message, event)) {
      const active = this.activeTools.get(tool.callId);
      const name = active?.name || tool.name;
      events.push(this.event('runtime:tool-end', {
        callId: tool.callId,
        name,
        output: {
          success: tool.success,
          content: tool.content,
        },
      }));
      this.activeTools.delete(tool.callId);
    }

    if (textParts.responseText) {
      const response = appendDelta(this.responseText, textParts.responseText);
      this.responseText = response.next;
      if (response.delta) {
        events.push(this.event('runtime:text-delta', {
          stream: 'response',
          content: response.delta,
          runtimeMode: this.runtimeMode,
        }));
      }
    }

    return events;
  }

  private checkpoint(checkpointType: string, payload: Record<string, unknown>): GooseRuntimeEventEnvelope {
    return this.event('runtime:checkpoint', {
      checkpointType,
      summary: checkpointType,
      payload,
    });
  }

  private event(eventType: GooseRuntimeEventType, payload: Record<string, unknown>): GooseRuntimeEventEnvelope {
    return {
      eventType,
      sessionId: this.sessionId,
      taskId: this.requestId,
      runtimeId: this.runtimeId,
      payload,
      timestamp: Date.now(),
    };
  }
}
