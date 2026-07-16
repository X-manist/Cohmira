#!/usr/bin/env node
"use strict";

const { createMcpServer, JsonMessageFramer, encodeMessage } = require("./server");

const server = createMcpServer({
  env: process.env,
});

const framer = new JsonMessageFramer({
  onMessage: async (message) => {
    const response = await server.handleMessage(message);
    if (response) {
      process.stdout.write(encodeMessage(response));
    }
  },
  onError: (error) => {
    const response = {
      jsonrpc: "2.0",
      id: null,
      error: {
        code: -32700,
        message: "Parse error",
        data: error.message,
      },
    };
    process.stdout.write(encodeMessage(response));
  },
});

process.stdin.on("data", (chunk) => {
  framer.push(chunk);
});

process.stdin.on("end", () => {
  framer.end();
});

process.stdin.resume();
