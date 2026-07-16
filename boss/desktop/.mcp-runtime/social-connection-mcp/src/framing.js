"use strict";

class JsonMessageParser {
  constructor(onMessage) {
    this.onMessage = onMessage;
    this.buffer = Buffer.alloc(0);
  }

  push(chunk) {
    this.buffer = Buffer.concat([this.buffer, Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)]);
    while (this.buffer.length > 0) {
      const parsed = this.tryParseOne();
      if (parsed === null) {
        return;
      }
      if (parsed === undefined) {
        continue;
      }
      this.onMessage(parsed);
    }
  }

  tryParseOne() {
    if (this.startsWithHeader()) {
      const headerEnd = this.buffer.indexOf("\r\n\r\n");
      if (headerEnd < 0) {
        return null;
      }
      const headerText = this.buffer.slice(0, headerEnd).toString("ascii");
      const contentLengthMatch = headerText.match(/(?:^|\r\n)Content-Length:\s*(\d+)/i);
      if (!contentLengthMatch) {
        throw new Error("Content-Length header missing");
      }
      const contentLength = Number.parseInt(contentLengthMatch[1], 10);
      const bodyStart = headerEnd + 4;
      const bodyEnd = bodyStart + contentLength;
      if (this.buffer.length < bodyEnd) {
        return null;
      }
      const body = this.buffer.slice(bodyStart, bodyEnd).toString("utf8");
      this.buffer = this.buffer.slice(bodyEnd);
      return JSON.parse(body);
    }

    const newlineIndex = this.buffer.indexOf("\n");
    if (newlineIndex < 0) {
      return null;
    }
    const line = this.buffer.slice(0, newlineIndex).toString("utf8").trim();
    this.buffer = this.buffer.slice(newlineIndex + 1);
    if (!line) {
      return undefined;
    }
    return JSON.parse(line);
  }

  startsWithHeader() {
    const prefix = this.buffer.slice(0, Math.min(this.buffer.length, 128)).toString("ascii");
    return /^[A-Za-z-]+:\s*/.test(prefix);
  }
}

function encodeContentLengthMessage(message) {
  const body = JSON.stringify(message);
  return `Content-Length: ${Buffer.byteLength(body, "utf8")}\r\n\r\n${body}`;
}

module.exports = {
  JsonMessageParser,
  encodeContentLengthMessage,
};
