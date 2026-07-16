import {
  type GooseBridgePermissionRequest,
  type GooseNormalizedEvent,
  type GooseSseFrame,
  type GooseToolCallState,
} from './types.ts';

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value));
}

function asString(value: unknown): string | undefined {
  return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function asRecord(value: unknown): Record<string, unknown> | undefined {
  return isRecord(value) ? value : undefined;
}

function requestIdsFrom(value: unknown): string[] {
  return Array.isArray(value)
    ? value.map((item) => String(item || '').trim()).filter(Boolean)
    : [];
}

function parseJsonObject(data?: string): Record<string, unknown> | null {
  if (!data) return null;
  try {
    const parsed = JSON.parse(data);
    return isRecord(parsed) ? parsed : null;
  } catch {
    return null;
  }
}

export class GooseSseParser {
  private buffer = '';

  push(chunk: string | Uint8Array): GooseSseFrame[] {
    this.buffer += typeof chunk === 'string' ? chunk : Buffer.from(chunk).toString('utf-8');
    const normalized = this.buffer.replace(/\r\n/g, '\n').replace(/\r/g, '\n');
    const frames: GooseSseFrame[] = [];
    let start = 0;
    let boundary = normalized.indexOf('\n\n');
    while (boundary >= 0) {
      const raw = normalized.slice(start, boundary);
      const frame = parseSseFrame(raw);
      if (frame) frames.push(frame);
      start = boundary + 2;
      boundary = normalized.indexOf('\n\n', start);
    }
    this.buffer = normalized.slice(start);
    return frames;
  }

  flush(): GooseSseFrame[] {
    const tail = this.buffer;
    this.buffer = '';
    const frame = parseSseFrame(tail);
    return frame ? [frame] : [];
  }
}

export function parseSseFrame(raw: string): GooseSseFrame | null {
  if (!raw.trim()) return null;

  const data: string[] = [];
  const comments: string[] = [];
  const frame: GooseSseFrame = {};

  for (const line of raw.split('\n')) {
    if (!line) continue;
    if (line.startsWith(':')) {
      comments.push(line.slice(1).trimStart());
      continue;
    }
    const separator = line.indexOf(':');
    const field = separator >= 0 ? line.slice(0, separator) : line;
    const rawValue = separator >= 0 ? line.slice(separator + 1) : '';
    const value = rawValue.startsWith(' ') ? rawValue.slice(1) : rawValue;
    if (field === 'data') data.push(value);
    if (field === 'id') frame.id = value;
    if (field === 'event') frame.event = value;
    if (field === 'retry') {
      const retry = Number(value);
      if (Number.isFinite(retry)) frame.retry = retry;
    }
  }

  if (data.length) frame.data = data.join('\n');
  if (comments.length) frame.comment = comments.join('\n');
  return Object.keys(frame).length ? frame : null;
}

export function normalizeGooseSseFrame(frame: GooseSseFrame): GooseNormalizedEvent {
  if (frame.comment?.startsWith('ping') && !frame.data) {
    return { kind: 'ping', id: frame.id };
  }

  const raw = parseJsonObject(frame.data);
  if (!raw) {
    return frame.comment ? { kind: 'ping', id: frame.id } : {
      kind: 'error',
      error: frame.data ? `Invalid SSE JSON: ${frame.data}` : 'Empty SSE data',
    };
  }

  const type = asString(raw.type);
  const requestId = asString(raw.request_id);
  const chatRequestId = asString(raw.chat_request_id);
  const tokenState = asRecord(raw.token_state);

  switch (type) {
    case 'Message':
      return {
        kind: 'message',
        requestId,
        chatRequestId,
        message: asRecord(raw.message) || {},
        tokenState,
        raw,
      };
    case 'Error':
      return {
        kind: 'error',
        requestId,
        chatRequestId,
        error: String(raw.error || 'Unknown Goose error'),
        raw,
      };
    case 'Finish':
      return {
        kind: 'finish',
        requestId,
        chatRequestId,
        reason: String(raw.reason || 'stop'),
        tokenState,
        raw,
      };
    case 'Notification':
      return {
        kind: 'notification',
        requestId,
        chatRequestId,
        notificationRequestId: asString(raw.request_id),
        message: raw.message,
        raw,
      };
    case 'UpdateConversation':
      return {
        kind: 'conversation',
        requestId,
        chatRequestId,
        conversation: raw.conversation,
        raw,
      };
    case 'ActiveRequests':
      return {
        kind: 'active_requests',
        requestIds: requestIdsFrom(raw.request_ids),
        raw,
      };
    case 'Ping':
      return {
        kind: 'ping',
        id: frame.id,
        raw,
      };
    default:
      return {
        kind: 'unknown',
        requestId,
        chatRequestId,
        type,
        raw,
      };
  }
}

export function normalizeGooseEventsFromChunk(
  parser: GooseSseParser,
  chunk: string | Uint8Array,
): GooseNormalizedEvent[] {
  return parser.push(chunk).map(normalizeGooseSseFrame);
}

export function extractToolCallStates(event: GooseNormalizedEvent, sessionId?: string): GooseToolCallState[] {
  if (event.kind !== 'message') return [];
  const content = Array.isArray(event.message.content) ? event.message.content : [];
  const now = Date.now();
  const states: GooseToolCallState[] = [];

  for (const item of content) {
    if (!isRecord(item)) continue;
    if (item.type === 'toolRequest' || item.type === 'frontendToolRequest') {
      const toolCall = asRecord(item.tool_call) || asRecord(item.toolCall) || {};
      states.push({
        id: String(item.id || toolCall.id || event.requestId || `tool_${states.length}`),
        requestId: event.requestId || event.chatRequestId,
        sessionId,
        name: String(toolCall.name || item.name || 'unknown'),
        arguments: asRecord(toolCall.arguments) || {},
        status: 'running',
        updatedAt: now,
      });
    }
    if (item.type === 'toolResponse') {
      const result = item.tool_result ?? item.toolResult;
      states.push({
        id: String(item.id || event.requestId || `tool_${states.length}`),
        requestId: event.requestId || event.chatRequestId,
        sessionId,
        name: 'unknown',
        arguments: {},
        status: isRecord(result) && result.error ? 'failed' : 'completed',
        result,
        error: isRecord(result) ? asString(result.error) : undefined,
        updatedAt: now,
      });
    }
  }

  return states;
}

export function extractPermissionRequests(event: GooseNormalizedEvent, sessionId?: string): GooseBridgePermissionRequest[] {
  if (event.kind !== 'message') return [];
  const content = Array.isArray(event.message.content) ? event.message.content : [];
  const now = Date.now();
  const requests: GooseBridgePermissionRequest[] = [];

  for (const item of content) {
    if (!isRecord(item)) continue;
    const data = asRecord(asRecord(item.data)?.data) || asRecord(item.data);
    const actionType = asString(data?.actionType);
    const isToolConfirmation = item.type === 'toolConfirmationRequest' || actionType === 'toolConfirmation';
    if (!isToolConfirmation) continue;

    const id = String(item.id || data?.id || `permission_${requests.length}`);
    const toolName = String(item.tool_name || item.toolName || data?.tool_name || data?.toolName || 'unknown');
    requests.push({
      id,
      requestId: event.requestId || event.chatRequestId,
      sessionId,
      toolCallId: id,
      toolName,
      arguments: asRecord(item.arguments) || asRecord(data?.arguments) || {},
      prompt: asString(item.prompt) || asString(data?.prompt),
      status: 'pending',
      createdAt: now,
    });
  }

  return requests;
}
