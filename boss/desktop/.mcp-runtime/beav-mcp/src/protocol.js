'use strict';

const JSONRPC_VERSION = '2.0';
const MCP_PROTOCOL_VERSION = '2024-11-05';

function safeJsonParse(text) {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function frameMessage(message) {
  const body = JSON.stringify(message);
  return `Content-Length: ${Buffer.byteLength(body, 'utf8')}\r\n\r\n${body}`;
}

function findHeaderSeparator(buffer) {
  const crlf = buffer.indexOf('\r\n\r\n');
  if (crlf >= 0) return { index: crlf, length: 4 };
  const lf = buffer.indexOf('\n\n');
  if (lf >= 0) return { index: lf, length: 2 };
  return null;
}

class MessageParser {
  constructor() {
    this.buffer = Buffer.alloc(0);
    this.lastFraming = 'header';
  }

  push(chunk) {
    this.buffer = Buffer.concat([this.buffer, Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk), 'utf8')]);
    const messages = [];

    while (this.buffer.length > 0) {
      const separator = findHeaderSeparator(this.buffer);
      if (separator) {
        const headerText = this.buffer.slice(0, separator.index).toString('utf8');
        const lengthMatch = headerText.match(/(?:^|\r?\n)Content-Length:\s*(\d+)/i);
        if (lengthMatch) {
          const contentLength = Number(lengthMatch[1]);
          const totalLength = separator.index + separator.length + contentLength;
          if (this.buffer.length < totalLength) break;

          const body = this.buffer.slice(separator.index + separator.length, totalLength).toString('utf8');
          this.buffer = this.buffer.slice(totalLength);
          const parsed = safeJsonParse(body);
          if (parsed) {
            this.lastFraming = 'header';
            messages.push(parsed);
          }
          continue;
        }
      }

      if (/^Content-Length:/i.test(this.buffer.toString('utf8', 0, Math.min(this.buffer.length, 32)))) {
        break;
      }

      const newlineIndex = this.buffer.indexOf('\n');
      if (newlineIndex < 0) break;

      const line = this.buffer.slice(0, newlineIndex).toString('utf8').trim();
      this.buffer = this.buffer.slice(newlineIndex + 1);
      if (!line) continue;

      const parsed = safeJsonParse(line);
      if (parsed) {
        this.lastFraming = 'jsonl';
        messages.push(parsed);
      }
    }

    return messages;
  }

  frame(message) {
    return this.lastFraming === 'jsonl'
      ? `${JSON.stringify(message)}\n`
      : frameMessage(message);
  }
}

function rpcResult(id, result) {
  return {
    jsonrpc: JSONRPC_VERSION,
    id,
    result,
  };
}

function rpcError(id, code, message, data) {
  const error = { code, message };
  if (data !== undefined) error.data = data;
  return {
    jsonrpc: JSONRPC_VERSION,
    id,
    error,
  };
}

function toolContent(payload, isError = false) {
  const result = {
    content: [
      {
        type: 'text',
        text: typeof payload === 'string' ? payload : JSON.stringify(payload, null, 2),
      },
    ],
  };
  if (payload && typeof payload === 'object') {
    result.structuredContent = payload;
  }
  if (isError) result.isError = true;
  return result;
}

module.exports = {
  JSONRPC_VERSION,
  MCP_PROTOCOL_VERSION,
  MessageParser,
  frameMessage,
  rpcResult,
  rpcError,
  safeJsonParse,
  toolContent,
};
