#!/usr/bin/env node
"use strict";

const { createServer } = require("./src/mcp");
const { startStdioServer } = require("./src/stdio");

if (require.main === module) {
  const server = createServer();
  startStdioServer(server, process.stdin, process.stdout);
}
