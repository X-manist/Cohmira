"use strict";

const { JsonMessageParser, encodeContentLengthMessage } = require("./framing");

function startStdioServer(server, input, output) {
  const parser = new JsonMessageParser(async (message) => {
    try {
      const response = await server.handle(message);
      if (response) {
        output.write(encodeContentLengthMessage(response));
      }
    } catch (error) {
      const response = jsonRpcError(message && message.id, -32603, error.message || "Internal error");
      output.write(encodeContentLengthMessage(response));
    }
  });

  input.on("data", (chunk) => {
    try {
      parser.push(chunk);
    } catch (error) {
      const response = {
        ...jsonRpcError(null, -32700, error.message || "Parse error"),
      };
      output.write(encodeContentLengthMessage(response));
    }
  });
}

function jsonRpcError(id, code, message) {
  return {
    jsonrpc: "2.0",
    id: id === undefined ? null : id,
    error: {
      code,
      message,
    },
  };
}

module.exports = {
  startStdioServer,
};
