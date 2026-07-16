import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const desktopDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const tauriDir = join(desktopDir, 'src-tauri');
const artifactDir = join(desktopDir, 'e2e-artifacts', 'cross-app-sync');
const reportPath = join(artifactDir, 'report.json');
const tempDir = await mkdtemp(join(tmpdir(), 'boss-cross-app-sync-'));
const databasePath = join(tempDir, 'boss.sqlite');
const apiPort = await freePort();
const apiUrl = `http://127.0.0.1:${apiPort}`;
const children = [];
const steps = [];

const fixture = {
  ownerCode: 'BOSS-7429',
  employeeId: 'e2e-cross-app-employee',
  employeeName: '跨端验收员工',
  eventId: 'e2e-cross-app-event-2026-07-16',
  dateFrom: '2026-07-13',
  dateTo: '2026-07-19',
  weekStart: '2026-07-13',
  weekEnd: '2026-07-19'
};

await rm(artifactDir, { recursive: true, force: true });
await mkdir(artifactDir, { recursive: true });

let health = null;
let firstReport = null;
let idempotentUpdate = null;
let bossWorkReport = null;
let savedReview = null;
let employeeReviews = null;

try {
  const server = startProcess(
    commandName('cargo'),
    ['run', '--quiet', '--bin', 'boss-server'],
    tauriDir,
    {
      BOSS_ACCOUNTING_DB: databasePath,
      BOSS_SERVER_ADDR: `127.0.0.1:${apiPort}`
    }
  );
  children.push(server);

  await step('启动隔离 Rust boss-server', async () => {
    health = await waitForHealth(`${apiUrl}/health`, server);
    assert.equal(health.ok, true);
    assert.equal(health.backend, 'rust');
    assert.equal(health.service, 'boss-server');
  });

  const initialEvent = {
    owner_code: fixture.ownerCode,
    event_id: fixture.eventId,
    employee_id: fixture.employeeId,
    employee_name: fixture.employeeName,
    role: '内容运营',
    employee_status: 'active',
    event_date: '2026-07-16',
    task_type: 'weekly_delivery',
    material_count: 5,
    cost_cents: 1680,
    quality_score: 88,
    summary: '完成首轮跨端工作上报'
  };

  await step('员工端通过 HTTP 上报工作事件', async () => {
    firstReport = await callTool('employee.report_work_event', initialEvent);
    assert.equal(firstReport.event_id, fixture.eventId);
    assert.equal(firstReport.employee_id, fixture.employeeId);
    assert.equal(firstReport.material_count, 5);
  });

  await step('相同 event_id 执行幂等更新', async () => {
    idempotentUpdate = await callTool('employee.report_work_event', {
      ...initialEvent,
      material_count: 9,
      cost_cents: 2460,
      quality_score: 94,
      summary: '相同 event_id 已更新为最终周交付'
    });
    assert.equal(idempotentUpdate.event_id, fixture.eventId);
    assert.equal(idempotentUpdate.material_count, 9);
    assert.equal(idempotentUpdate.summary, '相同 event_id 已更新为最终周交付');
  });

  await step('老板端读取员工工作报告并验证无重复事件', async () => {
    bossWorkReport = await callTool('boss.employee_work_report', {
      owner_code: fixture.ownerCode,
      employee_id: fixture.employeeId,
      date_from: fixture.dateFrom,
      date_to: fixture.dateTo
    });
    assert.equal(bossWorkReport.event_count, 1);
    assert.equal(bossWorkReport.events.length, 1);
    assert.equal(bossWorkReport.material_count, 9);
    assert.equal(bossWorkReport.cost_cents, 2460);
    assert.equal(bossWorkReport.average_quality, 94);
    assert.equal(bossWorkReport.events[0].id, fixture.eventId);
    assert.equal(bossWorkReport.events[0].summary, '相同 event_id 已更新为最终周交付');
  });

  await step('老板端写入本周审阅', async () => {
    savedReview = await callTool('boss.save_week_review', {
      owner_code: fixture.ownerCode,
      employee_id: fixture.employeeId,
      week_start: fixture.weekStart,
      week_end: fixture.weekEnd,
      status: 'needs_supplement',
      note: '请补充最终交付截图'
    });
    assert.equal(savedReview.employee_id, fixture.employeeId);
    assert.equal(savedReview.status, 'needs_supplement');
    assert.equal(savedReview.note, '请补充最终交付截图');
  });

  await step('员工端通过 HTTP 读回老板周审阅', async () => {
    employeeReviews = await callTool('employee.list_week_reviews', {
      owner_code: fixture.ownerCode,
      employee_id: fixture.employeeId,
      week_start: fixture.weekStart,
      week_end: fixture.weekEnd
    });
    assert.equal(employeeReviews.reviews.length, 1);
    assert.equal(employeeReviews.reviews[0].status, 'needs_supplement');
    assert.equal(employeeReviews.reviews[0].note, '请补充最终交付截图');
  });

  await writeReport('passed', null);
  console.log(`Cross-app sync E2E passed. Evidence: ${reportPath}`);
} catch (error) {
  await writeReport('failed', error);
  throw error;
} finally {
  for (const child of children.reverse()) stopProcess(child);
  await rm(tempDir, { recursive: true, force: true }).catch(() => {});
}

async function callTool(name, argumentsValue) {
  const response = await fetch(`${apiUrl}/api/boss/tool`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ name, arguments: argumentsValue }),
    signal: AbortSignal.timeout(12_000)
  });
  const payload = await response.json();
  assert.equal(response.ok, true, `${name} HTTP ${response.status}: ${JSON.stringify(payload)}`);
  assert.equal(payload.ok, true, `${name} 返回失败：${JSON.stringify(payload)}`);
  return payload.result;
}

async function step(name, callback) {
  const startedAt = Date.now();
  try {
    await callback();
    steps.push({ name, status: 'passed', durationMs: Date.now() - startedAt });
  } catch (error) {
    steps.push({
      name,
      status: 'failed',
      durationMs: Date.now() - startedAt,
      error: String(error.message || error)
    });
    throw error;
  }
}

async function writeReport(status, error) {
  const event = bossWorkReport?.events?.[0] ?? null;
  const review = employeeReviews?.reviews?.[0] ?? null;
  const report = {
    status,
    generatedAt: new Date().toISOString(),
    transport: {
      protocol: 'HTTP',
      endpoint: '/api/boss/tool',
      loopback: true,
      isolatedTemporaryDatabase: true,
      backend: health?.backend ?? null
    },
    fixture: {
      ownerCode: fixture.ownerCode,
      employeeId: fixture.employeeId,
      eventId: fixture.eventId,
      weekStart: fixture.weekStart,
      weekEnd: fixture.weekEnd
    },
    tools: [
      'employee.report_work_event',
      'boss.employee_work_report',
      'boss.save_week_review',
      'employee.list_week_reviews'
    ],
    assertions: {
      eventIdempotency: {
        passed: bossWorkReport?.event_count === 1
          && bossWorkReport?.events?.length === 1
          && event?.id === fixture.eventId
          && event?.material_count === 9,
        eventCount: bossWorkReport?.event_count ?? null,
        finalMaterialCount: event?.material_count ?? null,
        finalCostCents: event?.cost_cents ?? null,
        finalQualityScore: event?.quality_score ?? null,
        finalSummary: event?.summary ?? null
      },
      reviewRoundTrip: {
        passed: employeeReviews?.reviews?.length === 1
          && review?.status === 'needs_supplement'
          && review?.note === '请补充最终交付截图',
        reviewCount: employeeReviews?.reviews?.length ?? null,
        status: review?.status ?? null,
        note: review?.note ?? null
      }
    },
    responseReceipts: {
      firstReport: summarizeEvent(firstReport),
      idempotentUpdate: summarizeEvent(idempotentUpdate),
      bossWorkReport: bossWorkReport ? {
        eventCount: bossWorkReport.event_count,
        materialCount: bossWorkReport.material_count,
        costCents: bossWorkReport.cost_cents,
        averageQuality: bossWorkReport.average_quality
      } : null,
      savedReview: summarizeReview(savedReview),
      employeeReview: summarizeReview(review)
    },
    steps,
    error: error ? String(error.stack || error) : null,
    sensitiveValuesRecorded: false
  };
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
}

function summarizeEvent(event) {
  if (!event) return null;
  return {
    eventId: event.event_id,
    employeeId: event.employee_id,
    materialCount: event.material_count,
    costCents: event.cost_cents,
    qualityScore: event.quality_score,
    summary: event.summary
  };
}

function summarizeReview(review) {
  if (!review) return null;
  return {
    employeeId: review.employee_id,
    weekStart: review.week_start,
    weekEnd: review.week_end,
    status: review.status,
    note: review.note
  };
}

function startProcess(command, args, cwd, extraEnv) {
  const child = spawn(command, args, {
    cwd,
    env: { ...process.env, ...extraEnv },
    stdio: ['ignore', 'pipe', 'pipe'],
    detached: process.platform !== 'win32'
  });
  child.output = '';
  child.stdout.on('data', (chunk) => { child.output += chunk.toString(); });
  child.stderr.on('data', (chunk) => { child.output += chunk.toString(); });
  return child;
}

function stopProcess(child) {
  if (!child?.pid || child.exitCode !== null) return;
  if (process.platform === 'win32') {
    spawnSync('taskkill', ['/pid', String(child.pid), '/t', '/f'], { stdio: 'ignore' });
  } else {
    try { process.kill(-child.pid, 'SIGTERM'); } catch { child.kill('SIGTERM'); }
  }
}

async function waitForHealth(url, child, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) {
      throw new Error(`Rust boss-server 提前退出（${child.exitCode}）：${child.output}`);
    }
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(2_000) });
      if (response.ok) return await response.json();
    } catch {
      // Server is still compiling or starting.
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 200));
  }
  throw new Error(`Rust boss-server 启动超时：${child.output}`);
}

function commandName(name) {
  return process.platform === 'win32' ? `${name}.cmd` : name;
}

function freePort() {
  return new Promise((resolvePort, reject) => {
    const server = createServer();
    server.unref();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      const port = typeof address === 'object' && address ? address.port : 0;
      server.close((error) => error ? reject(error) : resolvePort(port));
    });
  });
}
