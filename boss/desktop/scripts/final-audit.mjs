import { chromium } from '@playwright/test';
import { spawn, spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const desktopDir = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const tauriDir = join(desktopDir, 'src-tauri');
const artifactDir = join(desktopDir, 'e2e-artifacts', 'final-audit');
const tempDir = await mkdtemp(join(tmpdir(), 'boss-final-audit-'));
const apiPort = await freePort();
const webPort = await freePort();
const apiUrl = `http://127.0.0.1:${apiPort}`;
const webUrl = `http://127.0.0.1:${webPort}`;
const children = [];
const steps = [];
const consoleErrors = [];
const pageErrors = [];
const requestFailures = [];
const viewAudits = [];
let browser;
let page;
let forceStaleInvoiceResponse = false;

await rm(artifactDir, { recursive: true, force: true });
await mkdir(artifactDir, { recursive: true });

try {
  const cargo = startProcess(commandName('cargo'), ['run', '--quiet', '--bin', 'boss-server'], tauriDir, {
    BOSS_ACCOUNTING_DB: join(tempDir, 'boss.sqlite'),
    BOSS_SERVER_ADDR: `127.0.0.1:${apiPort}`
  });
  children.push(cargo);
  await waitForUrl(`${apiUrl}/health`, cargo, 'Rust boss-server');

  const vite = startProcess(commandName('npm'), ['run', 'dev', '--', '--port', String(webPort)], desktopDir, {
    BOSS_SERVER_URL: apiUrl
  });
  children.push(vite);
  await waitForUrl(webUrl, vite, 'Vite');

  const invoicePath = join(tempDir, 'ui-audit-invoice.txt');
  await writeFile(invoicePath, [
    '发票号码：AUDIT20260716001',
    '开票日期：2026-07-16',
    '销售方：UI 验收供应商',
    '购买方：商媒运营助手',
    '价税合计：268.40',
    '税额：15.19',
    '项目：AI 工具服务费'
  ].join('\n'), 'utf8');

  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1440, height: 1000 },
    locale: 'zh-CN',
    permissions: ['clipboard-read', 'clipboard-write']
  });
  page = await context.newPage();
  await page.route('**/api/boss/tool', async (route) => {
    if (!forceStaleInvoiceResponse) {
      await route.continue();
      return;
    }
    const request = route.request();
    const body = request.postDataJSON();
    if (body?.name !== 'invoice.upload_and_extract') {
      await route.continue();
      return;
    }
    forceStaleInvoiceResponse = false;
    const response = await route.fetch();
    const payload = await response.json();
    if (payload?.result && typeof payload.result === 'object') {
      payload.result.status = 'extracted';
    }
    await route.fulfill({ response, json: payload });
  });
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });
  page.on('pageerror', (error) => pageErrors.push(error.message));
  page.on('requestfailed', (request) => requestFailures.push(`${request.method()} ${request.url()} :: ${request.failure()?.errorText || 'failed'}`));

  await page.goto(webUrl, { waitUntil: 'networkidle' });
  await assertVisible(page.getByRole('heading', { name: '先看关键进展，再做关键决定' }), '登录页标题');
  await step('登录页深浅主题完整切换', async () => {
    const currentTheme = await page.evaluate(() => document.documentElement.dataset.theme);
    if (currentTheme === 'dark') {
      await page.getByRole('button', { name: '切换到浅色模式' }).click();
    }
    await page.getByRole('button', { name: '切换到深色模式' }).click();
    assert(await page.evaluate(() => document.documentElement.dataset.theme) === 'dark', '登录页应切换到深色主题');
    const darkSurfaces = await page.evaluate(() => ['body', '.login-card', '.login-dashboard-preview'].map((selector) => {
      const element = document.querySelector(selector);
      return element ? getComputedStyle(element).backgroundColor : '';
    }));
    assert(darkSurfaces.every(isDarkCssColor), `登录页存在浅色表面：${JSON.stringify(darkSurfaces)}`);
    await screenshot('01a-login-dark.png');
    await page.getByRole('button', { name: '切换到浅色模式' }).click();
    assert(await page.evaluate(() => document.documentElement.dataset.theme) === 'light', '登录页应恢复浅色主题');
  });
  await screenshot('01-login-desktop.png');
  await step('登录错误提示', async () => {
    await page.locator('input[name="password"]').fill('wrong-password');
    await page.getByRole('button', { name: /进入工作台/ }).click();
    await assertText(page.locator('.login-error'), '演示账号或密码不正确');
  });
  await step('演示老板登录并加载 Rust 数据', async () => {
    await page.locator('input[name="password"]').fill('demo-password');
    await page.getByRole('button', { name: /进入工作台/ }).click();
    await assertVisible(page.getByRole('heading', { name: '下午好，徐总' }), '总览标题');
    await waitForWorkspaceReady();
  });
  await auditCurrentView('overview-desktop');
  await step('工作台深色主题覆盖内部表面', async () => {
    await page.getByRole('button', { name: '切换到深色模式' }).click();
    const themeAudit = await page.evaluate(() => ({
      theme: document.documentElement.dataset.theme,
      surfaces: ['body', '.workspace-shell', '.workspace-topbar', '.overview-metric', '.dashboard-section']
        .map((selector) => {
          const element = document.querySelector(selector);
          return element ? getComputedStyle(element).backgroundColor : '';
        }),
    }));
    assert(themeAudit.theme === 'dark', '工作台应切换到深色主题');
    assert(themeAudit.surfaces.every(isDarkCssColor), `工作台存在浅色表面：${JSON.stringify(themeAudit.surfaces)}`);
    await screenshot('02a-overview-dark.png');
    await page.getByRole('button', { name: '切换到浅色模式' }).click();
  });
  await screenshot('02-overview-desktop.png');

  await step('总览刷新、复制老板码和员工快捷入口', async () => {
    await page.getByRole('button', { name: '刷新数据' }).click();
    await waitForWorkspaceReady();
    await page.getByRole('button', { name: '复制老板码' }).click();
    await assertVisible(page.getByRole('status'), '复制结果提示');
    await page.getByRole('button', { name: '待办通知' }).click();
    await assertVisible(page.getByRole('heading', { name: '下午好，徐总' }), '待办通知回到总览');
    await page.locator('[data-open-action]').first().click();
    assert((await page.locator('h1').innerText()) !== '下午好，徐总', '待办详情应打开目标页面');
    await navigate('今日总览', '下午好，徐总');
    const messageCount = await page.locator('.message-row').count();
    await page.locator('[data-overview-ai]').first().click();
    await assertVisible(page.getByRole('heading', { name: '老板 AI', exact: true }), '总览管理摘要入口');
    await waitForMessageCount(messageCount + 2);
    await navigate('今日总览', '下午好，徐总');
    await page.locator('[data-open-employee]').first().click();
    await assertVisible(page.getByRole('heading', { name: '员工周报' }), '员工周报标题');
  });

  await step('员工筛选、搜索和周切换', async () => {
    await page.locator('[data-employee-filter="attention"]').click();
    await assertVisible(page.locator('.employee-button').first(), '需关注员工');
    await page.locator('[data-employee-filter="all"]').click();
    await page.locator('#employee-search').fill('周祺');
    assert((await page.locator('.employee-button').count()) === 1, '搜索应只返回周祺');
    await page.locator('#employee-search').fill('');
    await page.getByRole('button', { name: '上一周' }).click();
    await assertVisible(page.getByText('上周', { exact: true }), '上周标签');
    await page.getByRole('button', { name: '下一周' }).click();
    await assertVisible(page.getByText('本周', { exact: true }), '本周标签');
  });

  await step('员工周报补充要求校验与持久化', async () => {
    const note = page.locator('[data-review-form] textarea');
    await note.fill('');
    await page.locator('[data-review-status="needs_supplement"]').click();
    await assertToast('记录补充要求前请先填写具体内容');
    assert(!(await page.locator('.review-pill').innerText()).includes('已记录补充要求'), '空补充要求不得改变审阅状态');
    await note.fill('请补充转化截图与数据来源。');
    await page.locator('[data-review-status="needs_supplement"]').click();
    await page.getByText('已记录补充要求', { exact: true }).waitFor();
    await assertToast('补充要求已保存到老板端');
  });
  await auditCurrentView('employees-desktop');
  await screenshot('03-employee-review-desktop.png');

  await step('员工审阅刷新后仍存在', async () => {
    await page.getByRole('button', { name: '刷新数据' }).click();
    await waitForWorkspaceReady();
    await page.getByText('已记录补充要求', { exact: true }).waitFor();
    await assertText(page.locator('[data-review-form] textarea'), '请补充转化截图与数据来源。');
    await page.locator('[data-review-status="reviewed"]').click();
    await page.getByText('老板已阅', { exact: true }).waitFor();
    await page.locator('[data-employee-filter="reviewed"]').click();
    assert((await page.locator('.employee-button').count()) >= 1, '已阅筛选应显示刚审阅的员工');
    await page.locator('[data-employee-filter="all"]').click();
  });

  await navigate('财务记账', '财务记账');
  await step('账本设置与 Actual 状态', async () => {
    await page.getByRole('button', { name: /账本设置/ }).click();
    await assertVisible(page.locator('.integration-panel.is-open'), 'Actual 设置面板');
    const actualRows = page.locator('.actual-status-item');
    assert((await actualRows.count()) === 6, 'Actual 设置应有 6 个状态项');
    for (let index = 0; index < await actualRows.count(); index += 1) {
      assert((await actualRows.nth(index).locator('strong').count()) === 1, 'Actual 状态值不应重复渲染');
    }
    await page.locator('.integration-toggle').click();
    assert(!(await page.locator('.integration-panel').evaluate((element) => element.classList.contains('is-open'))), 'Actual 设置应可收起');
    await page.getByRole('button', { name: /账本设置/ }).click();
    await page.getByRole('button', { name: /检查连接/ }).click();
    await page.getByRole('button', { name: /检查连接/ }).waitFor();
    await page.getByRole('button', { name: /导出 Actual 文件/ }).click();
    await assertToast('Actual 导入文件已生成');
  });
  await screenshot('04-accounting-settings-desktop.png');

  await step('新增、筛选、复核与同步手工流水', async () => {
    await page.getByRole('button', { name: /新增流水/ }).click();
    await page.waitForTimeout(350);
    assert(await page.locator('#manual-entry-card input[name="amount"]').evaluate((element) => element === document.activeElement), '新增流水应聚焦金额输入框');
    await page.locator('#transaction-form select[name="type"]').selectOption('income');
    await page.locator('#transaction-form input[name="category"]').fill('UI 验收收入');
    await page.locator('#transaction-form input[name="counterparty"]').fill('自动化验收客户');
    await page.locator('#transaction-form input[name="amount"]').fill('321.45');
    await page.getByRole('button', { name: '保存待复核流水' }).click();
    const row = page.locator('.transaction-row').filter({ hasText: '自动化验收客户' });
    await row.waitFor();
    await assertText(row, '待复核');
    for (const filter of ['income', 'expense', 'needs_review', 'all']) {
      await page.locator(`[data-ledger-filter="${filter}"]`).click();
      await page.waitForTimeout(100);
    }
    await page.locator('[data-ledger-filter="needs_review"]').click();
    await row.getByRole('button', { name: /确认入账/ }).click();
    await page.locator('[data-ledger-filter="all"]').click();
    await assertText(row, '已入账');
    await row.getByRole('button', { name: '同步 Actual' }).click();
    await page.waitForTimeout(300);
  });

  await step('上传票据、生成流水并确认入账', async () => {
    await page.locator('#invoice-upload').setInputFiles(invoicePath);
    const draft = page.locator('.invoice-draft-row').filter({ hasText: 'ui-audit-invoice.txt' }).first();
    await draft.waitFor();
    await assertText(draft, 'UI 验收供应商');
    await draft.getByRole('button', { name: '生成流水' }).click();
    await page.getByText('待复核流水已生成', { exact: true }).waitFor();
    await page.locator('[data-ledger-filter="needs_review"]').click();
    const invoiceRow = page.locator('.transaction-row').filter({ hasText: 'UI 验收供应商' }).first();
    await invoiceRow.waitFor();
    await invoiceRow.getByRole('button', { name: /确认入账/ }).click();
    await page.locator('[data-ledger-filter="all"]').click();
    await assertText(invoiceRow, '已入账');
  });
  await auditCurrentView('accounting-desktop');
  await screenshot('05-invoice-ledger-desktop.png');

  await navigate('老板 AI', '老板 AI');
  await step('老板 AI 快捷问题、手工提问、工具刷新和附件', async () => {
    const before = await page.locator('.message-row').count();
    await page.getByRole('button', { name: '本月现金流和待复核账务' }).click();
    await waitForMessageCount(before + 2);
    await page.locator('#chat-form textarea[name="prompt"]').fill('请总结当前员工阻塞');
    await page.getByRole('button', { name: '发送' }).click();
    await waitForMessageText('请总结当前员工阻塞');
    await page.getByRole('button', { name: /刷新上下文/ }).click();
    await waitForWorkspaceReady();
    const messageCount = await page.locator('.message-row').count();
    await page.locator('#chat-attachment-input').setInputFiles(invoicePath);
    await waitForMessageCount(messageCount + 2);
    assert((await page.getByText('上传附件：ui-audit-invoice.txt', { exact: true }).count()) >= 1, 'AI 附件消息应显示文件名');
    const traces = page.locator('details.tool-call-box');
    assert((await traces.count()) >= 2, 'AI 回答应包含可展开工具来源');
    await traces.last().locator('summary').click();
  });
  await auditCurrentView('assistant-desktop');
  await screenshot('06-boss-ai-desktop.png');

  await step('重复票据上传保持单一草稿', async () => {
    await navigate('财务记账', '财务记账');
    forceStaleInvoiceResponse = true;
    await page.locator('#invoice-upload').setInputFiles(invoicePath);
    await page.waitForFunction(
      (fileName) => document.body.innerText.includes(`${fileName} 已进入发票草稿`),
      'ui-audit-invoice.txt'
    );
    const matchingDrafts = page.locator('.invoice-draft-row').filter({ hasText: 'ui-audit-invoice.txt' });
    assert((await matchingDrafts.count()) === 1, '同一票据重复上传不应生成重复草稿行');
    await assertText(matchingDrafts.first(), '已入账');
    assert((await matchingDrafts.first().getByRole('button', { name: '生成流水' }).count()) === 0, '陈旧幂等响应不得重新开放生成流水');
  });

  await navigate('员工与绑定', '员工与绑定');
  await step('绑定页复制、刷新与员工入口', async () => {
    await page.locator('#copy-code-secondary').click();
    await assertVisible(page.getByRole('status'), '绑定码复制提示');
    await page.getByRole('button', { name: /刷新状态/ }).click();
    await waitForWorkspaceReady();
    assert((await page.locator('.bound-employee-list article').count()) === 4, '绑定页应显示 4 名员工');
    await page.locator('.bound-employee-list [data-open-employee]').first().click();
    await assertVisible(page.getByRole('heading', { name: '员工周报' }), '绑定页员工入口');
  });
  await navigate('员工与绑定', '员工与绑定');
  await auditCurrentView('bindings-desktop');
  await screenshot('07-bindings-desktop.png');

  await step('账本超过首屏限制后可加载更多', async () => {
    const date = localIsoDate();
    for (let index = 0; index < 101; index += 1) {
      await callApiTool('ledger.add_transaction', {
        owner_code: 'BOSS-7429',
        date,
        type: 'expense',
        category: `分页验收 ${index + 1}`,
        counterparty: '自动化分页数据',
        amount: 1,
        source: 'UI E2E pagination',
        status: 'posted'
      });
    }
    await navigate('财务记账', '财务记账');
    await page.getByRole('button', { name: '刷新数据' }).click();
    await waitForWorkspaceReady();
    const loadMore = page.locator('[data-load-more-transactions]');
    await loadMore.waitFor();
    const before = await page.locator('.transaction-row').count();
    await loadMore.click();
    await page.waitForFunction((minimum) => document.querySelectorAll('.transaction-row').length > minimum, before);
  });

  await step('桌面导航所有入口可达', async () => {
    for (const [nav, heading] of [
      ['今日总览', '下午好，徐总'],
      ['员工周报', '员工周报'],
      ['财务记账', '财务记账'],
      ['老板 AI', '老板 AI'],
      ['员工与绑定', '员工与绑定']
    ]) await navigate(nav, heading);
  });

  await step('移动端主要页面无横向溢出', async () => {
    await page.setViewportSize({ width: 390, height: 844 });
    for (const [nav, heading, file] of [
      ['今日总览', '下午好，徐总', '08-mobile-overview.png'],
      ['员工周报', '员工周报', '09-mobile-employees.png'],
      ['财务记账', '财务记账', '10-mobile-accounting.png'],
      ['老板 AI', '老板 AI', '11-mobile-assistant.png'],
      ['员工与绑定', '员工与绑定', '12-mobile-bindings.png']
    ]) {
      await navigate(nav, heading);
      const overflow = await page.evaluate(() => ({
        html: document.documentElement.scrollWidth - document.documentElement.clientWidth,
        body: document.body.scrollWidth - document.body.clientWidth
      }));
      assert(overflow.html <= 1 && overflow.body <= 1, `${nav} 移动端存在横向溢出：${JSON.stringify(overflow)}`);
      const navLabels = await page.evaluate(() => [...document.querySelectorAll('.sidebar-nav .nav-button')].map((button) => {
        const label = button.querySelector('strong');
        const badge = button.querySelector('em');
        const labelRect = label?.getBoundingClientRect();
        const badgeRect = badge?.getBoundingClientRect();
        return {
          text: label?.textContent?.trim() || '',
          clipped: Boolean(label && label.scrollWidth > label.clientWidth + 1),
          badgeOverlap: Boolean(
            labelRect
            && badgeRect
            && labelRect.left < badgeRect.right
            && labelRect.right > badgeRect.left
            && labelRect.top < badgeRect.bottom
            && labelRect.bottom > badgeRect.top
          )
        };
      }));
      assert(navLabels.every((item) => !item.clipped), `${nav} 移动导航文字被截断：${JSON.stringify(navLabels)}`);
      assert(navLabels.every((item) => !item.badgeOverlap), `${nav} 移动导航 badge 挤压文字：${JSON.stringify(navLabels)}`);
      const activeNav = await page.evaluate((expected) => {
        const buttons = [...document.querySelectorAll('.sidebar-nav .nav-button')];
        const active = document.querySelector('.sidebar-nav .nav-button.is-active');
        const viewportWidth = document.documentElement.clientWidth;
        return {
          buttonCount: buttons.length,
          allInViewport: buttons.every((button) => {
            const rect = button.getBoundingClientRect();
            return rect.left >= -1 && rect.right <= viewportWidth + 1;
          }),
          activeText: active?.querySelector('strong')?.textContent?.trim() || '',
          activeInViewport: active ? (() => {
            const rect = active.getBoundingClientRect();
            return rect.left >= -1 && rect.right <= viewportWidth + 1;
          })() : false,
          expected
        };
      }, nav);
      assert(activeNav.buttonCount === 5 && activeNav.allInViewport, `${nav} 五个导航入口应同时可见：${JSON.stringify(activeNav)}`);
      assert(activeNav.activeText === nav && activeNav.activeInViewport, `${nav} 当前导航未正确显示 active：${JSON.stringify(activeNav)}`);
      await auditCurrentView(`${nav}-mobile`);
      await screenshot(file, false);
    }
  });

  await step('退出返回登录页', async () => {
    await page.getByRole('button', { name: '退出' }).click();
    await assertVisible(page.getByRole('heading', { name: '先看关键进展，再做关键决定' }), '退出后的登录页');
  });

  assert(pageErrors.length === 0, `页面异常：${pageErrors.join(' | ')}`);
  assert(consoleErrors.length === 0, `控制台错误：${consoleErrors.join(' | ')}`);
  assert(requestFailures.length === 0, `网络请求失败：${requestFailures.join(' | ')}`);

  await writeReport('passed');
  process.stdout.write(`Boss UI final audit passed. Artifacts: ${artifactDir}\n`);
} catch (error) {
  await writeReport('failed', error);
  if (page) await screenshot('99-failure.png').catch(() => {});
  throw error;
} finally {
  if (browser) await browser.close().catch(() => {});
  for (const child of children.reverse()) stopProcess(child);
  await rm(tempDir, { recursive: true, force: true }).catch(() => {});
}

async function navigate(navName, heading) {
  const navigation = page.getByRole('navigation', { name: '老板端导航' });
  await navigation.getByRole('button', { name: new RegExp(navName) }).click();
  await assertVisible(page.getByRole('heading', { name: heading, exact: true }), `${navName} 页面标题`);
}

async function waitForWorkspaceReady() {
  await page.waitForFunction(() => {
    const freshness = document.querySelector('.data-freshness');
    const footer = document.querySelector('.sidebar-footer strong');
    return !document.querySelector('[data-refresh-workspace][disabled]')
      && !footer?.textContent?.includes('正在同步')
      && (!freshness || !freshness.classList.contains('loading'));
  });
}

async function waitForMessageCount(count) {
  await page.waitForFunction((minimum) => document.querySelectorAll('.message-row').length >= minimum, count);
  await page.waitForFunction(() => !document.querySelector('.message-row:last-child .message-body p')?.textContent?.includes('正在读取'));
}

async function waitForMessageText(text) {
  await page.getByText(text, { exact: true }).waitFor();
  await page.waitForFunction(() => !document.querySelector('.message-row:last-child .message-body p')?.textContent?.includes('正在读取'));
}

async function auditCurrentView(name) {
  const audit = await page.evaluate(() => {
    const visible = (element) => {
      const style = getComputedStyle(element);
      const rect = element.getBoundingClientRect();
      return style.visibility !== 'hidden' && style.display !== 'none' && rect.width > 0 && rect.height > 0;
    };
    const buttons = [...document.querySelectorAll('button')].filter(visible);
    const unnamedButtons = buttons.filter((button) => !(button.textContent?.trim() || button.getAttribute('aria-label') || button.getAttribute('title')));
    const controls = [...document.querySelectorAll('input:not([type="hidden"]), select, textarea')].filter(visible);
    const unlabeledControls = controls.filter((control) => {
      if (control.getAttribute('aria-label') || control.getAttribute('title')) return false;
      if (control.id && document.querySelector(`label[for="${CSS.escape(control.id)}"]`)) return false;
      return !control.closest('label');
    });
    return {
      title: document.querySelector('h1')?.textContent?.trim() || '',
      buttons: buttons.length,
      controls: controls.length,
      unnamedButtons: unnamedButtons.map((button) => button.outerHTML.slice(0, 200)),
      unlabeledControls: unlabeledControls.map((control) => control.outerHTML.slice(0, 200)),
      scrollWidth: document.documentElement.scrollWidth,
      clientWidth: document.documentElement.clientWidth
    };
  });
  assert(audit.unnamedButtons.length === 0, `${name} 存在无名称按钮：${audit.unnamedButtons.join(' | ')}`);
  assert(audit.unlabeledControls.length === 0, `${name} 存在无标签控件：${audit.unlabeledControls.join(' | ')}`);
  viewAudits.push({ name, ...audit });
}

async function step(name, callback) {
  const startedAt = Date.now();
  try {
    await callback();
    steps.push({ name, status: 'passed', durationMs: Date.now() - startedAt });
  } catch (error) {
    steps.push({ name, status: 'failed', durationMs: Date.now() - startedAt, error: String(error) });
    throw error;
  }
}

async function screenshot(name, fullPage = true) {
  await page.evaluate(() => window.scrollTo({ top: 0, left: 0, behavior: 'instant' }));
  await page.waitForTimeout(60);
  await page.screenshot({ path: join(artifactDir, name), fullPage, animations: 'disabled' });
}

async function assertVisible(locator, label) {
  await locator.waitFor({ state: 'visible', timeout: 12_000 });
  assert(await locator.isVisible(), `${label} 不可见`);
}

async function assertText(locator, text) {
  await locator.waitFor({ state: 'visible', timeout: 12_000 });
  const value = await locator.evaluate((element) => 'value' in element ? element.value : element.textContent || '');
  assert(value.includes(text), `期望文本“${text}”，实际为“${value.trim()}”`);
}

async function assertToast(text) {
  await page.waitForFunction(
    (expected) => document.querySelector('[role="status"]')?.textContent?.includes(expected),
    text,
    { timeout: 12_000 }
  );
}

async function callApiTool(name, argumentsValue) {
  const response = await fetch(`${apiUrl}/api/boss/tool`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ name, arguments: argumentsValue })
  });
  const payload = await response.json();
  assert(response.ok && payload.ok, `${name} 预置数据失败：${JSON.stringify(payload)}`);
  return payload.result;
}

function localIsoDate(date = new Date()) {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function isDarkCssColor(value) {
  const channels = String(value).match(/[\d.]+/g)?.slice(0, 3).map(Number) || [];
  if (channels.length !== 3 || channels.some((channel) => !Number.isFinite(channel))) return false;
  const brightness = ((0.2126 * channels[0]) + (0.7152 * channels[1]) + (0.0722 * channels[2])) / 255;
  return brightness < 0.35;
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function writeReport(status, error) {
  await mkdir(artifactDir, { recursive: true });
  await writeFile(join(artifactDir, 'report.json'), JSON.stringify({
    status,
    generatedAt: new Date().toISOString(),
    webUrl,
    apiUrl,
    tempDatabase: join(tempDir, 'boss.sqlite'),
    steps,
    viewAudits,
    consoleErrors,
    pageErrors,
    requestFailures,
    error: error ? String(error.stack || error) : null
  }, null, 2), 'utf8');
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

async function waitForUrl(url, child, label, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`${label} 提前退出（${child.exitCode}）：${child.output}`);
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // The process is still starting.
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 200));
  }
  throw new Error(`${label} 启动超时：${child.output}`);
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
