#!/usr/bin/env node

import { spawn } from 'node:child_process';
import { createWriteStream, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import process from 'node:process';

const repoRoot = resolve(new URL('..', import.meta.url).pathname);
const desktopRoot = resolve(repoRoot, 'desktop');
const artifactRoot = resolve(
  process.env.E2E_ARTIFACT_DIR || resolve(repoRoot, 'e2e-artifacts', 'employee-ui'),
);
const screenshotsDir = resolve(artifactRoot, 'screenshots');
const logsDir = resolve(artifactRoot, 'logs');
const tmpDir = resolve(artifactRoot, 'tmp');
mkdirSync(screenshotsDir, { recursive: true });
mkdirSync(logsDir, { recursive: true });
mkdirSync(tmpDir, { recursive: true });

const vitePort = Number(process.env.E2E_VITE_PORT || 4179);
const cdpPort = Number(process.env.E2E_CDP_PORT || 9339);
const baseUrl = `http://127.0.0.1:${vitePort}`;
const results = {
  startedAt: new Date().toISOString(),
  baseUrl,
  safetyMode: true,
  screenshots: [],
  clicks: [],
  assertions: [],
  browserConsoleErrors: [],
  networkFailures: [],
  ignoredNetworkFailures: [],
  viewAudits: [],
  ipcCalls: [],
};

function findChrome() {
  const candidates = process.platform === 'darwin'
    ? [
        '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
        '/Applications/Chromium.app/Contents/MacOS/Chromium',
      ]
    : process.platform === 'win32'
      ? [
          `${process.env.PROGRAMFILES || 'C:\\Program Files'}\\Google\\Chrome\\Application\\chrome.exe`,
          `${process.env['PROGRAMFILES(X86)'] || 'C:\\Program Files (x86)'}\\Microsoft\\Edge\\Application\\msedge.exe`,
        ]
      : ['/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/chromium-browser'];
  for (const candidate of candidates) {
    try {
      if (candidate && process.getBuiltinModule('node:fs').existsSync(candidate)) return candidate;
    } catch {
      // Try the next browser candidate.
    }
  }
  throw new Error(`未找到可用于隔离 E2E 的 Chromium 浏览器（platform=${process.platform}）`);
}

function attachLogs(child, fileName) {
  const stream = createWriteStream(resolve(logsDir, fileName), { flags: 'w' });
  child.stdout?.pipe(stream, { end: false });
  child.stderr?.pipe(stream, { end: false });
  return stream;
}

async function waitForHttp(url, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
      lastError = new Error(`HTTP ${response.status}`);
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolveDelay) => setTimeout(resolveDelay, 200));
  }
  throw new Error(`等待 ${url} 超时：${String(lastError || '')}`);
}

class CdpClient {
  constructor(url) {
    this.socket = new WebSocket(url);
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = new Map();
  }

  async ready() {
    if (this.socket.readyState === WebSocket.OPEN) return;
    await new Promise((resolveReady, rejectReady) => {
      this.socket.addEventListener('open', resolveReady, { once: true });
      this.socket.addEventListener('error', rejectReady, { once: true });
    });
    this.socket.addEventListener('message', (event) => {
      const message = JSON.parse(String(event.data));
      if (message.id) {
        const pending = this.pending.get(message.id);
        if (!pending) return;
        this.pending.delete(message.id);
        if (message.error) pending.reject(new Error(JSON.stringify(message.error)));
        else pending.resolve(message.result || {});
        return;
      }
      for (const listener of this.listeners.get(message.method) || []) {
        listener(message.params || {});
      }
    });
  }

  on(method, listener) {
    const listeners = this.listeners.get(method) || [];
    listeners.push(listener);
    this.listeners.set(method, listeners);
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolveMessage, rejectMessage) => {
      this.pending.set(id, { resolve: resolveMessage, reject: rejectMessage });
      this.socket.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    this.socket.close();
  }
}

function mockTransportSource() {
  return String.raw`
    (() => {
      const calls = [];
      const listeners = new Map();
      const now = Date.now();
      const iso = (offsetMs = 0) => new Date(now + offsetMs).toISOString();
      const on = (channel, listener) => {
        const entries = listeners.get(channel) || [];
        entries.push(listener);
        listeners.set(channel, entries);
      };
      const off = (channel, listener) => {
        listeners.set(channel, (listeners.get(channel) || []).filter((entry) => entry !== listener));
      };
      const emit = (channel, payload) => {
        for (const listener of listeners.get(channel) || []) {
          try { listener({ channel, e2e: true }, payload); } catch (error) { console.error(error); }
        }
      };
      const settings = {
        api_endpoint: 'http://127.0.0.1:9/v1',
        api_key: 'e2e-placeholder-key',
        model_name: 'e2e-local-model',
        default_ai_source_id: 'e2e-local',
        ai_sources_json: JSON.stringify([{
          id: 'e2e-local',
          name: 'E2E 本地模型',
          baseURL: 'http://127.0.0.1:9/v1',
          apiKey: 'e2e-placeholder-key',
          models: ['e2e-local-model'],
          modelName: 'e2e-local-model',
        }]),
        workspace_dir: '/tmp/yunying-e2e-workspace',
        developer_mode_enabled: true,
      };
      const socialConfig = {
        version: 1,
        mediaCrawler: {
          enabled: false,
          apiUrl: 'http://127.0.0.1:9',
          apiTimeoutMs: 30000,
          defaultPlatform: 'xhs',
          defaultLoginType: 'qrcode',
          saveOption: 'jsonl',
          maxNotesCount: 20,
          maxCommentsCount: 20,
          proxyUrl: '',
          cookies: { xhs: '', douyin: '', bili: '' },
        },
        socialConnection: {
          enabled: false,
          runtimeMode: 'rust-cdp',
          browserExecutable: '',
          dataDir: '/tmp/yunying-e2e-social',
          headless: true,
          proxyUrl: '',
          accounts: {
            xiaohongshu: 'default', douyin: 'default', kuaishou: 'default',
            bilibili: 'default', tencent: 'default', youtube: 'default',
          },
        },
        goose: { inlineMediaCrawler: false, inlineSocialConnection: false },
      };
      const runnerStatus = {
        success: true,
        enabled: false,
        intervalMinutes: 20,
        maxAutomationPerTick: 2,
        capabilities: {
          automaticExecution: false,
          persistence: false,
          note: 'E2E 安全模式：只验证任务定义管理，不执行外部动作。',
        },
        heartbeat: { enabled: false, intervalMinutes: 30, suppressEmptyReport: true, reportToMainSession: true },
        scheduledTasks: {},
        longCycleTasks: {},
      };
      const scheduledTasks = [{
        id: 'weekly-review', name: '每周运营复盘', mode: 'weekly', enabled: true,
        prompt: '汇总本周完成内容、素材产出、风险和下周计划。',
        nextRunAt: iso(86400000), createdAt: iso(-604800000), updatedAt: iso(-3600000),
        lastRunAt: null, lastResult: null,
      }];
      const longCycleTasks = [{
        id: 'growth-sprint', name: '30 天增长冲刺', enabled: true,
        objective: '连续 30 天优化内容选题与转化。',
        stepPrompt: '复盘上一轮并输出下一轮最小行动。', intervalMinutes: 720,
        totalRounds: 30, completedRounds: 0, nextRunAt: iso(43200000),
        createdAt: iso(-604800000), updatedAt: iso(-1800000), lastRunAt: null, lastResult: null,
      }];
      const taskRows = [
        {
          id: 'weekly-content-report', goal: '本周内容策略报告', intent: 'report', taskType: 'report',
          status: 'succeeded', createdAt: iso(-172800000), updatedAt: iso(-3600000),
        },
        {
          id: 'failed-crawl', goal: '失败的外部采集任务', intent: 'crawler', taskType: 'crawler',
          status: 'failed', lastError: '安全模式阻断：safety.run_real_crawler=false',
          createdAt: iso(-86400000), updatedAt: iso(-3500000),
        },
      ];
      const draftRows = [{
        id: 'draft-1', status: 'draft', goal: '待确认的批量发布草稿',
        metadata: { title: '待确认的批量发布草稿', prompt: '仅生成发布计划，不执行真实发布。', triggerKind: 'once' },
        createdAt: iso(-4000000), updatedAt: iso(-3000000),
      }];
      const generationRows = [{
        id: 'image-complete', kind: 'image', status: 'succeeded', prompt: '安全模式示例图片任务',
        createdAt: iso(-7200000), updatedAt: iso(-7100000),
      }];
      const socialStatus = {
        success: true,
        config: socialConfig,
        roots: {
          repoRoot: '/tmp/e2e', mediaCrawlerRoot: '/tmp/e2e/mediacrawler',
          socialConnectionRoot: '/tmp/e2e/social', socialCookiesDir: '/tmp/e2e/cookies',
          mediaCrawlerBrowserDataDir: '/tmp/e2e/browser-data',
        },
        mediaCrawler: {
          rootExists: true, browserDataExists: false, apiUrl: 'http://127.0.0.1:9',
          apiHealthy: false, apiError: '安全模式：未连接真实采集服务',
        },
        socialConnection: {
          rootExists: true, runtimeMode: 'rust-cdp', browserAvailable: true,
          browserExecutable: '隔离 Chromium（E2E）', cookiesDirExists: true,
          accounts: [], discoveredAccounts: [],
        },
      };

      const responseFor = (channel, payload) => {
        switch (channel) {
          case 'spaces:list': return { activeSpaceId: 'default', spaces: [{ id: 'default', name: '验收空间' }] };
          case 'app:get-version': return '0.1.0-e2e';
          case 'auth:get-state': return { status: 'authenticated', user: { id: 'e2e-user' } };
          case 'redbox-auth:bootstrap':
          case 'redbox-auth:refresh': return { success: true, status: 'authenticated' };
          case 'app:startup-migration-status':
          case 'startup-migration:get-status': return { status: 'completed', shouldShowModal: false, progress: 100 };
          case 'db:get-settings': return settings;
          case 'db:save-settings': return { success: true };
          case 'chat:list-context-sessions': return [{
            id: 'redclaw-e2e-session', title: '商媒运营助手 · 验收空间', updatedAt: iso(), messageCount: 0,
            contextId: 'redclaw:default', contextType: 'redclaw',
          }];
          case 'chat:create-context-session': return {
            id: 'redclaw-e2e-session', title: '商媒运营助手 · 验收空间', updatedAt: iso(), createdAt: iso(),
          };
          case 'chat:get-messages': return [];
          case 'chat:get-runtime-state': return { success: true, isProcessing: false, partialResponse: '', updatedAt: now };
          case 'chat:get-context-usage': return {
            success: true, estimatedTotalTokens: 0, estimatedEffectiveTokens: 0,
            compactThreshold: 100000, compactRatio: 0, compactRounds: 0, compactUpdatedAt: null,
          };
          case 'chat:get-sessions': return [];
          case 'chatrooms:list': return [];
          case 'skills:list': return [];
          case 'redclaw:runner-status': return runnerStatus;
          case 'redclaw:runner-list-scheduled': return { success: true, tasks: scheduledTasks };
          case 'redclaw:runner-list-long-cycle': return { success: true, tasks: longCycleTasks };
          case 'redclaw:task-list': return { success: true, items: draftRows };
          case 'generation:list-job-summaries': return { success: true, items: generationRows };
          case 'generation:list-jobs': return { success: true, items: [] };
          case 'generation:get-runtime-status': return {
            success: true, runtimeReady: true, runtimeRunning: false,
            realImageEnabled: false, realVideoEnabled: false,
          };
          case 'tasks:list': return { success: true, items: taskRows };
          case 'media:list': return { success: true, assets: [] };
          case 'manuscripts:list': return [];
          case 'boss-sync:report-work-event': return { success: true, event_date: payload?.event_date || '2026-07-16' };
          case 'boss-sync:list-week-reviews': return {
            success: true,
            reviews: [{
              id: 'review-1', week_start: '2026-07-13', week_end: '2026-07-19',
              status: 'reviewed', note: '老板反馈：本周交付清晰，下周优先验证抖音前三秒留存。',
            }],
          };
          case 'boss-sync:connection-info': return { success: true, configured: true, employee_id: 'e2e-employee' };
          case 'social-tools:get-status': return socialStatus;
          case 'social-tools:save-config': return { success: true };
          case 'social-tools:start-mediacrawler-login': return {
            success: false, running: false, status: 'blocked', platform: payload?.platform || 'xhs',
            message: '安全模式已阻断真实爬取/登录：safety.run_real_crawler=false',
            error: 'safety_disabled', logs: ['未启动浏览器', '未访问任何外部平台'],
          };
          case 'social-tools:get-mediacrawler-status': return {
            success: false, running: false, status: 'blocked',
            message: '安全模式已阻断真实爬取：safety.run_real_crawler=false', logs: [],
          };
          case 'plugin:browser-extension-status': return {
            success: true, bundled: true, exported: false, bundledPath: '/tmp/e2e/browser-extension', exportPath: '', pluginPath: '',
          };
          case 'youtube:check-ytdlp': return { success: true, installed: true, version: 'e2e', path: '/tmp/e2e/yt-dlp' };
          case 'cli-runtime:detect': return { success: true, tools: [] };
          case 'cli-runtime:list-tools':
          case 'cli-runtime:list-environments': return [];
          case 'mcp:list': return { success: true, servers: [] };
          case 'mcp:sessions': return { success: true, sessions: [] };
          case 'logs:get-status': return {
            enabled: true, logDirectory: '/tmp/e2e/logs', reportDirectory: '/tmp/e2e/reports',
            retentionDays: 7, maxFileMb: 10, recentPreviewLimit: 200,
            uploadConfigured: false, pendingCount: 0, debugVerboseEnabled: false, previousUncleanShutdown: false,
          };
          case 'logs:get-recent': return { lines: [] };
          case 'logs:list-pending-reports': return [];
          case 'knowledge:get-file-index-dashboard': return {
            overall: { status: 'idle', indexedFiles: 0, totalFiles: 0, failedFiles: 0, lastIndexedAt: null },
            lanes: [], scopes: [],
          };
          case 'redclaw:profile:get-bundle': return {
            success: true, userProfile: '# UserProfile\nE2E 验收员工', creatorProfile: '# CreatorProfile\n安全模式运营验收',
          };
          case 'redclaw:profile:onboarding-status': return { success: true, completed: true };
          case 'notifications:permission-state': return { state: 'denied', supported: false };
          case 'wander:list-history':
          case 'wander:get-random':
          case 'knowledge:list':
          case 'knowledge:list-youtube':
          case 'knowledge:docs:list':
          case 'advisors:list':
          case 'ai:roles:list': return [];
          case 'cover:list': return { success: true, assets: [] };
          case 'cover:templates:list': return { success: true, templates: [] };
          case 'subjects:categories:list': return { success: true, categories: [] };
          case 'subjects:list': return { success: true, subjects: [] };
          case 'app:check-update': return { success: true, hasUpdate: false };
          case 'audio:get-capture-capability': return { success: true, available: false, activeRecording: false, reason: 'e2e' };
          default:
            if (channel.includes(':list') || channel.includes('list-')) return [];
            if (channel.includes(':status') || channel.includes(':get')) return { success: true };
            return { success: true };
        }
      };

      const transport = {
        on,
        off,
        removeAllListeners(channel) { listeners.delete(channel); },
        send(channel, payload) {
          calls.push({ kind: 'send', channel, payload, at: new Date().toISOString() });
          if (channel !== 'chat:send-message') return;
          const text = String(payload?.message || '');
          const sessionId = String(payload?.sessionId || 'redclaw-e2e-session');
          let operation = 'crawler';
          let flag = 'safety.run_real_crawler=false';
          if (/视频/.test(text)) { operation = 'video'; flag = 'safety.run_real_video=false'; }
          else if (/图片|配图|封面/.test(text)) { operation = 'image'; flag = 'safety.run_real_image=false'; }
          const callId = 'safe-block-' + operation + '-' + Date.now();
          setTimeout(() => emit('chat:tool-start', {
            sessionId, callId, name: operation === 'crawler' ? 'start_task' : 'generate_' + operation,
            input: { dry_run: false }, description: 'E2E 安全门检查',
          }), 80);
          setTimeout(() => emit('chat:tool-end', {
            sessionId, callId, name: operation === 'crawler' ? 'start_task' : 'generate_' + operation,
            output: { success: false, content: 'safety_disabled: ' + flag },
          }), 160);
          setTimeout(() => emit('chat:error', {
            sessionId,
            message: '安全模式已阻断真实' + (operation === 'image' ? '图片生成' : operation === 'video' ? '视频生成' : '爬取'),
            hint: flag + '；未调用付费服务、未访问外部平台。',
            code: 'safety_disabled', raw: 'blocked=true reason=safety_disabled ' + flag,
          }), 240);
        },
        async invoke(channel, payload) {
          calls.push({ kind: 'invoke', channel, payload, at: new Date().toISOString() });
          return responseFor(channel, payload);
        },
      };
      window.__YUNYING_E2E__ = { calls, emit, listeners };
      window.__RED_ELECTRON_IPC__ = transport;
      Object.defineProperty(navigator, 'clipboard', {
        configurable: true,
        value: { readText: async () => '', writeText: async () => undefined },
      });
    })();
  `;
}

async function main() {
  const viteBin = resolve(desktopRoot, 'node_modules', '.bin', process.platform === 'win32' ? 'vite.cmd' : 'vite');
  const vite = spawn(viteBin, ['--host', '127.0.0.1', '--port', String(vitePort), '--strictPort'], {
    cwd: desktopRoot,
    env: { ...process.env, BROWSER: 'none' },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const viteLog = attachLogs(vite, 'ui-vite.log');
  await waitForHttp(baseUrl);

  const chrome = spawn(findChrome(), [
    '--headless=new',
    '--disable-gpu',
    '--no-first-run',
    '--no-default-browser-check',
    '--disable-background-networking',
    '--disable-component-update',
    '--disable-sync',
    '--metrics-recording-only',
    '--password-store=basic',
    '--use-mock-keychain',
    `--remote-debugging-port=${cdpPort}`,
    `--user-data-dir=${resolve(tmpDir, `chrome-profile-${Date.now()}`)}`,
    '--window-size=1440,1000',
    'about:blank',
  ], { stdio: ['ignore', 'pipe', 'pipe'] });
  const chromeLog = attachLogs(chrome, 'ui-chrome.log');

  let cdp;
  try {
    await waitForHttp(`http://127.0.0.1:${cdpPort}/json/version`);
    const targets = await (await fetch(`http://127.0.0.1:${cdpPort}/json/list`)).json();
    const pageTarget = targets.find((target) => target.type === 'page');
    if (!pageTarget?.webSocketDebuggerUrl) throw new Error('未找到 Chromium page target');
    cdp = new CdpClient(pageTarget.webSocketDebuggerUrl);
    await cdp.ready();
    await Promise.all([
      cdp.send('Page.enable'),
      cdp.send('Runtime.enable'),
      cdp.send('Log.enable'),
      cdp.send('Network.enable'),
      cdp.send('Emulation.setDeviceMetricsOverride', {
        width: 1440, height: 1000, deviceScaleFactor: 1, mobile: false,
      }),
    ]);
    cdp.on('Runtime.consoleAPICalled', ({ type, args }) => {
      if (type !== 'error') return;
      results.browserConsoleErrors.push(args.map((arg) => arg.value || arg.description || '').join(' '));
    });
    cdp.on('Runtime.exceptionThrown', ({ exceptionDetails }) => {
      results.browserConsoleErrors.push(exceptionDetails?.exception?.description || exceptionDetails?.text || 'Runtime exception');
    });
    cdp.on('Log.entryAdded', ({ entry }) => {
      if (entry?.level !== 'error') return;
      results.browserConsoleErrors.push(entry.text || `Browser log error from ${entry.source || 'unknown'}`);
    });
    const requestUrls = new Map();
    cdp.on('Network.requestWillBeSent', ({ requestId, request }) => {
      if (requestId && request?.url) requestUrls.set(requestId, request.url);
    });
    cdp.on('Network.loadingFailed', ({ requestId, errorText, canceled, type }) => {
      const failure = {
        url: requestUrls.get(requestId) || '',
        errorText: errorText || 'unknown network failure',
        canceled: Boolean(canceled),
        type: type || '',
      };
      requestUrls.delete(requestId);
      if (
        (failure.canceled && failure.errorText === 'net::ERR_ABORTED')
        || (failure.url.endsWith('/favicon.ico') && failure.errorText.includes('ERR_FILE_NOT_FOUND'))
      ) {
        results.ignoredNetworkFailures.push({ ...failure, reason: '页面切换取消请求或缺省 favicon' });
        return;
      }
      results.networkFailures.push(failure);
    });
    await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: mockTransportSource() });

    const evaluate = async (expression, awaitPromise = true) => {
      const response = await cdp.send('Runtime.evaluate', {
        expression,
        awaitPromise,
        returnByValue: true,
        userGesture: true,
      });
      if (response.exceptionDetails) {
        throw new Error(response.exceptionDetails.exception?.description || response.exceptionDetails.text);
      }
      return response.result?.value;
    };
    const sleep = (ms) => new Promise((resolveSleep) => setTimeout(resolveSleep, ms));
    const bodyText = () => evaluate('document.body?.innerText || ""');
    const assertNoCrash = async (label) => {
      const text = await bodyText();
      if (text.includes('Something went wrong.')) throw new Error(`${label}: React ErrorBoundary 已触发`);
      results.assertions.push({ label, ok: true });
    };
    const auditView = async (label) => {
      const audit = await evaluate(`(() => {
        const isVisible = (node) => {
          const style = getComputedStyle(node);
          const rect = node.getBoundingClientRect();
          return style.display !== 'none' && style.visibility !== 'hidden'
            && Number(style.opacity || 1) !== 0 && rect.width > 0 && rect.height > 0;
        };
        const accessibleName = (node) => String(
          node.getAttribute('aria-label')
          || node.getAttribute('title')
          || node.innerText
          || node.textContent
          || ''
        ).trim();
        const unlabeledButtons = [...document.querySelectorAll('button, [role="button"]')]
          .filter(isVisible)
          .filter((node) => !accessibleName(node))
          .map((node) => node.outerHTML.slice(0, 240));
        const unlabeledFields = [...document.querySelectorAll('input, select, textarea')]
          .filter(isVisible)
          .filter((node) => {
            if (node.closest('label')) return false;
            if (node.id && document.querySelector('label[for="' + CSS.escape(node.id) + '"]')) return false;
            return !String(node.getAttribute('aria-label') || node.getAttribute('title') || '').trim();
          })
          .map((node) => node.outerHTML.slice(0, 240));
        const horizontalOverflowPx = Math.max(
          0,
          document.documentElement.scrollWidth - document.documentElement.clientWidth,
          document.body ? document.body.scrollWidth - document.body.clientWidth : 0,
        );
        const parseColor = (value) => {
          const parts = String(value || '').match(/[\\d.]+/g)?.map(Number) || [];
          if (parts.length < 3) return null;
          return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 };
        };
        const backgroundFor = (node) => {
          let current = node;
          while (current) {
            const color = parseColor(getComputedStyle(current).backgroundColor);
            if (color && color.a >= 0.95) return color;
            current = current.parentElement;
          }
          return { r: 255, g: 255, b: 255, a: 1 };
        };
        const luminance = ({ r, g, b }) => {
          const channel = (value) => {
            const normalized = value / 255;
            return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
          };
          return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
        };
        const contrastChecks = [...document.querySelectorAll('[data-e2e-contrast]')]
          .filter(isVisible)
          .map((node) => {
            const foreground = parseColor(getComputedStyle(node).color);
            const background = backgroundFor(node);
            if (!foreground) return { key: node.getAttribute('data-e2e-contrast'), ratio: 0 };
            const first = luminance(foreground);
            const second = luminance(background);
            const ratio = (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
            return { key: node.getAttribute('data-e2e-contrast'), ratio: Number(ratio.toFixed(2)) };
          });
        return {
          viewport: { width: innerWidth, height: innerHeight },
          documentWidth: document.documentElement.scrollWidth,
          horizontalOverflowPx,
          unlabeledButtons,
          unlabeledFields,
          contrastChecks,
          contrastFailures: contrastChecks.filter((item) => item.ratio < 4.5),
          errorBoundary: (document.body?.innerText || '').includes('Something went wrong.'),
        };
      })()`);
      results.viewAudits.push({ label, ...audit });
      if (audit.errorBoundary) throw new Error(`${label}: React ErrorBoundary 已触发`);
      if (audit.horizontalOverflowPx > 1) {
        throw new Error(`${label}: documentElement 横向溢出 ${audit.horizontalOverflowPx}px`);
      }
      if (audit.unlabeledButtons.length > 0) {
        throw new Error(`${label}: 存在无 accessible name 的可见按钮：${audit.unlabeledButtons.join(' | ')}`);
      }
      if (audit.unlabeledFields.length > 0) {
        throw new Error(`${label}: 存在无 label/aria-label/title 的可见表单控件：${audit.unlabeledFields.join(' | ')}`);
      }
      if (audit.contrastFailures.length > 0) {
        throw new Error(`${label}: 关键文字对比度不足 4.5:1：${audit.contrastFailures.map((item) => `${item.key}=${item.ratio}`).join(' | ')}`);
      }
      results.assertions.push({ label: `视图审计：${label}`, ok: true });
    };
    const waitForText = async (text, timeoutMs = 12_000) => {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        if ((await bodyText()).includes(text)) {
          results.assertions.push({ label: `页面包含：${text}`, ok: true });
          return;
        }
        await sleep(120);
      }
      throw new Error(`等待页面文本超时：${text}`);
    };
    const waitForTextareaValue = async (prefix, timeoutMs = 12_000) => {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        const matched = await evaluate(`[...document.querySelectorAll('textarea')].some((node) => String(node.value || '').startsWith(${JSON.stringify(prefix)}))`);
        if (matched) {
          results.assertions.push({ label: `输入框预填：${prefix}`, ok: true });
          return;
        }
        await sleep(120);
      }
      throw new Error(`等待输入框预填超时：${prefix}`);
    };
    const visibleTextCenter = async (text, exact = true) => {
      const serialized = JSON.stringify(text);
      const expression = `(() => {
        const wanted = ${serialized};
        const nodes = [...document.querySelectorAll('button, [role="button"], a')];
        const matches = nodes.filter((node) => {
          const value = String(node.innerText || node.getAttribute('aria-label') || node.getAttribute('title') || '').trim();
          const style = getComputedStyle(node);
          const rect = node.getBoundingClientRect();
          return (${exact ? 'value === wanted' : 'value.includes(wanted)'})
            && style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0;
        }).sort((left, right) => {
          const leftText = String(left.innerText || '').trim() === wanted ? 1 : 0;
          const rightText = String(right.innerText || '').trim() === wanted ? 1 : 0;
          return rightText - leftText;
        });
        const node = matches[0];
        if (!node) return null;
        node.scrollIntoView({ block: 'center', inline: 'center' });
        const rect = node.getBoundingClientRect();
        return { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2, label: String(node.innerText || node.getAttribute('aria-label') || '').trim() };
      })()`;
      return evaluate(expression);
    };
    const clickAt = async (point, label) => {
      if (!point) throw new Error(`找不到可点击控件：${label}`);
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: point.x, y: point.y });
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: point.x, y: point.y, button: 'left', clickCount: 1 });
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: point.x, y: point.y, button: 'left', clickCount: 1 });
      results.clicks.push({ label, x: point.x, y: point.y, at: new Date().toISOString() });
      await sleep(180);
    };
    const clickText = async (text, exact = true) => {
      let point = await visibleTextCenter(text, exact);
      if (point) {
        await sleep(80);
        point = await visibleTextCenter(text, exact);
      }
      await clickAt(point, text);
    };
    const clickSelector = async (selector, label) => {
      const expression = `(() => {
        const node = document.querySelector(${JSON.stringify(selector)});
        if (!node) return null;
        node.scrollIntoView({ block: 'center', inline: 'center' });
        const rect = node.getBoundingClientRect();
        return rect.width && rect.height ? { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 } : null;
      })()`;
      let point = await evaluate(expression);
      if (point) {
        await sleep(80);
        point = await evaluate(expression);
      }
      await clickAt(point, label);
    };
    const fillTextarea = async (text) => {
      const changed = await evaluate(`(() => {
        const textareas = [...document.querySelectorAll('textarea')];
        const node = textareas.find((item) => {
          const rect = item.getBoundingClientRect();
          const placeholder = String(item.getAttribute('placeholder') || '');
          return rect.width > 0 && rect.height > 0 && !item.readOnly
            && (placeholder.includes('继续补充') || placeholder.includes('问我任何问题') || placeholder.includes('发送消息'));
        }) || textareas.find((item) => {
          const rect = item.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0 && !item.readOnly;
        });
        if (!node) return false;
        const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set;
        setter.call(node, ${JSON.stringify(text)});
        node.dispatchEvent(new Event('input', { bubbles: true }));
        node.dispatchEvent(new Event('change', { bubbles: true }));
        node.focus();
        return true;
      })()`);
      if (!changed) throw new Error('找不到可填写的聊天输入框');
      results.assertions.push({ label: `已填写输入框：${text}`, ok: true });
      await sleep(120);
    };
    const screenshot = async (fileName) => {
      const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true, captureBeyondViewport: false });
      writeFileSync(resolve(screenshotsDir, fileName), Buffer.from(data, 'base64'));
      results.screenshots.push(fileName);
    };
    const assertTaskDefinitionHasNoFakeCompletion = async (title) => {
      const cardText = await evaluate(`(() => {
        const title = ${JSON.stringify(title)};
        const card = [...document.querySelectorAll('button')].find((node) => String(node.innerText || '').includes(title));
        return card ? String(card.innerText || '') : '';
      })()`);
      if (!cardText) throw new Error(`找不到任务定义卡片：${title}`);
      if (cardText.includes('已完成') || cardText.includes('执行 已成功')) {
        throw new Error(`${title}: 未执行的任务定义被伪装成已完成`);
      }
      results.assertions.push({ label: `${title} 保持未执行定义状态`, ok: true });
    };
    const reloadHome = async () => {
      await cdp.send('Page.navigate', { url: baseUrl });
      await waitForText('商媒运营助手工作台');
      await assertNoCrash('运营中枢加载');
      await auditView('运营中枢：首页/聊天');
    };

    await reloadHome();
    await screenshot('00-home-chat.png');

    await clickText('图片创作');
    await waitForTextareaValue('我要做图片创作');
    await fillTextarea('生成一张 3:4 的猫爬架投流图，真实调用应由安全门拦截。');
    await clickSelector('form button[type="submit"]:not([disabled])', '发送图片生成请求');
    await waitForText('safety.run_real_image=false');
    await auditView('图片生成阻断态');
    await screenshot('01-image-generation-blocked.png');

    await reloadHome();
    await clickText('视频创作');
    await waitForTextareaValue('我要做视频创作');
    await fillTextarea('生成一条 9:16 的猫爬架投流短视频，真实调用应由安全门拦截。');
    await clickSelector('form button[type="submit"]:not([disabled])', '发送视频生成请求');
    await waitForText('safety.run_real_video=false');
    await auditView('视频生成阻断态');
    await screenshot('02-video-generation-blocked.png');

    await clickText('任务队列');
    await waitForText('商媒运营助手任务中心');
    await waitForText('每周运营复盘');
    await waitForText('30 天增长冲刺');
    await assertTaskDefinitionHasNoFakeCompletion('每周运营复盘');
    await assertTaskDefinitionHasNoFakeCompletion('30 天增长冲刺');
    await assertNoCrash('任务队列加载');
    await auditView('任务队列：定时与长周期');
    await screenshot('03-workboard-scheduled-long-cycle.png');

    await clickText('本周内容策略报告', false);
    await waitForText('工作摘要（提交前可编辑）');
    await screenshot('04-workboard-completed-report-form.png');
    await clickText('上报老板');
    await waitForText('已上报老板端');
    await clickText('查看老板反馈');
    await waitForText('老板反馈：本周交付清晰');
    await auditView('任务队列：老板上报与反馈');
    await screenshot('05-workboard-boss-report-review.png');

    await clickText('待确认的批量发布草稿', false);
    await waitForText('草稿、失败、取消和未执行任务不能上报');
    await screenshot('06-workboard-draft-not-reportable.png');
    await clickText('失败的外部采集任务', false);
    await waitForText('当前任务没有成功完成的执行记录');
    await screenshot('07-workboard-failed-not-reportable.png');

    await clickText('设置');
    await waitForText('常规设置');
    await assertNoCrash('设置首页加载');
    await auditView('设置：常规设置');
    await screenshot('08-settings-general.png');
    await clickText('AI 模型');
    await waitForText('AI 模型设置');
    await assertNoCrash('AI 模型设置加载');
    await auditView('设置：AI 模型');
    await clickText('用户档案');
    await assertNoCrash('用户档案设置加载');
    await auditView('设置：用户档案');
    await clickText('实验功能');
    await assertNoCrash('实验功能设置加载');
    await auditView('设置：实验功能');
    await clickText('工具管理');
    await waitForText('外部工具管理');
    await waitForText('MediaCrawler 采集配置');
    await assertNoCrash('工具设置加载');
    await auditView('设置：工具管理');
    await screenshot('09-settings-tools-crawler.png');
    await clickText('二维码登录');
    await waitForText('safety.run_real_crawler=false');
    await auditView('爬取阻断态');
    await screenshot('10-crawler-blocked.png');

    const navSmoke = [
      ['运营中枢', '11-nav-operations.png'],
      ['资料库', '12-nav-knowledge.png'],
      ['爆品调研', '13-nav-research.png'],
      ['内容稿件', '14-nav-manuscripts.png'],
      ['博主库', '15-nav-subjects.png'],
      ['团队成果', '16-nav-team.png'],
      ['社媒账号', '17-nav-social-accounts.png'],
      ['封面', '18-nav-cover.png'],
    ];
    for (const [label, fileName] of navSmoke) {
      await clickText(label);
      await sleep(650);
      await assertNoCrash(`主导航：${label}`);
      await auditView(`主导航：${label}`);
      await screenshot(fileName);
    }

    results.ipcCalls = await evaluate('window.__YUNYING_E2E__?.calls || []');
    await sleep(500);
    if (results.browserConsoleErrors.length > 0) {
      throw new Error(`浏览器 console/runtime error 非空：${results.browserConsoleErrors.join(' | ')}`);
    }
    if (results.networkFailures.length > 0) {
      throw new Error(`浏览器请求失败非空：${results.networkFailures.map((failure) => `${failure.url} ${failure.errorText}`).join(' | ')}`);
    }
    results.completedAt = new Date().toISOString();
    results.success = true;
    writeFileSync(resolve(logsDir, 'ui-browser-results.json'), `${JSON.stringify(results, null, 2)}\n`);
  } finally {
    cdp?.close();
    chrome.kill('SIGTERM');
    vite.kill('SIGTERM');
    viteLog.end();
    chromeLog.end();
  }
}

main().catch((error) => {
  results.completedAt = new Date().toISOString();
  results.success = false;
  results.error = error instanceof Error ? error.stack || error.message : String(error);
  writeFileSync(resolve(logsDir, 'ui-browser-results.json'), `${JSON.stringify(results, null, 2)}\n`);
  console.error(error);
  process.exitCode = 1;
});
