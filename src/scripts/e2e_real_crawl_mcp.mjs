#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import process from 'node:process';
import readline from 'node:readline';

const repoRoot = resolve(new URL('..', import.meta.url).pathname);
const binary = resolve(repoRoot, 'target', 'debug', 'yunying-ops-mcp');
const platform = process.env.CRAWL_PLATFORM || 'bilibili';
const keyword = process.env.CRAWL_KEYWORD || '猫爬架';
const limit = Number.parseInt(process.env.CRAWL_LIMIT || '1', 10);
const reportPath = resolve(
  repoRoot,
  `e2e-artifacts/2026-07-16-real-business/real-crawl-${platform}-report.json`,
);
const startedAt = new Date();
const child = spawn(binary, [], {
  cwd: repoRoot,
  env: {
    ...process.env,
    RUN_REAL_CRAWLER: 'true',
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
const lines = readline.createInterface({ input: child.stdout });
lines.on('line', (line) => {
  let message;
  try { message = JSON.parse(line); } catch { return; }
  if (message.id == null) return;
  const waiter = pending.get(String(message.id));
  if (!waiter) return;
  pending.delete(String(message.id));
  if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
  else waiter.resolve(message.result);
});

function request(id, method, params) {
  return new Promise((resolveRequest, rejectRequest) => {
    const timer = setTimeout(() => {
      pending.delete(String(id));
      rejectRequest(new Error(`${method} timed out`));
    }, 30_000);
    pending.set(String(id), {
      resolve(value) { clearTimeout(timer); resolveRequest(value); },
      reject(error) { clearTimeout(timer); rejectRequest(error); },
    });
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id, method, params })}\n`);
  });
}

async function main() {
  const initialize = await request(1, 'initialize', {
    protocolVersion: '2025-03-26',
    capabilities: {},
    clientInfo: { name: 'yunying-real-crawl-e2e', version: '1.0.0' },
  });
  child.stdin.write(`${JSON.stringify({
    jsonrpc: '2.0', method: 'notifications/initialized', params: {},
  })}\n`);
  const result = await request(2, 'tools/call', {
    name: 'start_task',
    arguments: {
      platform,
      keywords: [keyword],
      max_notes_count: limit,
      dry_run: false,
    },
  });
  const text = result?.content?.find((item) => item?.type === 'text')?.text;
  if (!text) throw new Error('start_task returned no text receipt');
  const receipt = JSON.parse(text);
  if (receipt.executed !== true) {
    throw new Error(`real crawl did not execute: ${receipt.error || 'unknown error'}`);
  }
  if (!Number.isInteger(receipt.count) || receipt.count < 1) {
    throw new Error(`real crawl returned no business result: count=${receipt.count}`);
  }

  child.stdin.end();
  const exit = await new Promise((resolveExit) => child.once('exit', (code, signal) => (
    resolveExit({ code, signal })
  )));
  if (exit.code !== 0) throw new Error(`MCP exited abnormally: ${JSON.stringify(exit)}`);

  const completedAt = new Date();
  const first = Array.isArray(receipt.items) ? receipt.items[0] : undefined;
  const report = {
    status: 'passed',
    startedAt: startedAt.toISOString(),
    completedAt: completedAt.toISOString(),
    durationMs: completedAt.getTime() - startedAt.getTime(),
    backend: 'rust-yunying-ops-mcp',
    safetyOverride: 'process-only',
    platform: receipt.platform,
    keyword: receipt.keyword,
    executed: receipt.executed,
    count: receipt.count,
    sample: first ? {
      platform: first.platform,
      contentType: first.content_type,
      platformId: first.platform_id,
      title: first.title,
      publicUrl: first.url,
      authorNickname: first.author?.nickname || '',
      publishedAt: first.published_at,
      viewCount: first.liked_count,
      commentCount: first.comment_count,
    } : null,
    processExit: exit,
    stderr: stderr.trim(),
    secretsRecorded: false,
  };
  await mkdir(dirname(reportPath), { recursive: true });
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
  console.log(`Real crawl passed: count=${report.count}`);
}

main().catch(async (error) => {
  child.kill('SIGTERM');
  const completedAt = new Date();
  const report = {
    status: 'failed',
    startedAt: startedAt.toISOString(),
    completedAt: completedAt.toISOString(),
    durationMs: completedAt.getTime() - startedAt.getTime(),
    backend: 'rust-yunying-ops-mcp',
    safetyOverride: 'process-only',
    platform,
    keyword,
    executed: false,
    count: null,
    error: String(error?.message || error),
    stderr: stderr.trim(),
    secretsRecorded: false,
  };
  await mkdir(dirname(reportPath), { recursive: true });
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
  console.error(report.error);
  process.exitCode = 1;
});
