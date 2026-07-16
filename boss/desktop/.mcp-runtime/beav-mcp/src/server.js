#!/usr/bin/env node
'use strict';

const {
  MCP_PROTOCOL_VERSION,
  MessageParser,
  rpcResult,
  rpcError,
} = require('./protocol');
const {
  SERVER_VERSION,
  createTools,
  listToolDefinitions,
  callTool,
} = require('./tools');

const tools = createTools();
const parser = new MessageParser();

function writeResponse(message) {
  process.stdout.write(parser.frame(message));
}

async function handleRequest(message) {
  if (!message || typeof message !== 'object') return;
  const id = Object.prototype.hasOwnProperty.call(message, 'id') ? message.id : undefined;
  const method = String(message.method || '');
  const params = message.params && typeof message.params === 'object' ? message.params : {};

  if (id === undefined && method.startsWith('notifications/')) {
    return;
  }

  try {
    if (method === 'initialize') {
      writeResponse(rpcResult(id, {
        protocolVersion: String(params.protocolVersion || MCP_PROTOCOL_VERSION),
        capabilities: {
          tools: {},
        },
        serverInfo: {
          name: 'beav-mcp',
          version: SERVER_VERSION,
        },
      }));
      return;
    }

    if (method === 'tools/list') {
      writeResponse(rpcResult(id, {
        tools: listToolDefinitions(tools),
      }));
      return;
    }

    if (method === 'tools/call') {
      const name = String(params.name || '').trim();
      const args = params.arguments && typeof params.arguments === 'object' ? params.arguments : {};
      if (!name) {
        writeResponse(rpcError(id, -32602, 'tools/call requires params.name'));
        return;
      }
      const result = await callTool(tools, name, args);
      writeResponse(rpcResult(id, result));
      return;
    }

    writeResponse(rpcError(id, -32601, `Method not found: ${method}`));
  } catch (error) {
    writeResponse(rpcError(id, -32603, error instanceof Error ? error.message : String(error)));
  }
}

process.stdin.on('data', (chunk) => {
  const messages = parser.push(chunk);
  for (const message of messages) {
    void handleRequest(message);
  }
});

process.stdin.on('end', () => {
  process.exit(0);
});

process.on('SIGTERM', () => process.exit(0));
process.on('SIGINT', () => process.exit(0));

module.exports = {
  handleRequest,
};
