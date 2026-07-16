#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { dirname, resolve, sep } from 'node:path';
import process from 'node:process';
import readline from 'node:readline';

const repoRoot = resolve(new URL('..', import.meta.url).pathname);
const binary = resolve(process.env.YUNYING_OPS_MCP || resolve(repoRoot, 'target', 'debug', 'yunying-ops-mcp'));
const artifactRoot = resolve(process.env.E2E_ARTIFACT_DIR || resolve(repoRoot, 'e2e-artifacts', 'create-note-mcp'));
const dataDir = resolve(process.env.YUNYING_DATA_DIR || resolve(artifactRoot, 'data'));
const resultPath = resolve(artifactRoot, 'logs', 'create-note-mcp-e2e.json');
mkdirSync(dirname(resultPath), { recursive: true });
mkdirSync(dataDir, { recursive: true });

const title = '员工端 MCP 持久化日报验收';
const body = [
  '## 今日完成',
  '',
  '- 验证 create_note 真实原子落盘',
  '- 验证 MCP 进程退出后正文仍可读取',
  '- 未执行任何真实发布或付费生成',
].join('\n');
const result = {
  startedAt: new Date().toISOString(),
  binary,
  dataDir,
  title,
  body,
};

function fail(message) {
  throw new Error(message);
}

async function main() {
  if (!existsSync(binary)) fail(`MCP binary 不存在：${binary}`);
  const child = spawn(binary, [], {
    cwd: artifactRoot,
    env: {
      ...process.env,
      YUNYING_DATA_DIR: dataDir,
      RUN_REAL_CRAWLER: 'false',
      RUN_REAL_IMAGE: 'false',
      RUN_REAL_VIDEO: 'false',
      RUN_REAL_PUBLISH: 'false',
    },
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  const pending = new Map();
  const protocolLines = [];
  const lineReader = readline.createInterface({ input: child.stdout });
  lineReader.on('line', (line) => {
    const trimmed = line.trim();
    if (!trimmed) return;
    protocolLines.push(trimmed);
    let message;
    try {
      message = JSON.parse(trimmed);
    } catch (error) {
      for (const waiter of pending.values()) waiter.reject(error);
      pending.clear();
      return;
    }
    if (message.id == null) return;
    const waiter = pending.get(String(message.id));
    if (!waiter) return;
    pending.delete(String(message.id));
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result);
  });

  const send = (message) => {
    child.stdin.write(`${JSON.stringify(message)}\n`);
  };
  const request = (id, method, params) => new Promise((resolveRequest, rejectRequest) => {
    const timer = setTimeout(() => {
      pending.delete(String(id));
      rejectRequest(new Error(`等待 MCP ${method} 响应超时`));
    }, 10_000);
    pending.set(String(id), {
      resolve(value) { clearTimeout(timer); resolveRequest(value); },
      reject(error) { clearTimeout(timer); rejectRequest(error); },
    });
    send({ jsonrpc: '2.0', id, method, params });
  });

  const initialize = await request(1, 'initialize', {
    protocolVersion: '2025-03-26',
    capabilities: {},
    clientInfo: { name: 'yunying-create-note-e2e', version: '1.0.0' },
  });
  send({ jsonrpc: '2.0', method: 'notifications/initialized', params: {} });
  const callResult = await request(2, 'tools/call', {
    name: 'create_note',
    arguments: { title, body },
  });

  const text = Array.isArray(callResult?.content)
    ? callResult.content.find((item) => item?.type === 'text')?.text
    : '';
  if (!text) fail(`create_note 未返回文本内容：${JSON.stringify(callResult)}`);
  const receipt = JSON.parse(text);
  if (receipt.success !== true || receipt.persisted !== true) {
    fail(`create_note 没有确认真实持久化：${text}`);
  }
  if (!receipt.path || !existsSync(receipt.path)) fail(`create_note 返回路径不存在：${receipt.path || '(empty)'}`);
  const canonicalDataDir = realpathSync(dataDir);
  const canonicalNotePath = realpathSync(receipt.path);
  if (!canonicalNotePath.startsWith(`${canonicalDataDir}${sep}`)) {
    fail(`笔记逃逸 YUNYING_DATA_DIR：${canonicalNotePath}`);
  }

  child.stdin.end();
  const exit = await new Promise((resolveExit) => {
    const timeout = setTimeout(() => {
      child.kill('SIGTERM');
      resolveExit({ code: null, signal: 'SIGTERM-timeout' });
    }, 5_000);
    child.once('exit', (code, signal) => {
      clearTimeout(timeout);
      resolveExit({ code, signal });
    });
  });
  if (exit.code !== 0) fail(`MCP 进程退出异常：${JSON.stringify(exit)} stderr=${stderr}`);

  // 进程已经退出；再次从磁盘读取，证明结果不是进程内假句柄。
  const persistedMarkdown = readFileSync(canonicalNotePath, 'utf8');
  if (!persistedMarkdown.includes(body)) fail('进程退出后落盘 Markdown 缺少完整正文');
  if (!persistedMarkdown.includes(`# ${title}`)) fail('进程退出后落盘 Markdown 缺少标题');
  if (!persistedMarkdown.includes(`id: ${receipt.id}`)) fail('进程退出后落盘 Markdown 缺少返回 ID');

  Object.assign(result, {
    completedAt: new Date().toISOString(),
    success: true,
    initializeProtocolVersion: initialize?.protocolVersion || '',
    receipt,
    processExit: exit,
    persistedBytes: Buffer.byteLength(persistedMarkdown),
    protocolMessageCount: protocolLines.length,
    stderr: stderr.trim(),
  });
  writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
  console.log(`create-note MCP E2E OK: ${canonicalNotePath}`);
}

main().catch((error) => {
  Object.assign(result, {
    completedAt: new Date().toISOString(),
    success: false,
    error: error instanceof Error ? error.stack || error.message : String(error),
  });
  writeFileSync(resultPath, `${JSON.stringify(result, null, 2)}\n`);
  console.error(error);
  process.exitCode = 1;
});
