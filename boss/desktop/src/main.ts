import './index.css';
import { invoke } from '@tauri-apps/api/core';
import {
  AlertTriangle,
  ArrowRight,
  Bell,
  Bot,
  CalendarDays,
  Check,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  CircleDollarSign,
  Clock3,
  Copy,
  createIcons,
  Database,
  FileText,
  Filter,
  LayoutDashboard,
  LogOut,
  MessageSquareText,
  Moon,
  Paperclip,
  Plus,
  ReceiptText,
  RefreshCw,
  Search,
  Send,
  Sun,
  TrendingDown,
  TrendingUp,
  Upload,
  UserRoundCog,
  UsersRound,
  WalletCards,
  X
} from 'lucide';

type View = 'overview' | 'employees' | 'accounting' | 'assistant' | 'bindings';
type ThemeMode = 'light' | 'dark';
type EmployeeStatus = 'active' | 'blocked' | 'idle';
type EmployeeFilter = 'all' | 'attention' | 'reviewed';
type LedgerFilter = 'all' | 'needs_review' | 'income' | 'expense';
type ReviewStatus = 'pending' | 'reviewed' | 'needs_supplement';
type WorkItemStatus = 'done' | 'in_progress' | 'blocked' | 'reported';
type SummaryMode = 'summary' | 'team' | 'actions' | 'finance';
type ToolName =
  | 'boss.owner_context'
  | 'boss.dashboard_summary'
  | 'boss.employee_work_report'
  | 'boss.save_week_review'
  | 'owner.list_bound_employees'
  | 'ledger.add_transaction'
  | 'ledger.list_transactions'
  | 'ledger.update_transaction_status'
  | 'ledger.report'
  | 'invoice.upload_and_extract'
  | 'invoice.post_to_ledger'
  | 'invoice.list_drafts'
  | 'actual.integration_status'
  | 'actual.sync_transaction'
  | 'actual.export_import_file';

type WorkItem = {
  id: string;
  day: string;
  title: string;
  output: string;
  status: WorkItemStatus;
};

type Employee = {
  id: string;
  name: string;
  role: string;
  status: EmployeeStatus;
  quality: number;
  tasksDone: number;
  materials: number;
  cost: number;
  lastWork: string;
  signals: string[];
  weeklyTarget: number | null;
  workDays: number;
  trend: number;
  lastUpdated: string;
  weeklyFocus: string;
  nextPlan: string;
  workItems: WorkItem[];
};

type BossAction = {
  id: string;
  title: string;
  description: string;
  source: string;
  due: string;
  severity: 'urgent' | 'normal';
  targetView: View;
  employeeId?: string;
  transactionId?: string;
};

type Transaction = {
  id: string;
  backendId?: string;
  date: string;
  type: 'income' | 'expense';
  category: string;
  counterparty: string;
  amount: number;
  currency: string;
  source: string;
  evidence?: string;
  status: 'posted' | 'needs_review' | 'synced';
};

type InvoiceDraft = {
  id: string;
  backendId?: string;
  fileName: string;
  status: 'extracted' | 'needs_review' | 'needs_ai_ocr_adapter' | 'needs_file' | 'file_not_found' | 'ledger_draft' | 'posted_to_ledger';
  source: string;
  uploadedAt: string;
  fields: {
    invoiceNumber: string | null;
    invoiceCode: string | null;
    invoiceDate: string | null;
    seller: string | null;
    buyer: string | null;
    amount: number | null;
    tax: number | null;
    total: number | null;
    currency: string;
    categorySuggestion: string | null;
    confidence: number;
  };
};

type ActualSyncJob = {
  id: string;
  transactionId: string;
  status: 'queued' | 'running' | 'dry_run' | 'synced' | 'failed';
  reason: string;
  createdAt: string;
};

type ActualStatus = {
  localPath: string;
  apiPackage: string;
  cliPackage: string;
  packageManager: string;
  sourcePresent: boolean;
  cliAvailable: boolean;
  directSyncReady: boolean;
  serverUrlConfigured: boolean;
  syncIdConfigured: boolean;
  credentialConfigured: boolean;
  defaultAccountIdConfigured: boolean;
  statusMessage: string;
  lastCheckedAt: string;
  requiredEnv: string[];
  capabilities: string[];
};

type LedgerSummary = {
  month: string;
  income: number;
  expense: number;
  net: number;
  needsReview: number;
  pendingAmount: number;
  transactionCount: number;
  incomeCount: number;
  expenseCount: number;
  expenseByCategory: Record<string, number>;
};

type ChatMessage = {
  id: string;
  role: 'owner' | 'assistant';
  content: string;
  toolRun?: {
    name: ToolName;
    label: string;
    summary: string;
  };
};

const OWNER_CODE = 'BOSS-7429';
const THEME_STORAGE_KEY = 'cohmira.themeMode';
const ACTUAL_REPO_URL = 'https://github.com/actualbudget/actual';
const ACTUAL_LOCAL_PATH = 'third_party/actual';
const actualStatus: ActualStatus = {
  localPath: ACTUAL_LOCAL_PATH,
  apiPackage: '@actual-app/api@26.6.0',
  cliPackage: '@actual-app/cli@26.6.0',
  packageManager: 'yarn@4.13.0',
  sourcePresent: true,
  cliAvailable: false,
  directSyncReady: false,
  serverUrlConfigured: false,
  syncIdConfigured: false,
  credentialConfigured: false,
  defaultAccountIdConfigured: false,
  statusMessage: '本地状态未检查',
  lastCheckedAt: '',
  requiredEnv: ['ACTUAL_SERVER_URL', 'ACTUAL_SYNC_ID', 'ACTUAL_PASSWORD 或 ACTUAL_SESSION_TOKEN'],
  capabilities: ['accounts', 'transactions', 'categories', 'payees', 'rules', 'schedules', 'query', 'sync']
};

const employees: Employee[] = [
  {
    id: 'emp-001',
    name: '林倩',
    role: '小红书素材与评论',
    status: 'active',
    quality: 93,
    tasksDone: 18,
    materials: 64,
    cost: 328,
    lastWork: '完成 6 条竞品评论归档，3 条进入选题池',
    signals: ['素材引用完整', '评论归档质量稳定', '本周目标进度正常'],
    weeklyTarget: 22,
    workDays: 3,
    trend: 8,
    lastUpdated: '今天 14:32',
    weeklyFocus: '围绕夏季通勤场景补齐竞品评论样本，并筛出可转化的选题方向。',
    nextPlan: '完成高互动评论聚类，提交 5 个可进入脚本阶段的选题。',
    workItems: [
      { id: 'work-001', day: '周一', title: '竞品评论批量归档', output: '归档 28 条，标注 6 个高频痛点', status: 'done' },
      { id: 'work-002', day: '周二', title: '通勤场景素材清洗', output: '沉淀 31 份可引用素材', status: 'done' },
      { id: 'work-003', day: '今天', title: '选题池二次筛选', output: '3 条已入池，2 条待补证据', status: 'in_progress' }
    ]
  },
  {
    id: 'emp-002',
    name: '周祺',
    role: '视频脚本与封面',
    status: 'blocked',
    quality: 86,
    tasksDone: 9,
    materials: 31,
    cost: 512,
    lastWork: '封面 A/B 版本等待老板确认',
    signals: ['A 版点击预估高', 'B 版品牌一致性更好', '需要 18:00 前确认方向'],
    weeklyTarget: 15,
    workDays: 3,
    trend: -4,
    lastUpdated: '今天 13:18',
    weeklyFocus: '完成新品种草视频脚本与两套封面方向，进入老板确认后排产。',
    nextPlan: '按确认方向完成封面定稿，并将脚本拆成 3 个拍摄镜头包。',
    workItems: [
      { id: 'work-004', day: '周一', title: '新品脚本第一版', output: '完成 1,240 字脚本与镜头建议', status: 'done' },
      { id: 'work-005', day: '周二', title: '封面方向 A/B 测试', output: '输出 2 套方案与点击预估', status: 'done' },
      { id: 'work-006', day: '今天', title: '封面方向确认', output: '等待老板选择 A 或 B 版', status: 'blocked' }
    ]
  },
  {
    id: 'emp-003',
    name: '赵敏',
    role: '店铺客服与私信',
    status: 'active',
    quality: 91,
    tasksDone: 24,
    materials: 17,
    cost: 96,
    lastWork: '蒲公英合作咨询已分级，2 条需要商务跟进',
    signals: ['无超时会话', '新增合作线索 2 条', '评论风险低'],
    weeklyTarget: 28,
    workDays: 3,
    trend: 12,
    lastUpdated: '今天 15:06',
    weeklyFocus: '保持客服响应时效，筛选高意向合作咨询并完成风险评论分级。',
    nextPlan: '跟进 2 条商务线索，整理本周高频售前问题供内容团队使用。',
    workItems: [
      { id: 'work-007', day: '周一', title: '客服会话巡检', output: '处理 42 个会话，无超时', status: 'done' },
      { id: 'work-008', day: '周二', title: '合作咨询分级', output: '识别 2 条高意向线索', status: 'done' },
      { id: 'work-009', day: '今天', title: '评论风险复核', output: '17 条完成分级，风险可控', status: 'done' }
    ]
  },
  {
    id: 'emp-004',
    name: '陈越',
    role: '投放与数据复盘',
    status: 'idle',
    quality: 78,
    tasksDone: 7,
    materials: 9,
    cost: 184,
    lastWork: '上午投放日报缺少转化截图',
    signals: ['数据链路不完整', '需要补齐转化截图', '午后任务空窗'],
    weeklyTarget: 12,
    workDays: 2,
    trend: -11,
    lastUpdated: '今天 10:24',
    weeklyFocus: '完成投放日报和素材效率复盘，给出下半周预算调整建议。',
    nextPlan: '补齐转化证据，按素材维度拆解 CPA 并提交预算调整表。',
    workItems: [
      { id: 'work-010', day: '周一', title: '投放账户数据汇总', output: '完成 3 个账户基础数据归集', status: 'done' },
      { id: 'work-011', day: '周二', title: '素材效率对比', output: '完成初版，缺转化截图', status: 'blocked' },
      { id: 'work-012', day: '今天', title: '预算调整建议', output: '尚未开始', status: 'in_progress' }
    ]
  }
];

const initialTransactions: Transaction[] = [
  {
    id: 'txn-1001',
    date: '2026-07-15',
    type: 'income',
    category: '客户回款',
    counterparty: '杭州某品牌店铺',
    amount: 12800,
    currency: 'CNY',
    source: '合同回款截图',
    status: 'posted'
  },
  {
    id: 'txn-1002',
    date: '2026-07-15',
    type: 'expense',
    category: '达人样品',
    counterparty: '供应链样品费',
    amount: 1880,
    currency: 'CNY',
    source: '发票识别草稿',
    status: 'needs_review'
  },
  {
    id: 'txn-1003',
    date: '2026-07-14',
    type: 'expense',
    category: 'AI 工具',
    counterparty: '模型 API 充值',
    amount: 600,
    currency: 'CNY',
    source: '手动录入',
    status: 'posted'
  },
  {
    id: 'txn-1004',
    date: '2026-07-14',
    type: 'expense',
    category: '素材采集',
    counterparty: '第三方数据接口',
    amount: 420,
    currency: 'CNY',
    source: '工具同步',
    status: 'posted'
  }
];

const initialInvoiceDrafts: InvoiceDraft[] = [
  {
    id: 'inv-1001',
    fileName: '供应链样品费-发票.txt',
    status: 'extracted',
    source: '发票识别草稿',
    uploadedAt: '2026-06-30 11:28',
    fields: {
      invoiceNumber: 'FP202606300001',
      invoiceCode: '044002600111',
      invoiceDate: '2026-06-30',
      seller: '供应链样品费',
      buyer: '商媒运营助手 · 老板指挥台',
      amount: 1773.58,
      tax: 106.42,
      total: 1880,
      currency: 'CNY',
      categorySuggestion: '达人样品',
      confidence: 0.91
    }
  }
];

const root = document.querySelector<HTMLDivElement>('#root');

if (!root) {
  throw new Error('Missing #root element');
}

const appRoot = root;

function readInitialThemeMode(): ThemeMode {
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (stored === 'light' || stored === 'dark') return stored;
  } catch {
    // Fall through to the operating-system preference when storage is unavailable.
  }
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

function applyThemeMode(mode: ThemeMode): void {
  document.documentElement.setAttribute('data-theme', mode);
  document.documentElement.classList.toggle('dark', mode === 'dark');
}

const iconNodes = {
  AlertTriangle,
  ArrowRight,
  Bell,
  Bot,
  CalendarDays,
  Check,
  CheckCircle2,
  ChevronLeft,
  ChevronRight,
  CircleDollarSign,
  Clock3,
  Copy,
  Database,
  FileText,
  Filter,
  LayoutDashboard,
  LogOut,
  MessageSquareText,
  Moon,
  Paperclip,
  Plus,
  ReceiptText,
  RefreshCw,
  Search,
  Send,
  Sun,
  TrendingDown,
  TrendingUp,
  Upload,
  UserRoundCog,
  UsersRound,
  WalletCards,
  X
};

function readStoredRecord(key: string): Record<string, string> {
  try {
    const value = JSON.parse(window.localStorage.getItem(key) || '{}');
    if (!isRecord(value)) return {};
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, string] => typeof entry[1] === 'string'));
  } catch {
    return {};
  }
}

function readStoredReviews(): Record<string, ReviewStatus> {
  const stored = readStoredRecord('cohmira.employeeReviews');
  return Object.fromEntries(
    Object.entries(stored).filter((entry): entry is [string, ReviewStatus] => ['pending', 'reviewed', 'needs_supplement'].includes(entry[1]))
  );
}

function writeStoredRecord(key: string, value: Record<string, string>): void {
  try {
    window.localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // In-memory review state remains usable when browser storage is unavailable.
  }
}

const state: {
  signedIn: boolean;
  loginError: string;
  currentView: View;
  messages: ChatMessage[];
  transactions: Transaction[];
  invoiceDrafts: InvoiceDraft[];
  actualSyncJobs: ActualSyncJob[];
  actualSyncJobCounts: Record<string, number>;
  ledgerSummary: LedgerSummary | null;
  ledgerTotal: number;
  ledgerNextOffset: number;
  ledgerHasMore: boolean;
  ledgerLoadingMore: boolean;
  selectedEmployeeId: string;
  uploadStatus: string;
  actualChecking: boolean;
  actualBridgeError: string;
  weekOffset: number;
  employeeFilter: EmployeeFilter;
  employeeSearch: string;
  ledgerFilter: LedgerFilter;
  employeeReviews: Record<string, ReviewStatus>;
  employeeNotes: Record<string, string>;
  employeeNoteDrafts: Record<string, string>;
  ownerContextStatus: 'idle' | 'loading' | 'ready' | 'partial' | 'error';
  ownerContextError: string;
  dataAsOf: string;
  loadedWeekStart: string;
  loadedWeekEnd: string;
  showFinanceSettings: boolean;
  themeMode: ThemeMode;
  toast: string;
} = {
  signedIn: false,
  loginError: '',
  currentView: 'overview',
  selectedEmployeeId: employees[0]?.id ?? '',
  transactions: [...initialTransactions],
  invoiceDrafts: [...initialInvoiceDrafts],
  actualSyncJobs: [],
  actualSyncJobCounts: {},
  ledgerSummary: null,
  ledgerTotal: initialTransactions.length,
  ledgerNextOffset: initialTransactions.length,
  ledgerHasMore: false,
  ledgerLoadingMore: false,
  uploadStatus: '等待上传发票或收据',
  actualChecking: false,
  actualBridgeError: '',
  weekOffset: 0,
  employeeFilter: 'all',
  employeeSearch: '',
  ledgerFilter: 'all',
  employeeReviews: readStoredReviews(),
  employeeNotes: readStoredRecord('cohmira.employeeNotes'),
  employeeNoteDrafts: readStoredRecord('cohmira.employeeNoteDrafts'),
  ownerContextStatus: 'idle',
  ownerContextError: '',
  dataAsOf: '本地演示数据',
  loadedWeekStart: '',
  loadedWeekEnd: '',
  showFinanceSettings: false,
  themeMode: readInitialThemeMode(),
  toast: '',
  messages: [
    {
      id: 'welcome',
      role: 'assistant',
      content: '我会先读取员工周报、阻塞、账本和发票草稿，再给出可追溯的管理结论。'
    }
  ]
};

applyThemeMode(state.themeMode);

function normalizeCurrencyCode(value: unknown): string {
  const code = typeof value === 'string' ? value.trim().toUpperCase() : '';
  return /^[A-Z]{3}$/.test(code) ? code : 'CNY';
}

function currency(value: number, currencyCode = 'CNY'): string {
  const digits = Number.isInteger(value) ? 0 : 2;
  try {
    return new Intl.NumberFormat('zh-CN', {
      style: 'currency',
      currency: normalizeCurrencyCode(currencyCode),
      minimumFractionDigits: digits,
      maximumFractionDigits: 2
    }).format(value);
  } catch {
    return `CNY ${value.toFixed(digits)}`;
  }
}

function renderIcon(name: string, size = 18): string {
  return `<i data-lucide="${name}" width="${size}" height="${size}" aria-hidden="true"></i>`;
}

function localIsoDate(date = new Date()): string {
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

function getWeekRange(offset = state.weekOffset) {
  const pivot = new Date();
  pivot.setHours(12, 0, 0, 0);
  pivot.setDate(pivot.getDate() + offset * 7);
  const mondayIndex = (pivot.getDay() + 6) % 7;
  const start = new Date(pivot);
  start.setDate(pivot.getDate() - mondayIndex);
  const end = new Date(start);
  end.setDate(start.getDate() + 6);
  const short = (value: Date) => `${value.getMonth() + 1}月${value.getDate()}日`;
  return {
    start,
    end,
    startIso: localIsoDate(start),
    endIso: localIsoDate(end),
    label: `${short(start)} - ${short(end)}`,
    relativeLabel: offset === 0 ? '本周' : offset === -1 ? '上周' : `${Math.abs(offset)} 周前`
  };
}

function employeeReviewKey(employeeId: string): string {
  return `${getWeekRange().startIso}:${employeeId}`;
}

function employeeReviewStatus(employeeId: string): ReviewStatus | undefined {
  return state.employeeReviews[employeeReviewKey(employeeId)];
}

function completionRate(employee: Employee): number {
  if (!employee.weeklyTarget) return 0;
  return Math.max(0, Math.min(100, Math.round(employee.tasksDone / employee.weeklyTarget * 100)));
}

function workStatusLabel(status: WorkItemStatus): string {
  if (status === 'done') return '已完成';
  if (status === 'blocked') return '有阻塞';
  if (status === 'reported') return '已回传';
  return '进行中';
}

let toastTimer: number | undefined;
let workspaceRequestSequence = 0;
const invoicePostingIds = new Set<string>();

function showToast(message: string): void {
  state.toast = message;
  let element = document.querySelector<HTMLDivElement>('.toast');
  if (!element) {
    element = document.createElement('div');
    element.className = 'toast';
    element.setAttribute('role', 'status');
    appRoot.append(element);
  }
  element.textContent = message;
  if (toastTimer) window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => {
    state.toast = '';
    document.querySelector<HTMLDivElement>('.toast')?.remove();
  }, 2200);
}

function escapeHtml(value: string): string {
  return String(value)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function isTauriRuntime(): boolean {
  return '__TAURI_INTERNALS__' in window;
}

function unwrapBackendToolResult<T>(payload: unknown, name: ToolName): T {
  if (isRecord(payload) && typeof payload.ok === 'boolean') {
    if (!payload.ok) {
      throw new Error(typeof payload.error === 'string' ? payload.error : `工具调用失败：${name}`);
    }
    return payload.result as T;
  }
  return payload as T;
}

async function callBackendTool<T>(name: ToolName, args: Record<string, unknown>): Promise<T> {
  if (isTauriRuntime()) {
    const payload = await invoke<unknown>('call_boss_tool', { name, args });
    return unwrapBackendToolResult<T>(payload, name);
  }

  const response = await fetch('/api/boss/tool', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ name, arguments: args })
  });
  const payload = await response.json().catch(() => null);
  if (!response.ok) {
    const error = isRecord(payload) && typeof payload.error === 'string'
      ? payload.error
      : `工具调用失败：${name}`;
    throw new Error(error);
  }
  return unwrapBackendToolResult<T>(payload, name);
}

function packageLabel(name: unknown, version: unknown): string {
  const packageName = typeof name === 'string' && name ? name : '未识别';
  return typeof version === 'string' && version ? `${packageName}@${version}` : packageName;
}

function applyActualIntegrationStatus(result: unknown): void {
  const payload = isRecord(result) ? result : {};
  const actual = isRecord(payload.actual) ? payload.actual : {};
  actualStatus.localPath = typeof actual.local_path === 'string' ? actual.local_path : ACTUAL_LOCAL_PATH;
  actualStatus.apiPackage = packageLabel(actual.api_package, actual.api_version);
  actualStatus.cliPackage = packageLabel(actual.cli_package, actual.cli_version);
  actualStatus.packageManager = typeof actual.package_manager === 'string' ? actual.package_manager : actualStatus.packageManager;
  actualStatus.sourcePresent = Boolean(actual.source_present);
  actualStatus.cliAvailable = Boolean(actual.cli_available);
  actualStatus.serverUrlConfigured = Boolean(actual.server_url_configured);
  actualStatus.syncIdConfigured = Boolean(actual.sync_id_configured);
  actualStatus.credentialConfigured = Boolean(actual.credential_configured);
  actualStatus.defaultAccountIdConfigured = Boolean(actual.default_account_id_configured);
  actualStatus.directSyncReady = Boolean(actual.ready_for_direct_cli_sync && actual.default_account_id_configured);
  actualStatus.requiredEnv = Array.isArray(actual.required_env) ? actual.required_env.map(String) : actualStatus.requiredEnv;
  actualStatus.capabilities = Array.isArray(actual.capabilities) ? actual.capabilities.map(String) : actualStatus.capabilities;
  actualStatus.lastCheckedAt = new Date().toLocaleString('zh-CN', { hour12: false });
  actualStatus.statusMessage = actualStatus.directSyncReady
    ? '已读取本地 .env，Actual 可直连'
    : '缺少 Actual server、sync id、凭据或默认账户';
  state.actualBridgeError = '';
  if (Array.isArray(payload.sync_jobs)) {
    state.actualSyncJobs = payload.sync_jobs.map((value, index) => {
      const record = isRecord(value) ? value : {};
      const rawStatus = typeof record.status === 'string' ? record.status : 'failed';
      const status: ActualSyncJob['status'] = ['queued', 'running', 'dry_run', 'synced', 'failed'].includes(rawStatus)
        ? rawStatus as ActualSyncJob['status']
        : 'failed';
      let reason = status === 'queued'
        ? '等待 Actual 配置或重试'
        : status === 'running'
          ? '正在同步 Actual'
          : status === 'failed'
            ? '同步失败'
            : '同步任务已记录';
      if (typeof record.response_json === 'string') {
        try {
          const response = JSON.parse(record.response_json);
          if (isRecord(response)) {
            if (typeof response.reason === 'string') reason = response.reason;
            else if (typeof response.stderr === 'string' && response.stderr.trim()) reason = response.stderr.trim();
          }
        } catch {
          // Preserve the status-derived reason for malformed historical records.
        }
      }
      return {
        id: typeof record.id === 'string' ? record.id : `actual-job-${index}`,
        transactionId: typeof record.transaction_id === 'string' ? record.transaction_id : 'unknown',
        status,
        reason,
        createdAt: typeof record.created_at === 'string' ? record.created_at : ''
      };
    });
  }
  if (Array.isArray(payload.sync_job_counts)) {
    state.actualSyncJobCounts = payload.sync_job_counts.reduce<Record<string, number>>((counts, value) => {
      if (!isRecord(value) || typeof value.status !== 'string') return counts;
      counts[value.status] = numberFromUnknown(value.count) ?? 0;
      return counts;
    }, {});
  }
}

function applyLedgerReport(result: unknown): void {
  const context = isRecord(result) ? result : {};
  const report = isRecord(context.ledger_report) ? context.ledger_report : context;
  if (!Array.isArray(report.rows)) return;
  const summary: LedgerSummary = {
    month: typeof report.month === 'string' ? report.month : localIsoDate().slice(0, 7),
    income: 0,
    expense: 0,
    net: 0,
    needsReview: 0,
    pendingAmount: 0,
    transactionCount: 0,
    incomeCount: 0,
    expenseCount: 0,
    expenseByCategory: {}
  };
  report.rows.forEach((value) => {
    if (!isRecord(value)) return;
    if (normalizeCurrencyCode(value.currency) !== 'CNY') return;
    const amount = (numberFromUnknown(value.amount_cents) ?? 0) / 100;
    const count = numberFromUnknown(value.count) ?? 0;
    summary.transactionCount += count;
    if (value.status === 'needs_review') {
      summary.needsReview += count;
      summary.pendingAmount += amount;
      return;
    }
    if (value.type === 'income') {
      summary.income += amount;
      summary.incomeCount += count;
    }
    if (value.type === 'expense') {
      summary.expense += amount;
      summary.expenseCount += count;
      const category = typeof value.category === 'string' ? value.category : '未分类';
      summary.expenseByCategory[category] = (summary.expenseByCategory[category] ?? 0) + amount;
    }
  });
  summary.net = summary.income - summary.expense;
  state.ledgerSummary = summary;
}

function applyTransactionList(result: unknown, replace: boolean, updatePagination = true): void {
  const payload = isRecord(result) ? result : {};
  const rows = Array.isArray(payload.transactions) ? payload.transactions.map(mapBackendTransaction) : [];
  if (replace) {
    state.transactions = rows;
  } else {
    const byId = new Map(state.transactions.map((transaction) => [transaction.id, transaction]));
    rows.forEach((transaction) => byId.set(transaction.id, transaction));
    state.transactions = [...byId.values()].sort((a, b) => b.date.localeCompare(a.date));
  }
  if (!updatePagination) return;
  const pagination = isRecord(payload.pagination) ? payload.pagination : {};
  state.ledgerTotal = numberFromUnknown(pagination.total) ?? state.transactions.length;
  state.ledgerNextOffset = numberFromUnknown(pagination.next_offset) ?? rows.length;
  state.ledgerHasMore = Boolean(pagination.has_more);
}

function applyInvoiceList(result: unknown): void {
  const payload = isRecord(result) ? result : {};
  const rows = Array.isArray(payload.drafts) ? payload.drafts : [];
  state.invoiceDrafts = rows.map((draft) => {
    const record = isRecord(draft) ? draft : {};
    const filePath = typeof record.file_path === 'string' ? record.file_path : '';
    const fileName = filePath.split(/[\\/]/).pop() || (typeof record.id === 'string' ? record.id : '未命名票据');
    return mapBackendInvoiceDraft(draft, fileName);
  });
}

function applyBackendEmployees(result: unknown): number {
  const context = isRecord(result) ? result : {};
  const bindings = isRecord(context.bindings) ? context.bindings : {};
  const hasEmployeePayload = Array.isArray(context.employees) || Array.isArray(bindings.employees);
  const rows = Array.isArray(context.employees)
    ? context.employees
    : Array.isArray(bindings.employees)
      ? bindings.employees
      : [];
  const backendEmployeeIds = new Set<string>();
  rows.forEach((value) => {
    if (!isRecord(value) || typeof value.id !== 'string') return;
    backendEmployeeIds.add(value.id);
    let employee = employees.find((item) => item.id === value.id);
    if (!employee) {
      const status: EmployeeStatus = value.status === 'active' || value.status === 'blocked' || value.status === 'idle'
        ? value.status
        : 'idle';
      employee = {
        id: value.id,
        name: typeof value.name === 'string' ? value.name : '未命名员工',
        role: typeof value.role === 'string' ? value.role : '岗位未设置',
        status,
        quality: 0,
        tasksDone: 0,
        materials: 0,
        cost: 0,
        lastWork: typeof value.last_summary === 'string' ? value.last_summary : '尚未回传工作摘要',
        signals: [],
        weeklyTarget: null,
        workDays: 0,
        trend: 0,
        lastUpdated: '尚未回传',
        weeklyFocus: '',
        nextPlan: '',
        workItems: []
      };
      employees.push(employee);
    }
    if (typeof value.name === 'string') employee.name = value.name;
    if (typeof value.role === 'string') employee.role = value.role;
    if (value.status === 'active' || value.status === 'blocked' || value.status === 'idle') employee.status = value.status;
    employee.lastWork = typeof value.last_summary === 'string' && value.last_summary
      ? value.last_summary
      : '所选周尚未回传工作摘要';
    if (typeof value.last_event_date === 'string' && value.last_event_date) {
      employee.lastUpdated = `最近 ${value.last_event_date.slice(5).replace('-', '/')}`;
    } else {
      employee.lastUpdated = '所选周尚未回传';
    }
  });
  if (hasEmployeePayload) {
    const boundEmployees = employees.filter((employee) => backendEmployeeIds.has(employee.id));
    employees.splice(0, employees.length, ...boundEmployees);
    if (!employees.some((employee) => employee.id === state.selectedEmployeeId)) {
      state.selectedEmployeeId = employees[0]?.id ?? '';
    }
  }

  const report = isRecord(context.work_report) ? context.work_report : {};
  const events = Array.isArray(report.events) ? report.events : [];
  employees.forEach((employee) => {
    employee.workItems = [];
    employee.tasksDone = 0;
    employee.workDays = 0;
    employee.weeklyTarget = null;
    employee.materials = 0;
    employee.cost = 0;
    employee.quality = 0;
  });
  const grouped = new Map<string, {
    workItems: WorkItem[];
    materials: number;
    cost: number;
    qualityTotal: number;
    qualityCount: number;
  }>();
  events.forEach((value, index) => {
    if (!isRecord(value) || typeof value.employee_id !== 'string') return;
    const summary = typeof value.summary === 'string' ? value.summary : '员工端已回传工作事件';
    const taskType = typeof value.task_type === 'string' ? value.task_type : 'work_event';
    const date = typeof value.event_date === 'string' ? value.event_date.slice(5).replace('-', '/') : '本周';
    const item: WorkItem = {
      id: typeof value.id === 'string' ? value.id : `backend-work-${index}`,
      day: date,
      title: taskType.replaceAll('_', ' '),
      output: summary,
      status: 'reported'
    };
    const current = grouped.get(value.employee_id) ?? {
      workItems: [],
      materials: 0,
      cost: 0,
      qualityTotal: 0,
      qualityCount: 0
    };
    const quality = numberFromUnknown(value.quality_score);
    current.workItems.push(item);
    current.materials += numberFromUnknown(value.material_count) ?? 0;
    current.cost += (numberFromUnknown(value.cost_cents) ?? 0) / 100;
    if (quality !== null) {
      current.qualityTotal += quality;
      current.qualityCount += 1;
    }
    grouped.set(value.employee_id, current);
  });
  grouped.forEach((summary, employeeId) => {
    const employee = employees.find((item) => item.id === employeeId);
    if (!employee) return;
    employee.workItems = summary.workItems;
    employee.tasksDone = summary.workItems.length;
    employee.workDays = new Set(summary.workItems.map((item) => item.day)).size;
    employee.materials = summary.materials;
    employee.cost = summary.cost;
    employee.quality = summary.qualityCount ? Math.round(summary.qualityTotal / summary.qualityCount) : 0;
    employee.lastUpdated = '工作事件已读取';
  });
  return events.length;
}

function applyBackendReviews(result: unknown, weekStart: string): void {
  const context = isRecord(result) ? result : {};
  if (!Array.isArray(context.reviews)) return;

  const weekPrefix = `${weekStart}:`;
  Object.keys(state.employeeReviews).forEach((key) => {
    if (key.startsWith(weekPrefix)) delete state.employeeReviews[key];
  });
  Object.keys(state.employeeNotes).forEach((key) => {
    if (key.startsWith(weekPrefix) && !Object.hasOwn(state.employeeNoteDrafts, key)) {
      delete state.employeeNotes[key];
    }
  });

  context.reviews.forEach((value) => {
    if (!isRecord(value) || typeof value.employee_id !== 'string') return;
    const status = value.status;
    if (status !== 'pending' && status !== 'reviewed' && status !== 'needs_supplement') return;
    const reviewWeekStart = typeof value.week_start === 'string' && value.week_start
      ? value.week_start
      : weekStart;
    const key = `${reviewWeekStart}:${value.employee_id}`;
    state.employeeReviews[key] = status;
    if (typeof value.note === 'string' && !Object.hasOwn(state.employeeNoteDrafts, key)) {
      state.employeeNotes[key] = value.note;
    }
  });
  writeStoredRecord('cohmira.employeeReviews', state.employeeReviews);
  writeStoredRecord('cohmira.employeeNotes', state.employeeNotes);
}

async function saveWeekReview(employeeId: string, status: ReviewStatus, note: string): Promise<void> {
  const range = getWeekRange();
  const reviewKey = `${range.startIso}:${employeeId}`;

  // Keep the current text locally as an offline draft until the Rust backend confirms it.
  state.employeeNotes[reviewKey] = note;
  state.employeeNoteDrafts[reviewKey] = note;
  writeStoredRecord('cohmira.employeeNotes', state.employeeNotes);
  writeStoredRecord('cohmira.employeeNoteDrafts', state.employeeNoteDrafts);

  try {
    await callBackendTool<unknown>('boss.save_week_review', {
      owner_code: OWNER_CODE,
      employee_id: employeeId,
      week_start: range.startIso,
      week_end: range.endIso,
      status,
      note
    });
    state.employeeReviews[reviewKey] = status;
    delete state.employeeNoteDrafts[reviewKey];
    writeStoredRecord('cohmira.employeeReviews', state.employeeReviews);
    writeStoredRecord('cohmira.employeeNoteDrafts', state.employeeNoteDrafts);
    showToast(status === 'reviewed' ? '周报已保存为老板已阅' : '补充要求已保存到老板端');
  } catch (error) {
    const message = error instanceof Error ? error.message : '后端不可用';
    showToast(`保存失败，已保留离线草稿：${message}`);
  }
  render();
}

async function refreshWorkspaceData(showFeedback = false): Promise<boolean> {
  const range = getWeekRange();
  const month = localIsoDate().slice(0, 7);
  const requestId = ++workspaceRequestSequence;
  let feedbackMessage = '';
  let coreReady = false;
  state.ownerContextStatus = 'loading';
  state.ownerContextError = '';
  render();
  try {
    const [contextTask, transactionTask, ledgerTask, invoiceTask, actualTask] = await Promise.allSettled([
      callBackendTool<unknown>('boss.owner_context', {
        owner_code: OWNER_CODE,
        date_from: range.startIso,
        date_to: range.endIso,
        month
      }),
      callBackendTool<unknown>('ledger.list_transactions', { owner_code: OWNER_CODE, month, limit: 100, offset: 0 }),
      callBackendTool<unknown>('ledger.report', { owner_code: OWNER_CODE, month }),
      callBackendTool<unknown>('invoice.list_drafts', { owner_code: OWNER_CODE, limit: 50 }),
      callBackendTool<unknown>('actual.integration_status', { owner_code: OWNER_CODE })
    ]);
    if (requestId !== workspaceRequestSequence) return false;
    const errors: string[] = [];
    const failedDomains: string[] = [];
    let eventCount: number | null = null;
    if (contextTask.status === 'fulfilled') {
      eventCount = applyBackendEmployees(contextTask.value);
      applyBackendReviews(contextTask.value, range.startIso);
      state.loadedWeekStart = range.startIso;
      state.loadedWeekEnd = range.endIso;
    } else {
      failedDomains.push('员工');
      errors.push(`员工：${String(contextTask.reason)}`);
    }
    if (transactionTask.status === 'fulfilled') applyTransactionList(transactionTask.value, true);
    else {
      failedDomains.push('流水');
      errors.push(`流水：${String(transactionTask.reason)}`);
    }
    if (ledgerTask.status === 'fulfilled') applyLedgerReport(ledgerTask.value);
    else {
      failedDomains.push('报表');
      errors.push(`报表：${String(ledgerTask.reason)}`);
    }
    if (invoiceTask.status === 'fulfilled') applyInvoiceList(invoiceTask.value);
    else {
      failedDomains.push('发票');
      errors.push(`发票：${String(invoiceTask.reason)}`);
    }
    if (actualTask.status === 'fulfilled') applyActualIntegrationStatus(actualTask.value);
    else {
      failedDomains.push('Actual');
      errors.push(`Actual：${String(actualTask.reason)}`);
    }

    coreReady = contextTask.status === 'fulfilled'
      && transactionTask.status === 'fulfilled'
      && ledgerTask.status === 'fulfilled'
      && invoiceTask.status === 'fulfilled';
    state.ownerContextStatus = errors.length ? (contextTask.status === 'fulfilled' ? 'partial' : 'error') : 'ready';
    state.ownerContextError = errors.join('；');
    if (errors.length && contextTask.status === 'fulfilled') {
      state.dataAsOf = `部分同步 · ${failedDomains.join('、')}读取失败`;
    } else if (contextTask.status === 'fulfilled') {
      state.dataAsOf = eventCount
        ? `员工与账本已同步 · ${new Date().toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}`
        : '账本已同步 · 本周员工事件尚未回传';
    } else {
      const cachedRange = state.loadedWeekStart
        ? `${state.loadedWeekStart} 至 ${state.loadedWeekEnd}`
        : '本地演示数据';
      state.dataAsOf = `员工数据刷新失败 · 保留 ${cachedRange}`;
    }
    if (showFeedback) feedbackMessage = errors.length ? '部分数据刷新失败，已保留可用数据' : '数据已刷新';
  } catch (error) {
    if (requestId !== workspaceRequestSequence) return false;
    state.ownerContextStatus = 'error';
    state.ownerContextError = error instanceof Error ? error.message : String(error);
    const cachedRange = state.loadedWeekStart ? `${state.loadedWeekStart} 至 ${state.loadedWeekEnd}` : '本地演示数据';
    state.dataAsOf = `连接失败 · 保留 ${cachedRange}`;
    if (showFeedback) feedbackMessage = '刷新失败，已保留当前数据';
  } finally {
    if (requestId === workspaceRequestSequence) {
      render();
      if (feedbackMessage) showToast(feedbackMessage);
    }
  }
  return coreReady;
}

async function loadMoreTransactions(): Promise<void> {
  if (state.ledgerLoadingMore || !state.ledgerHasMore) return;
  state.ledgerLoadingMore = true;
  render();
  try {
    const result = await callBackendTool<unknown>('ledger.list_transactions', {
      owner_code: OWNER_CODE,
      month: currentMonthKey(),
      limit: 100,
      offset: state.ledgerNextOffset
    });
    applyTransactionList(result, false);
  } catch (error) {
    showToast(`流水加载失败：${error instanceof Error ? error.message : String(error)}`);
  } finally {
    state.ledgerLoadingMore = false;
    render();
  }
}

async function ensureLedgerFilterRows(filter: LedgerFilter): Promise<void> {
  if (filter === 'all') return;
  const args: Record<string, unknown> = {
    owner_code: OWNER_CODE,
    month: currentMonthKey(),
    limit: 200,
    offset: 0
  };
  if (filter === 'needs_review') args.status = 'needs_review';
  if (filter === 'income' || filter === 'expense') args.type = filter;
  try {
    const result = await callBackendTool<unknown>('ledger.list_transactions', args);
    applyTransactionList(result, false, false);
    render();
  } catch (error) {
    showToast(`筛选流水加载失败：${error instanceof Error ? error.message : String(error)}`);
  }
}

async function refreshActualStatus(showMessage: boolean): Promise<void> {
  state.actualChecking = true;
  render();
  try {
    const result = await callBackendTool<unknown>('actual.integration_status', { owner_code: OWNER_CODE });
    applyActualIntegrationStatus(result);
    if (showMessage) {
      state.messages.push({
        id: `actual-check-${Date.now()}`,
        role: 'assistant',
        content: actualStatus.directSyncReady ? 'Actual 已读取本地配置，可以直连同步。' : 'Actual 仍缺少直连配置，页面已显示缺失项。',
        toolRun: {
          name: 'actual.integration_status',
          label: 'Actual 集成状态',
          summary: JSON.stringify(result)
        }
      });
    }
  } catch (error) {
    actualStatus.directSyncReady = false;
    actualStatus.statusMessage = '本地 Rust 后端不可用';
    actualStatus.lastCheckedAt = new Date().toLocaleString('zh-CN', { hour12: false });
    state.actualBridgeError = error instanceof Error ? error.message : String(error);
    if (showMessage) {
      state.messages.push({
        id: `actual-check-failed-${Date.now()}`,
        role: 'assistant',
        content: `Actual 状态读取失败：${state.actualBridgeError}`,
        toolRun: {
          name: 'actual.integration_status',
          label: 'Actual 集成状态',
          summary: state.actualBridgeError
        }
      });
    }
  } finally {
    state.actualChecking = false;
    render();
    scrollMessagesToBottom();
  }
}

function currentMonthKey(): string {
  return localIsoDate().slice(0, 7);
}

function currentMonthTransactions(): Transaction[] {
  const month = currentMonthKey();
  return state.transactions.filter((transaction) => transaction.date.startsWith(month));
}

function totalIncome(): number {
  if (state.ledgerSummary?.month === currentMonthKey()) return state.ledgerSummary.income;
  return currentMonthTransactions()
    .filter((transaction) => transaction.currency === 'CNY' && transaction.type === 'income' && transaction.status !== 'needs_review')
    .reduce((sum, transaction) => sum + transaction.amount, 0);
}

function totalExpense(): number {
  if (state.ledgerSummary?.month === currentMonthKey()) return state.ledgerSummary.expense;
  return currentMonthTransactions()
    .filter((transaction) => transaction.currency === 'CNY' && transaction.type === 'expense' && transaction.status !== 'needs_review')
    .reduce((sum, transaction) => sum + transaction.amount, 0);
}

function pendingReviewAmount(): number {
  if (state.ledgerSummary?.month === currentMonthKey()) return state.ledgerSummary.pendingAmount;
  return currentMonthTransactions()
    .filter((transaction) => transaction.currency === 'CNY' && transaction.status === 'needs_review')
    .reduce((sum, transaction) => sum + transaction.amount, 0);
}

function teamCompletionRate(): number {
  const done = employees.reduce((sum, employee) => sum + employee.tasksDone, 0);
  const target = employees.reduce((sum, employee) => sum + (employee.weeklyTarget ?? 0), 0);
  return target ? Math.round(done / target * 100) : 0;
}

function currentBossActions(): BossAction[] {
  const actions: BossAction[] = [];
  employees.forEach((employee) => {
    if (employee.status === 'blocked') {
      actions.push({
        id: `employee-blocked:${employee.id}`,
        title: `处理${employee.name}的工作阻塞`,
        description: employee.lastWork || '员工已标记阻塞，等待老板处理。',
        source: `${employee.name} · ${employee.role}`,
        due: '今天',
        severity: 'urgent',
        targetView: 'employees',
        employeeId: employee.id
      });
    } else if (employee.status === 'idle' && employee.workItems.length === 0) {
      actions.push({
        id: `employee-idle:${employee.id}`,
        title: `确认${employee.name}的本周安排`,
        description: '该员工当前空闲，且所选周没有工作事件回传。',
        source: `${employee.name} · ${employee.role}`,
        due: '本周',
        severity: 'normal',
        targetView: 'employees',
        employeeId: employee.id
      });
    }
  });
  currentMonthTransactions()
    .filter((transaction) => transaction.status === 'needs_review')
    .forEach((transaction) => actions.push({
      id: `ledger-review:${transaction.id}`,
      title: `复核${transaction.category}流水`,
      description: `${transaction.counterparty || '交易对方未填写'} · ${currency(transaction.amount, transaction.currency)}`,
      source: `财务记账 · ${transaction.source}`,
      due: '今天',
      severity: 'normal',
      targetView: 'accounting',
      transactionId: transaction.id
    }));
  state.invoiceDrafts
    .filter((draft) => ['extracted', 'needs_review', 'needs_ai_ocr_adapter'].includes(draft.status))
    .forEach((draft) => actions.push({
      id: `invoice-review:${draft.id}`,
      title: `处理票据 ${draft.fileName}`,
      description: invoiceStatusLabel(draft.status),
      source: '发票与凭证',
      due: '本周',
      severity: 'normal',
      targetView: 'accounting'
    }));
  return actions;
}

function openBossActions(): BossAction[] {
  return currentBossActions();
}

function averageQuality(): number {
  const reported = employees.filter((employee) => employee.quality > 0);
  return reported.length ? Math.round(reported.reduce((sum, employee) => sum + employee.quality, 0) / reported.length) : 0;
}

function pendingInvoiceCount(): number {
  return state.invoiceDrafts.filter((draft) => draft.status !== 'posted_to_ledger').length;
}

function pendingSyncCount(): number {
  if (Object.prototype.hasOwnProperty.call(state.actualSyncJobCounts, 'queued')
      || Object.prototype.hasOwnProperty.call(state.actualSyncJobCounts, 'running')) {
    return (state.actualSyncJobCounts.queued ?? 0) + (state.actualSyncJobCounts.running ?? 0);
  }
  return state.actualSyncJobs.filter((job) => job.status === 'queued' || job.status === 'running').length;
}

function needsReviewCount(): number {
  if (state.ledgerSummary?.month === currentMonthKey()) return state.ledgerSummary.needsReview;
  return currentMonthTransactions().filter((transaction) => transaction.currency === 'CNY' && transaction.status === 'needs_review').length;
}

function statusLabel(status: EmployeeStatus): string {
  if (status === 'active') return '进行中';
  if (status === 'blocked') return '待老板';
  return '空闲';
}

function statusClass(status: EmployeeStatus): string {
  if (status === 'active') return 'is-good';
  if (status === 'blocked') return 'is-warn';
  return 'is-muted';
}

function buildOwnerContext() {
  const blocked = employees.filter((employee) => employee.status === 'blocked');
  const active = employees.filter((employee) => employee.status === 'active');
  const materials = employees.reduce((sum, employee) => sum + employee.materials, 0);
  const costs = employees.reduce((sum, employee) => sum + employee.cost, 0);
  return {
    ownerCode: OWNER_CODE,
    employees: employees.map((employee) => ({ ...employee })),
    summary: {
      activeCount: active.length,
      blockedCount: blocked.length,
      taskCount: employees.reduce((sum, employee) => sum + employee.tasksDone, 0),
      materialCount: materials,
      averageQuality: averageQuality(),
      employeeCost: costs,
      ledgerIncome: totalIncome(),
      ledgerExpense: totalExpense(),
      ledgerNet: totalIncome() - totalExpense(),
      needsReview: needsReviewCount(),
      invoiceDrafts: pendingInvoiceCount(),
      actualSyncQueued: pendingSyncCount()
    },
    blockedItems: blocked.map((employee) => ({
      name: employee.name,
      role: employee.role,
      issue: employee.lastWork
    })),
    accountingProvider: {
      name: 'Actual Budget',
      repository: ACTUAL_REPO_URL,
      localPath: actualStatus.localPath,
      apiPackage: actualStatus.apiPackage,
      cliPackage: actualStatus.cliPackage,
      integrationMode: 'Actual CLI/API adapter + unified boss UI',
      status: actualStatus.sourcePresent ? 'source_ready' : 'source_missing',
      directSync: actualStatus.directSyncReady ? 'ready' : 'queued_until_env_configured'
    },
    transactions: currentMonthTransactions().map((transaction) => ({ ...transaction })),
    invoiceDrafts: state.invoiceDrafts.map((draft) => ({ ...draft })),
    actualSyncJobs: state.actualSyncJobs.map((job) => ({ ...job }))
  };
}

function callTool(name: ToolName) {
  if (name === 'boss.owner_context') {
    return {
      name,
      label: '老板全上下文',
      result: buildOwnerContext()
    };
  }
  if (name === 'actual.integration_status') {
    return {
      name,
      label: 'Actual 集成状态',
      result: {
        repository: ACTUAL_REPO_URL,
        ...actualStatus,
        directSyncStatus: actualStatus.directSyncReady ? 'ready' : 'queued_until_actual_env_configured',
        envStatus: {
          serverUrlConfigured: actualStatus.serverUrlConfigured,
          syncIdConfigured: actualStatus.syncIdConfigured,
          credentialConfigured: actualStatus.credentialConfigured,
          defaultAccountIdConfigured: actualStatus.defaultAccountIdConfigured,
          cliAvailable: actualStatus.cliAvailable,
          lastCheckedAt: actualStatus.lastCheckedAt
        },
        mappedAccounts: ['运营现金账户', '应收回款', '运营支出', '员工成本'],
        mappedCategories: ['客户回款', '达人样品', 'AI 工具', '素材采集', '发票待分配']
      }
    };
  }
  if (name === 'invoice.list_drafts') {
    return {
      name,
      label: '发票草稿列表',
      result: {
        drafts: state.invoiceDrafts,
        pending: pendingInvoiceCount()
      }
    };
  }
  if (name === 'actual.export_import_file') {
    return {
      name,
      label: 'Actual 导出文件',
      result: {
        status: 'ready_for_export',
        format: ['csv', 'jsonl'],
        transactionCount: state.transactions.length,
        note: 'Rust 后端会在系统应用数据目录的 actual_exports 下生成 Actual 导入文件。'
      }
    };
  }
  if (name === 'actual.sync_transaction') {
    return {
      name,
      label: 'Actual 同步队列',
      result: state.actualSyncJobs[0] ?? {
        status: 'queued_until_actual_env_configured',
        requiredEnv: actualStatus.requiredEnv
      }
    };
  }
  if (name === 'ledger.list_transactions' || name === 'ledger.report') {
    return {
      name,
      label: '账本查询',
      result: {
        transactions: state.transactions,
        income: totalIncome(),
        expense: totalExpense(),
        net: totalIncome() - totalExpense(),
        needsReview: needsReviewCount()
      }
    };
  }
  return {
    name,
    label: '新增记账草稿',
    result: state.transactions[0] ?? null
  };
}

function inferSummaryMode(prompt: string): SummaryMode {
  if (employees.some((employee) => prompt.includes(employee.name))) return 'team';
  if (/(账|财务|收入|支出|成本|净额|利润|发票|票据|actual)/i.test(prompt)) return 'finance';
  if (/(待办|优先|处理|阻塞|风险|该做|要做)/i.test(prompt)) return 'actions';
  if (/(员工|团队|周报|进度|质量|素材|工作)/i.test(prompt)) return 'team';
  return 'summary';
}

function summarizeOwnerContext(prompt: string, context: ReturnType<typeof buildOwnerContext>, mode: SummaryMode): string {
  const mentionedEmployee = employees.find((employee) => prompt.includes(employee.name));
  const selected = mentionedEmployee ?? employees.find((employee) => employee.id === state.selectedEmployeeId) ?? employees[0];
  const blockers = context.blockedItems.length
    ? context.blockedItems.map((item) => `${item.name}：${item.issue}`).join('；')
    : '暂无需要老板处理的阻塞。';
  const accounting = `账本收入 ${currency(context.summary.ledgerIncome)}，支出 ${currency(context.summary.ledgerExpense)}，净额 ${currency(context.summary.ledgerNet)}，${context.summary.needsReview} 笔待复核，${context.summary.invoiceDrafts} 张发票草稿，${context.summary.actualSyncQueued} 个 Actual 同步任务排队。`;
  const qualitySummary = context.summary.averageQuality ? `平均质量 ${context.summary.averageQuality}` : '质量尚未上报';
  const employeeBrief = `本周 ${context.summary.activeCount} 人进行中，${context.summary.blockedCount} 项待老板处理，已回传 ${context.summary.taskCount} 条工作记录，沉淀 ${context.summary.materialCount} 份素材，${qualitySummary}。`;
  const focus = selected
    ? `当前重点看 ${selected.name}：${selected.lastWork}。`
    : '';
  const source = `数据来源：${state.dataAsOf}。未上报字段不会补写。`;
  if (mentionedEmployee) {
    const records = mentionedEmployee.workItems.length
      ? mentionedEmployee.workItems.slice(0, 5).map((item) => `${item.day} ${item.title}：${item.output}`).join('\n')
      : '所选周没有工作事件回传。';
    const quality = mentionedEmployee.quality ? String(mentionedEmployee.quality) : '未上报';
    return `${mentionedEmployee.name}（${mentionedEmployee.role}）当前状态：${statusLabel(mentionedEmployee.status)}。所选周回传 ${mentionedEmployee.tasksDone} 条，素材 ${mentionedEmployee.materials} 份，质量 ${quality}，记录成本 ${currency(mentionedEmployee.cost)}。\n\n${records}\n\n${source}`;
  }
  if (mode === 'team') return `${employeeBrief}\n\n${blockers} ${focus}\n\n${source}`;
  if (mode === 'finance') return `${accounting}\n\n${source}`;
  if (mode === 'actions') {
    const actions = openBossActions();
    const actionSummary = actions.length
      ? actions.slice(0, 5).map((action, index) => `${index + 1}. ${action.title}：${action.description}`).join('\n')
      : '当前没有需要老板处理的事项。';
    return `${actionSummary}\n\n${source}`;
  }
  return `${employeeBrief}\n\n${blockers} ${focus}\n\n${accounting}\n\n${source}`;
}

function renderLogin(): string {
  const themeTitle = state.themeMode === 'dark' ? '切换到浅色模式' : '切换到深色模式';
  return `
    <main class="login-screen">
      <button class="icon-button login-theme-button" data-toggle-theme type="button" title="${themeTitle}" aria-label="${themeTitle}">${renderIcon(state.themeMode === 'dark' ? 'sun' : 'moon', 18)}</button>
      <section class="login-card">
        <div class="brand-row">
          <img src="/cohmira-mark.svg" alt="商媒运营助手" />
          <div>
            <strong>商媒运营助手</strong>
            <span>老板指挥台 · Cohmira Command</span>
          </div>
        </div>
        <div class="login-copy">
          <span class="eyebrow">老板工作区</span>
          <h1>先看关键进展，再做关键决定</h1>
          <p>员工周报、待办审批与经营账本集中在一个工作台。</p>
        </div>
        <form id="login-form" class="login-form">
          <label>
            <span>老板账号（演示）</span>
            <input name="email" type="email" value="boss@demo.local" autocomplete="username" required />
          </label>
          <label>
            <span>密码</span>
            <input name="password" type="password" value="demo-password" autocomplete="current-password" required />
          </label>
          ${state.loginError ? `<div class="login-error">${escapeHtml(state.loginError)}</div>` : ''}
          <button type="submit">进入工作台 ${renderIcon('arrow-right', 17)}</button>
        </form>
      </section>
      <aside class="login-preview">
        <div class="login-dashboard-preview">
          <div class="login-preview-header">
            <span>今日经营简报</span>
            <strong>${getWeekRange().label}</strong>
          </div>
          <div class="login-preview-metrics">
            <div><span>团队进度</span><strong>${teamCompletionRate()}%</strong></div>
            <div><span>待我处理</span><strong>${openBossActions().length}</strong></div>
            <div><span>本月净额</span><strong>${currency(totalIncome() - totalExpense())}</strong></div>
          </div>
          <div class="login-preview-actions">
            <span>优先处理</span>
            <div>${renderIcon('alert-triangle', 18)}<p><strong>确认新品封面方向</strong><small>周祺 · 今天 18:00</small></p></div>
            <div>${renderIcon('receipt-text', 18)}<p><strong>复核达人样品支出</strong><small>${currency(1880)} · 等待入账</small></p></div>
          </div>
        </div>
      </aside>
    </main>
  `;
}

function renderApp(): string {
  return `
    <div class="app-frame editor-ui-shell">
      ${renderSidebar()}
      <main class="app-main-shell workspace-shell">
        ${renderTopbar()}
        ${renderView()}
      </main>
      ${state.toast ? `<div class="toast" role="status">${renderIcon('check-circle-2', 17)}${escapeHtml(state.toast)}</div>` : ''}
    </div>
  `;
}

function renderSidebar(): string {
  const connectionTone = state.ownerContextStatus === 'error'
    ? 'is-error'
    : state.ownerContextStatus === 'partial'
      ? 'is-warn'
      : '';
  const connectionLabel = state.ownerContextStatus === 'loading'
    ? '正在同步数据'
    : state.ownerContextStatus === 'error'
      ? '本地服务未连接'
      : state.ownerContextStatus === 'partial'
        ? '部分数据未同步'
        : '本地数据服务';
  return `
    <aside class="app-sidebar-shell sidebar-shell">
      <div class="sidebar-brand">
        <img src="/cohmira-mark.svg" alt="商媒运营助手" />
        <div>
          <strong>商媒运营助手</strong>
          <span>老板指挥台 · ${OWNER_CODE}</span>
        </div>
      </div>
      <nav class="sidebar-nav" aria-label="老板端导航">
        <span class="nav-section-label">工作台</span>
        ${renderNavItem('overview', '今日总览', 'layout-dashboard')}
        ${renderNavItem('employees', '员工周报', 'users-round')}
        ${renderNavItem('accounting', '财务记账', 'wallet-cards')}
        ${renderNavItem('assistant', '老板 AI', 'bot')}
        <span class="nav-section-label">团队</span>
        ${renderNavItem('bindings', '员工与绑定', 'user-round-cog')}
      </nav>
      <div class="sidebar-footer">
        <span class="connection-dot ${connectionTone}"></span>
        <div>
          <strong>${connectionLabel}</strong>
          <span title="${escapeHtml(state.ownerContextError)}">${escapeHtml(state.dataAsOf)}</span>
        </div>
      </div>
    </aside>
  `;
}

function renderNavItem(view: View, label: string, icon: string): string {
  const active = state.currentView === view ? 'is-active' : '';
  return `
    <button class="nav-button ${active}" data-view="${view}" type="button">
      <span>${renderIcon(icon, 18)}</span>
      <strong>${label}</strong>
      ${view === 'overview' && openBossActions().length ? `<em>${openBossActions().length}</em>` : ''}
    </button>
  `;
}

function renderTopbar(): string {
  const titles: Record<View, string> = {
    overview: '今日总览',
    employees: '员工周报',
    accounting: '财务记账',
    assistant: '老板 AI',
    bindings: '员工与绑定'
  };
  const themeTitle = state.themeMode === 'dark' ? '切换到浅色模式' : '切换到深色模式';
  return `
    <header class="workspace-topbar">
      <div class="topbar-title">
        <span>商媒运营助手团队</span>
        <strong>${titles[state.currentView]}</strong>
      </div>
      <div class="topbar-actions">
        <button class="icon-button" data-refresh-workspace type="button" title="刷新数据" aria-label="刷新数据" ${state.ownerContextStatus === 'loading' ? 'disabled' : ''}>${renderIcon('refresh-cw', 18)}</button>
        <button class="icon-button" data-toggle-theme type="button" title="${themeTitle}" aria-label="${themeTitle}">${renderIcon(state.themeMode === 'dark' ? 'sun' : 'moon', 18)}</button>
        <button class="icon-button has-indicator" data-view="overview" type="button" title="待办通知" aria-label="待办通知">${renderIcon('bell', 18)}<span>${openBossActions().length}</span></button>
        <button class="icon-button" id="copy-code" type="button" title="复制老板码" aria-label="复制老板码">${renderIcon('copy', 18)}</button>
        <span class="topbar-divider"></span>
        <div class="owner-avatar" title="老板账号">徐</div>
        <button class="icon-button" id="logout" type="button" title="退出" aria-label="退出">${renderIcon('log-out', 18)}</button>
      </div>
    </header>
  `;
}

function renderView(): string {
  if (state.currentView === 'overview') return renderOverview();
  if (state.currentView === 'employees') return renderEmployees();
  if (state.currentView === 'accounting') return renderAccounting();
  if (state.currentView === 'bindings') return renderBindings();
  return renderAssistant();
}

function renderOverview(): string {
  const range = getWeekRange();
  const actions = openBossActions();
  const blockedCount = employees.filter((employee) => employee.status === 'blocked').length;
  const net = totalIncome() - totalExpense();
  const reportedWorkCount = employees.reduce((sum, employee) => sum + employee.tasksDone, 0);
  const goalCount = employees.reduce((sum, employee) => sum + (employee.weeklyTarget ?? 0), 0);
  const hasWeeklyGoals = employees.every((employee) => employee.weeklyTarget !== null);
  const expenseByCategory = new Map<string, number>();
  if (state.ledgerSummary?.month === currentMonthKey()) {
    Object.entries(state.ledgerSummary.expenseByCategory).forEach(([category, amount]) => expenseByCategory.set(category, amount));
  } else {
    currentMonthTransactions()
      .filter((transaction) => transaction.currency === 'CNY' && transaction.type === 'expense' && transaction.status !== 'needs_review')
      .forEach((transaction) => expenseByCategory.set(
        transaction.category,
        (expenseByCategory.get(transaction.category) ?? 0) + transaction.amount
      ));
  }
  const categories = [...expenseByCategory.entries()].sort((a, b) => b[1] - a[1]).slice(0, 4);
  const maxCategory = Math.max(1, ...categories.map(([, amount]) => amount));
  const primaryAction = actions[0];
  const secondaryAction = actions[1];
  return `
    <section class="overview-view">
      <header class="overview-heading">
        <div>
          <span class="eyebrow">${range.relativeLabel} · ${range.label}</span>
          <h1>下午好，徐总</h1>
          <p>当前有 ${actions.length} 件事需要判断，${blockedCount} 名员工存在阻塞。</p>
        </div>
        <div class="overview-heading-actions">
          <span class="data-freshness ${state.ownerContextStatus}" title="${escapeHtml(state.ownerContextError)}">${renderIcon(state.ownerContextStatus === 'error' || state.ownerContextStatus === 'partial' ? 'alert-triangle' : 'database', 15)}${escapeHtml(state.dataAsOf)}</span>
          <button class="primary-button" data-overview-ai type="button">${renderIcon('bot', 17)}生成管理摘要</button>
        </div>
      </header>

      <div class="metric-strip">
        ${hasWeeklyGoals
          ? renderOverviewMetric('本周团队进度', `${teamCompletionRate()}%`, `${reportedWorkCount} / ${goalCount} 项`, 'trending-up', 'positive')
          : renderOverviewMetric('本周工作回传', String(reportedWorkCount), `${employees.filter((employee) => employee.workItems.length > 0).length} 人已回传`, 'file-text', reportedWorkCount ? 'positive' : 'warning')}
        ${renderOverviewMetric('待我处理', String(actions.length), actions.length ? `${actions.filter((item) => item.severity === 'urgent').length} 项优先` : '已全部处理', 'clock-3', actions.length ? 'warning' : 'positive')}
        ${renderOverviewMetric('本月经营净额', currency(net), `收入 ${currency(totalIncome())}`, net >= 0 ? 'trending-up' : 'trending-down', net >= 0 ? 'positive' : 'danger')}
        ${renderOverviewMetric('账务待复核', currency(pendingReviewAmount()), `${needsReviewCount()} 笔流水`, 'receipt-text', needsReviewCount() ? 'warning' : 'positive')}
      </div>

      <div class="overview-layout">
        <div class="overview-main-column">
          <section class="dashboard-section action-section">
            <div class="dashboard-section-header">
              <div><h2>待我处理</h2><span>按影响与截止时间排序</span></div>
              <button class="text-button" data-view="employees" type="button">查看全部 ${renderIcon('arrow-right', 15)}</button>
            </div>
            <div class="action-list">
              ${actions.length ? actions.map(renderBossAction).join('') : `
                <div class="empty-success">${renderIcon('check-circle-2', 22)}<div><strong>当前没有待处理事项</strong><span>新的员工阻塞或账务复核会出现在这里。</span></div></div>
              `}
            </div>
          </section>

          <section class="dashboard-section team-section">
            <div class="dashboard-section-header">
              <div><h2>本周团队进展</h2><span>${employees.length} 名员工 · 平均质量 ${averageQuality() || '未上报'}</span></div>
              <button class="text-button" data-view="employees" type="button">打开周报 ${renderIcon('arrow-right', 15)}</button>
            </div>
            <div class="team-progress-table">
              <div class="team-progress-head"><span>员工</span><span>本周进度</span><span>质量</span><span>状态</span><span>最后回传</span></div>
              ${employees.map(renderTeamProgressRow).join('')}
            </div>
          </section>
        </div>

        <aside class="overview-side-column">
          <section class="dashboard-section finance-snapshot">
            <div class="dashboard-section-header">
              <div><h2>经营快照</h2><span>本月已入账</span></div>
              <button class="icon-button subtle" data-view="accounting" type="button" title="打开财务记账" aria-label="打开财务记账">${renderIcon('arrow-right', 17)}</button>
            </div>
            <div class="cash-summary">
              <span>经营净额</span>
              <strong>${currency(net)}</strong>
              <div><span>收入 ${currency(totalIncome())}</span><span>支出 ${currency(totalExpense())}</span></div>
            </div>
            <div class="category-bars">
              ${categories.length ? categories.map(([category, amount]) => `
                <div class="category-bar">
                  <div><span>${escapeHtml(category)}</span><strong>${currency(amount)}</strong></div>
                  <span class="bar-track"><i style="width:${Math.max(8, Math.round(amount / maxCategory * 100))}%"></i></span>
                </div>
              `).join('') : '<div class="empty-compact">暂无已入账支出</div>'}
            </div>
          </section>

          <section class="dashboard-section ai-briefing">
            <div class="briefing-mark">${renderIcon('bot', 18)}</div>
            <div>
              <span>AI 管理提示</span>
              <p>${primaryAction
                ? `<strong>先处理“${escapeHtml(primaryAction.title)}”。</strong>${escapeHtml(primaryAction.description)}${secondaryAction ? ` 随后处理“${escapeHtml(secondaryAction.title)}”。` : ''}`
                : '<strong>当前待办已清空。</strong>可以继续查看员工周报和本月经营数据。'}</p>
            </div>
            <button class="text-button" data-overview-ai type="button">继续追问 ${renderIcon('arrow-right', 15)}</button>
          </section>
        </aside>
      </div>
    </section>
  `;
}

function renderOverviewMetric(label: string, value: string, meta: string, icon: string, tone: string): string {
  return `
    <article class="overview-metric ${tone}">
      <div class="metric-icon">${renderIcon(icon, 19)}</div>
      <span>${escapeHtml(label)}</span>
      <strong>${escapeHtml(value)}</strong>
      <small>${escapeHtml(meta)}</small>
    </article>
  `;
}

function renderBossAction(action: BossAction): string {
  const urgent = action.severity === 'urgent';
  return `
    <article class="action-row ${urgent ? 'urgent' : ''}">
      <div class="action-kind">${renderIcon(urgent ? 'alert-triangle' : action.targetView === 'accounting' ? 'receipt-text' : 'clock-3', 18)}</div>
      <div class="action-copy">
        <div><strong>${escapeHtml(action.title)}</strong>${urgent ? '<span class="priority-label">优先</span>' : ''}</div>
        <p>${escapeHtml(action.description)}</p>
        <small>${escapeHtml(action.source)} · ${escapeHtml(action.due)}</small>
      </div>
      <div class="action-buttons">
        <button class="icon-button" data-open-action="${escapeHtml(action.id)}" type="button" title="打开详情" aria-label="打开详情">${renderIcon('arrow-right', 17)}</button>
      </div>
    </article>
  `;
}

function renderTeamProgressRow(employee: Employee): string {
  const rate = completionRate(employee);
  return `
    <button class="team-progress-row" data-open-employee="${escapeHtml(employee.id)}" type="button">
      <span class="employee-identity"><i>${escapeHtml(employee.name.slice(-1))}</i><span><strong>${escapeHtml(employee.name)}</strong><small>${escapeHtml(employee.role)}</small></span></span>
      ${employee.weeklyTarget === null
        ? `<span class="progress-cell reported"><span><i style="width:${employee.tasksDone ? 100 : 0}%"></i></span><strong>${employee.tasksDone} 条</strong></span>`
        : `<span class="progress-cell"><span><i style="width:${rate}%"></i></span><strong>${rate}%</strong></span>`}
      <span class="quality-cell"><strong>${employee.quality || '--'}</strong><small class="${employee.weeklyTarget === null ? 'is-muted' : employee.trend >= 0 ? 'is-good' : 'is-danger'}">${employee.weeklyTarget === null ? '本周' : `${employee.trend >= 0 ? '+' : ''}${employee.trend}`}</small></span>
      <span><i class="status-pill ${statusClass(employee.status)}">${statusLabel(employee.status)}</i></span>
      <span class="last-sync">${escapeHtml(employee.lastUpdated)}</span>
    </button>
  `;
}

function renderAssistant(): string {
  const context = buildOwnerContext();
  return `
    <section class="boss-ai-view">
      <div class="chat-column">
        <section class="chat-surface">
          <div class="assistant-welcome">
            <img src="/cohmira-mark.svg" alt="Cohmira" />
            <div>
              <h1>老板 AI</h1>
              <p>${escapeHtml(state.dataAsOf)} · 回答附带数据来源</p>
            </div>
          </div>
          <div class="shortcut-row">
            ${renderShortcut('本周团队进度怎么样？', 'team')}
            ${renderShortcut('哪些事情需要我现在处理？', 'actions')}
            ${renderShortcut('本月现金流和待复核账务', 'finance')}
            ${renderShortcut('生成今天的管理摘要', 'summary')}
          </div>
          <div class="message-list" id="message-list">
            ${state.messages.map(renderMessage).join('')}
          </div>
          <form id="chat-form" class="composer-shell">
            <textarea name="prompt" rows="1" placeholder="查询团队、待办或财务摘要" aria-label="向老板 AI 提问"></textarea>
            <div class="composer-actions">
              <div class="composer-left-actions">
                <input id="chat-attachment-input" type="file" multiple accept=".txt,.csv,.json,.md,.pdf,.png,.jpg,.jpeg,.webp" />
                <label class="composer-attach-button" for="chat-attachment-input" title="上传文件、图片或发票" aria-label="上传文件、图片或发票">${renderIcon('paperclip', 17)}</label>
                <button class="composer-tool" type="button" data-run-tool="boss.owner_context">${renderIcon('database', 14)}刷新上下文</button>
              </div>
              <button class="send-button" type="submit" aria-label="发送">${renderIcon('send', 17)}</button>
            </div>
          </form>
        </section>
      </div>
      <aside class="context-rail">
        <section class="context-card">
          <div class="section-title">
            <h2>当前上下文</h2>
            <span>${context.summary.blockedCount} 待处理</span>
          </div>
          <div class="signal-grid">
            ${renderMiniMetric('员工动作', String(context.summary.taskCount))}
            ${renderMiniMetric('平均质量', context.summary.averageQuality ? String(context.summary.averageQuality) : '未上报')}
            ${renderMiniMetric('素材入库', String(context.summary.materialCount))}
            ${renderMiniMetric('本期净额', currency(context.summary.ledgerNet))}
            ${renderMiniMetric('发票草稿', String(context.summary.invoiceDrafts))}
            ${renderMiniMetric('Actual 排队', String(context.summary.actualSyncQueued))}
          </div>
        </section>
        <section class="context-card">
          <div class="section-title">
            <h2>数据来源</h2>
            <span>可追溯</span>
          </div>
          <div class="tool-trace">
            <strong>boss.owner_context</strong>
            <span>员工周报、阻塞、账本和发票</span>
          </div>
          <div class="tool-trace">
            <strong>invoice.upload_and_extract</strong>
            <span>票据字段识别与入账草稿</span>
          </div>
          <div class="tool-trace">
            <strong>actual.integration_status</strong>
            <span>${actualStatus.directSyncReady ? '账本同步已就绪' : '本地账本可用，直连待配置'}</span>
          </div>
        </section>
      </aside>
    </section>
  `;
}

function renderShortcut(text: string, mode: SummaryMode): string {
  return `<button class="shortcut-chip" data-prompt="${escapeHtml(text)}" data-summary-mode="${mode}" type="button">${escapeHtml(text)}</button>`;
}

function renderMessage(message: ChatMessage): string {
  return `
    <article class="message-row ${message.role === 'owner' ? 'is-owner' : 'is-ai'}">
      <div class="message-avatar">${message.role === 'owner' ? '徐' : renderIcon('bot', 16)}</div>
      <div class="message-body">
        <p>${escapeHtml(message.content)}</p>
        ${message.toolRun ? `
          <details class="tool-call-box">
            <summary><strong>${message.toolRun.name}</strong><span>查看工具返回</span></summary>
            <pre>${escapeHtml(message.toolRun.summary)}</pre>
          </details>
        ` : ''}
      </div>
    </article>
  `;
}

function renderMiniMetric(label: string, value: string): string {
  return `
    <div class="mini-metric">
      <span>${label}</span>
      <strong>${value}</strong>
    </div>
  `;
}

function renderEmployees(): string {
  const range = getWeekRange();
  const filteredEmployees = employees.filter((employee) => {
    const query = state.employeeSearch.trim().toLocaleLowerCase('zh-CN');
    const matchesSearch = !query || `${employee.name} ${employee.role}`.toLocaleLowerCase('zh-CN').includes(query);
    if (state.employeeFilter === 'attention') return matchesSearch && (employee.status === 'blocked' || employee.quality < 85);
    if (state.employeeFilter === 'reviewed') return matchesSearch && employeeReviewStatus(employee.id) === 'reviewed';
    return matchesSearch;
  });
  return `
    <section class="secondary-view employee-weekly-view">
      <div class="view-heading weekly-view-heading">
        <div>
          <h1>员工周报</h1>
          <p>${employees.length} 名员工 · ${employees.filter((item) => item.status === 'blocked').length} 项阻塞 · ${escapeHtml(state.dataAsOf)}</p>
        </div>
        <div class="view-actions">
          <div class="week-picker" aria-label="选择周">
            <button class="icon-button subtle" data-week-shift="-1" type="button" title="上一周" aria-label="上一周">${renderIcon('chevron-left', 17)}</button>
            <div>${renderIcon('calendar-days', 16)}<span><strong>${range.relativeLabel}</strong><small>${range.label}</small></span></div>
            <button class="icon-button subtle" data-week-shift="1" type="button" title="下一周" aria-label="下一周" ${state.weekOffset >= 0 ? 'disabled' : ''}>${renderIcon('chevron-right', 17)}</button>
          </div>
        </div>
      </div>
      <div class="employee-toolbar">
        <div class="segmented-control" aria-label="周报筛选">
          ${renderEmployeeFilter('all', '全部', employees.length)}
          ${renderEmployeeFilter('attention', '需关注', employees.filter((employee) => employee.status === 'blocked' || employee.quality < 85).length)}
          ${renderEmployeeFilter('reviewed', '已阅', employees.filter((employee) => employeeReviewStatus(employee.id) === 'reviewed').length)}
        </div>
        <label class="search-control">${renderIcon('search', 16)}<input id="employee-search" type="search" value="${escapeHtml(state.employeeSearch)}" placeholder="搜索员工或岗位" aria-label="搜索员工或岗位" /></label>
      </div>
      <div class="employee-board">
        <div class="employee-list">
          ${filteredEmployees.length ? filteredEmployees.map(renderEmployeeButton).join('') : '<div class="empty-compact">当前筛选下没有员工周报</div>'}
        </div>
        <section class="employee-detail">
          ${renderSelectedEmployee()}
        </section>
      </div>
    </section>
  `;
}

function renderEmployeeFilter(filter: EmployeeFilter, label: string, count: number): string {
  return `<button class="${state.employeeFilter === filter ? 'is-active' : ''}" data-employee-filter="${filter}" type="button">${label}<span>${count}</span></button>`;
}

function renderEmployeeButton(employee: Employee): string {
  const selected = employee.id === state.selectedEmployeeId ? 'is-selected' : '';
  const review = employeeReviewStatus(employee.id);
  const rate = completionRate(employee);
  return `
    <button class="employee-button ${selected}" data-employee-id="${escapeHtml(employee.id)}" type="button">
      <span class="employee-avatar">${escapeHtml(employee.name.slice(-1))}<i class="status-dot ${statusClass(employee.status)}"></i></span>
      <div class="employee-button-main">
        <span><strong>${escapeHtml(employee.name)}</strong><em>${employee.status === 'blocked' ? '需关注' : review === 'reviewed' ? '已阅' : ''}</em></span>
        <small>${escapeHtml(employee.role)}</small>
        <span class="employee-progress"><i><b style="width:${employee.weeklyTarget === null ? (employee.tasksDone ? 100 : 0) : rate}%"></b></i><small>${employee.weeklyTarget === null ? `${employee.tasksDone} 条记录` : `${employee.tasksDone}/${employee.weeklyTarget}`}</small></span>
      </div>
      ${renderIcon('chevron-right', 16)}
    </button>
  `;
}

function renderSelectedEmployee(): string {
  const employee = employees.find((item) => item.id === state.selectedEmployeeId) ?? employees[0];
  if (!employee) return '';
  const review = employeeReviewStatus(employee.id) ?? 'pending';
  return `
    <div class="detail-header">
      <div class="employee-detail-identity">
        <span class="employee-avatar large">${escapeHtml(employee.name.slice(-1))}<i class="status-dot ${statusClass(employee.status)}"></i></span>
        <div>
          <h2>${escapeHtml(employee.name)}</h2>
          <p>${escapeHtml(employee.role)} · ${escapeHtml(employee.lastUpdated)}</p>
        </div>
      </div>
      <div class="detail-statuses">
        <span class="review-pill ${review}">${review === 'reviewed' ? '老板已阅' : review === 'needs_supplement' ? '已记录补充要求' : '待审阅'}</span>
        <span class="status-pill ${statusClass(employee.status)}">当前 · ${statusLabel(employee.status)}</span>
      </div>
    </div>
    <div class="weekly-detail-metrics">
      ${employee.weeklyTarget === null
        ? renderWeeklyMetric('工作回传', `${employee.tasksDone} 条`, employee.tasksDone ? '员工端事件' : '本周未上报')
        : renderWeeklyMetric('完成进度', `${completionRate(employee)}%`, `${employee.tasksDone}/${employee.weeklyTarget} 项`)}
      ${renderWeeklyMetric('质量评分', employee.quality ? String(employee.quality) : '--', employee.weeklyTarget === null ? '本周口径' : `${employee.trend >= 0 ? '+' : ''}${employee.trend} 较上周`)}
      ${renderWeeklyMetric('素材产出', String(employee.materials), `${employee.workDays} 个工作日`)}
      ${renderWeeklyMetric('本周成本', currency(employee.cost), '已记录')}
    </div>
    <section class="weekly-focus">
      <span>本周工作重点</span>
      <p>${escapeHtml(employee.weeklyTarget === null ? '员工端尚未提交结构化本周目标。' : employee.weeklyFocus)}</p>
    </section>
    <section class="work-timeline-section">
      <div class="detail-section-heading"><h3>工作记录</h3><span>${employee.workItems.length} 条可核验记录</span></div>
      <div class="work-timeline">
        ${employee.workItems.length ? employee.workItems.map((item) => `
          <article class="work-item ${item.status}">
            <span class="timeline-marker">${item.status === 'done' ? renderIcon('check', 14) : item.status === 'blocked' ? renderIcon('alert-triangle', 14) : renderIcon('clock-3', 14)}</span>
            <span class="work-day">${escapeHtml(item.day)}</span>
            <div><strong>${escapeHtml(item.title)}</strong><p>${escapeHtml(item.output)}</p></div>
            <em>${workStatusLabel(item.status)}</em>
          </article>
        `).join('') : '<div class="empty-compact">该周尚未收到员工端工作事件</div>'}
      </div>
    </section>
    <div class="employee-detail-lower">
      <section>
        <div class="detail-section-heading"><h3>管理信号</h3></div>
        <div class="signal-list compact">
          ${employee.weeklyTarget === null
            ? '<div><span>员工端尚未回传结构化管理信号</span></div>'
            : employee.signals.map((signal) => `<div>${renderIcon(signal.includes('缺') || signal.includes('需要') ? 'alert-triangle' : 'check-circle-2', 15)}<span>${escapeHtml(signal)}</span></div>`).join('')}
        </div>
      </section>
      <section>
        <div class="detail-section-heading"><h3>下周计划</h3></div>
        <p class="next-plan">${escapeHtml(employee.weeklyTarget === null ? '员工端尚未提交下周计划。' : employee.nextPlan)}</p>
      </section>
    </div>
    <form class="review-composer" data-review-form="${escapeHtml(employee.id)}">
      <label for="employee-note">老板反馈</label>
      <textarea id="employee-note" name="note" rows="2" placeholder="填写方向、补充要求或确认意见">${escapeHtml(state.employeeNotes[employeeReviewKey(employee.id)] ?? '')}</textarea>
      <div>
        <button class="secondary-button" data-review-employee="${escapeHtml(employee.id)}" data-review-status="needs_supplement" type="button">${renderIcon('message-square-text', 16)}记录补充要求</button>
        <button class="primary-button" data-review-employee="${escapeHtml(employee.id)}" data-review-status="reviewed" type="button">${renderIcon('check', 16)}标记已阅</button>
      </div>
    </form>
  `;
}

function renderWeeklyMetric(label: string, value: string, meta: string): string {
  return `<div><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong><small>${escapeHtml(meta)}</small></div>`;
}

function renderAccounting(): string {
  const monthTransactions = currentMonthTransactions();
  const filteredTransactions = monthTransactions.filter((transaction) => {
    if (state.ledgerFilter === 'needs_review') return transaction.status === 'needs_review';
    if (state.ledgerFilter === 'income') return transaction.type === 'income';
    if (state.ledgerFilter === 'expense') return transaction.type === 'expense';
    return true;
  });
  const summary = state.ledgerSummary?.month === currentMonthKey() ? state.ledgerSummary : null;
  const incomeCount = summary?.incomeCount ?? monthTransactions.filter((item) => item.currency === 'CNY' && item.type === 'income' && item.status !== 'needs_review').length;
  const expenseCount = summary?.expenseCount ?? monthTransactions.filter((item) => item.currency === 'CNY' && item.type === 'expense' && item.status !== 'needs_review').length;
  const transactionCount = summary?.transactionCount ?? monthTransactions.length;
  const monthLabel = new Intl.DateTimeFormat('zh-CN', { year: 'numeric', month: 'long' }).format(new Date());
  return `
    <section class="secondary-view accounting-view">
      <div class="view-heading">
        <div>
          <h1>财务记账</h1>
          <p>${monthLabel} · 已入账与待复核分开统计 · ${escapeHtml(state.dataAsOf)}</p>
        </div>
        <div class="view-actions">
          <button class="secondary-button" data-toggle-finance-settings type="button">${renderIcon('database', 16)}账本设置</button>
          <button class="primary-button" data-focus-entry type="button">${renderIcon('plus', 16)}新增流水</button>
        </div>
      </div>
      <div class="finance-metric-strip">
        ${renderFinanceMetric('经营净额', currency(totalIncome() - totalExpense()), '已入账收入减支出', 'circle-dollar-sign', 'primary')}
        ${renderFinanceMetric('本月收入', currency(totalIncome()), `${incomeCount} 笔已入账`, 'trending-up', 'positive')}
        ${renderFinanceMetric('本月支出', currency(totalExpense()), `${expenseCount} 笔已入账`, 'trending-down', 'neutral')}
        ${renderFinanceMetric('待复核', currency(pendingReviewAmount()), `${needsReviewCount()} 笔不计入净额`, 'alert-triangle', needsReviewCount() ? 'warning' : 'positive')}
      </div>

      <div class="finance-workspace">
        <section class="finance-panel transaction-card">
          <div class="section-title">
            <div><h2>账本流水</h2><span>本月 ${transactionCount} 笔 · 当前加载 ${monthTransactions.length} 笔</span></div>
            <div class="segmented-control compact" aria-label="流水筛选">
              ${renderLedgerFilter('all', '全部')}
              ${renderLedgerFilter('needs_review', `待复核 ${needsReviewCount()}`)}
              ${renderLedgerFilter('income', '收入')}
              ${renderLedgerFilter('expense', '支出')}
            </div>
          </div>
          <div class="transaction-list">
            <div class="transaction-head"><span>日期</span><span>分类 / 对方</span><span>来源</span><span>金额</span><span>状态</span><span></span></div>
            ${filteredTransactions.length ? filteredTransactions.map(renderTransaction).join('') : '<div class="empty-compact">当前已加载范围内没有匹配流水</div>'}
            ${state.ledgerHasMore ? `<div class="ledger-load-more"><button class="secondary-button" data-load-more-transactions type="button" ${state.ledgerLoadingMore ? 'disabled' : ''}>${renderIcon('refresh-cw', 15)}${state.ledgerLoadingMore ? '加载中' : `加载更多（已加载 ${state.ledgerNextOffset}/${state.ledgerTotal}）`}</button></div>` : ''}
          </div>
        </section>

        <aside class="finance-entry-rail">
          <section class="finance-panel manual-entry-card" id="manual-entry-card">
            <div class="section-title"><div><h2>新增流水</h2><span>保存后进入待复核</span></div>${renderIcon('plus', 17)}</div>
            <form id="transaction-form" class="ledger-form">
              <div class="form-row">
                <label><span>收支类型</span><select name="type"><option value="expense">支出</option><option value="income">收入</option></select></label>
                <label><span>发生日期</span><input name="date" type="date" value="${localIsoDate()}" /></label>
              </div>
              <label><span>分类</span><input name="category" value="运营成本" required /></label>
              <label><span>交易对方</span><input name="counterparty" placeholder="供应商或客户名称" required /></label>
              <label><span>金额（元）</span><input name="amount" type="number" min="0.01" step="0.01" placeholder="0.00" required /></label>
              <button class="primary-button" type="submit">保存待复核流水</button>
            </form>
          </section>
          <section class="finance-panel invoice-upload-card">
            <div class="section-title"><div><h2>票据识别</h2><span>${escapeHtml(state.uploadStatus)}</span></div>${renderIcon('receipt-text', 17)}</div>
            <label class="upload-zone" for="invoice-upload">
              <input id="invoice-upload" type="file" accept=".txt,.csv,.json,.md,.pdf,.png,.jpg,.jpeg,.webp" />
              <span class="upload-icon">${renderIcon('upload', 20)}</span>
              <strong>上传发票、收据或截图</strong>
              <span>识别结果先进入草稿，不会直接入账</span>
            </label>
          </section>
        </aside>

        <section class="finance-panel invoice-draft-card">
          <div class="section-title">
            <div><h2>发票与凭证</h2><span>${pendingInvoiceCount()} 项待处理</span></div>
            <span>${state.invoiceDrafts.length} 份文件</span>
          </div>
          <div class="invoice-draft-list">
            ${state.invoiceDrafts.length ? state.invoiceDrafts.map(renderInvoiceDraft).join('') : '<div class="empty-compact">还没有发票草稿</div>'}
          </div>
        </section>

        <section class="finance-panel integration-panel ${state.showFinanceSettings ? 'is-open' : ''}">
          <button class="integration-toggle" data-toggle-finance-settings type="button">
            <div>${renderIcon('database', 18)}<span><strong>Actual Budget 同步设置</strong><small>${actualStatus.directSyncReady ? '直连同步已就绪' : '本地账本可用，直连配置不完整'}</small></span></div>
            ${renderIcon(state.showFinanceSettings ? 'chevron-left' : 'chevron-right', 17)}
          </button>
          <div class="integration-content">
            <div class="actual-status-grid">
              ${renderActualItem('本地源码', actualStatus.sourcePresent ? '已接入' : '未找到')}
              ${renderActualItem('CLI', actualStatus.cliAvailable ? '可用' : '不可用')}
              ${renderActualItem('Server URL', actualStatus.serverUrlConfigured ? '已配置' : '缺失')}
              ${renderActualItem('Sync ID', actualStatus.syncIdConfigured ? '已配置' : '缺失')}
              ${renderActualItem('凭据', actualStatus.credentialConfigured ? '已配置' : '缺失')}
              ${renderActualItem('默认账户', actualStatus.defaultAccountIdConfigured ? '已配置' : '缺失')}
            </div>
            ${state.actualBridgeError ? `<div class="inline-error">${escapeHtml(state.actualBridgeError)}</div>` : ''}
            <div class="integration-actions">
              <button class="secondary-button" data-run-tool="actual.integration_status" type="button">${renderIcon('refresh-cw', 16)}${state.actualChecking ? '检查中' : '检查连接'}</button>
              <button class="secondary-button" data-export-actual type="button">${renderIcon('file-text', 16)}导出 Actual 文件</button>
            </div>
            <div class="sync-job-list">
              ${state.actualSyncJobs.length ? state.actualSyncJobs.map(renderSyncJob).join('') : '<div class="empty-compact">当前没有同步任务</div>'}
            </div>
          </div>
        </section>
      </div>
    </section>
  `;
}

function renderFinanceMetric(label: string, value: string, meta: string, icon: string, tone: string): string {
  return `<article class="finance-metric ${tone}"><span class="metric-icon">${renderIcon(icon, 18)}</span><div><span>${escapeHtml(label)}</span><strong>${escapeHtml(value)}</strong><small>${escapeHtml(meta)}</small></div></article>`;
}

function renderLedgerFilter(filter: LedgerFilter, label: string): string {
  return `<button class="${state.ledgerFilter === filter ? 'is-active' : ''}" data-ledger-filter="${filter}" type="button">${escapeHtml(label)}</button>`;
}

function renderActualItem(label: string, value: string): string {
  return `
    <div class="actual-status-item">
      <span>${escapeHtml(label)}</span>
      <strong>${escapeHtml(value)}</strong>
    </div>
  `;
}

function invoiceStatusLabel(status: InvoiceDraft['status']): string {
  if (status === 'extracted') return '已识别';
  if (status === 'ledger_draft') return '已生成草稿';
  if (status === 'posted_to_ledger') return '已入账';
  if (status === 'needs_ai_ocr_adapter') return '待 OCR';
  if (status === 'needs_file') return '待上传';
  if (status === 'file_not_found') return '文件缺失';
  return '待复核';
}

function renderInvoiceDraft(draft: InvoiceDraft): string {
  const total = draft.fields.total ?? draft.fields.amount;
  const confidence = Math.round((draft.fields.confidence || 0) * 100);
  const persisted = Boolean(draft.backendId);
  const posting = invoicePostingIds.has(draft.id);
  const actionable = persisted && Boolean(total) && !posting && !['posted_to_ledger', 'ledger_draft'].includes(draft.status);
  const warningStatus = !persisted || ['needs_review', 'needs_ai_ocr_adapter', 'needs_file', 'file_not_found'].includes(draft.status);
  const statusText = persisted ? invoiceStatusLabel(draft.status) : '本地预览 · 未保存';
  const buttonText = posting
    ? '生成中'
    : !persisted
    ? '未保存'
    : draft.status === 'posted_to_ledger'
      ? renderIcon('check', 15) + '已入账'
      : draft.status === 'ledger_draft'
        ? '已生成流水'
        : '生成流水';
  return `
    <article class="invoice-draft-row">
      <span class="invoice-icon">${renderIcon('file-text', 17)}</span>
      <div class="invoice-main">
        <strong>${escapeHtml(draft.fileName)}</strong>
        <span>${escapeHtml(draft.fields.seller || '销售方待识别')} · ${escapeHtml(draft.fields.invoiceNumber || '发票号待识别')}</span>
      </div>
      <div class="invoice-fields">
        <span>${escapeHtml(draft.fields.invoiceDate || '日期待识别')}</span>
        <span>${total ? currency(total, draft.fields.currency) : '金额待识别'}</span>
        <span>置信度 ${confidence}%</span>
      </div>
      <span class="status-pill ${warningStatus ? 'is-warn' : 'is-good'}">${statusText}</span>
      <button class="secondary-button small" data-post-invoice="${escapeHtml(draft.id)}" type="button" ${actionable ? '' : 'disabled'}>${buttonText}</button>
    </article>
  `;
}

function renderTransaction(transaction: Transaction): string {
  const statusLabel =
    transaction.status === 'synced'
      ? '已同步'
      : transaction.status === 'posted'
        ? '已入账'
        : '待复核';
  const statusClass =
    transaction.status === 'synced'
      ? 'is-good'
      : transaction.status === 'posted'
        ? 'is-good'
        : 'is-warn';
  return `
    <article class="transaction-row">
      <span>${escapeHtml(transaction.date)}</span>
      <span class="transaction-main"><strong>${escapeHtml(transaction.category)}</strong><small>${escapeHtml(transaction.counterparty)}</small></span>
      <span class="transaction-source">${escapeHtml(transaction.source)}</span>
      <em class="${transaction.type === 'income' ? 'is-good' : 'is-danger'}">${transaction.type === 'income' ? '+' : '-'}${currency(transaction.amount, transaction.currency)}</em>
      <span class="status-pill ${statusClass}">${statusLabel}</span>
      <span class="transaction-actions">
        ${transaction.status === 'needs_review'
          ? `<button class="secondary-button small" data-approve-transaction="${escapeHtml(transaction.id)}" type="button">${renderIcon('check', 14)}确认入账</button>`
          : transaction.status === 'synced'
            ? `<span class="synced-label">${renderIcon('check-circle-2', 15)}Actual</span>`
            : `<button class="icon-button subtle" data-sync-transaction="${escapeHtml(transaction.id)}" type="button" title="同步 Actual" aria-label="同步 Actual">${renderIcon('refresh-cw', 16)}</button>`}
      </span>
    </article>
  `;
}

function renderSyncJob(job: ActualSyncJob): string {
  return `
    <article class="sync-job-row">
      <strong>${escapeHtml(job.transactionId)}</strong>
      <span>${escapeHtml(job.reason)}</span>
      <em>${escapeHtml(job.status)}</em>
    </article>
  `;
}

function renderBindings(): string {
  const bindingStatusClass = state.ownerContextStatus === 'error'
    ? 'is-error'
    : state.ownerContextStatus === 'partial'
      ? 'is-warn'
      : '';
  const bindingStatus = state.ownerContextStatus === 'error'
    ? `${renderIcon('alert-triangle', 15)}本地服务未连接`
    : state.ownerContextStatus === 'partial'
      ? `${renderIcon('alert-triangle', 15)}部分数据异常`
      : `${renderIcon('check-circle-2', 15)}本地结构已就绪`;
  return `
    <section class="secondary-view bindings-view">
      <div class="view-heading">
        <div>
          <h1>员工与绑定</h1>
          <p>${employees.length} 名员工已归属当前老板空间 · ${escapeHtml(state.dataAsOf)}</p>
        </div>
        <button class="secondary-button" data-refresh-workspace type="button">${renderIcon('refresh-cw', 16)}刷新状态</button>
      </div>
      <div class="binding-layout">
        <section class="binding-code-card">
          <span class="binding-icon">${renderIcon('user-round-cog', 20)}</span>
          <span>员工端绑定码</span>
          <strong>${OWNER_CODE}</strong>
          <p>接入员工端同步 API 后，可用此码归属工作事件、任务产物与成本摘要。</p>
          <button class="primary-button" id="copy-code-secondary" type="button">${renderIcon('copy', 16)}复制老板码</button>
        </section>
        <section class="binding-employees-panel">
          <div class="section-title">
            <div><h2>当前员工记录</h2><span>${employees.length} 个员工档案</span></div>
            <span class="connection-status ${bindingStatusClass}">${bindingStatus}</span>
          </div>
          <div class="bound-employee-list">
            ${employees.map((employee) => `
              <article>
                <span class="employee-avatar">${escapeHtml(employee.name.slice(-1))}<i class="status-dot ${statusClass(employee.status)}"></i></span>
                <div><strong>${escapeHtml(employee.name)}</strong><span>${escapeHtml(employee.role)}</span></div>
                <span><strong>${escapeHtml(employee.lastUpdated)}</strong><small>最近记录</small></span>
                <button class="icon-button subtle" data-open-employee="${escapeHtml(employee.id)}" type="button" title="打开员工周报" aria-label="打开员工周报">${renderIcon('arrow-right', 16)}</button>
              </article>
            `).join('')}
          </div>
        </section>
      </div>
      <section class="data-boundary-panel">
        <div>${renderIcon('database', 18)}<span><strong>同步数据范围</strong><small>工作摘要、任务状态、产物引用、质量结果与成本</small></span></div>
        <div>${renderIcon('check-circle-2', 18)}<span><strong>敏感信息留在员工端</strong><small>账号凭据、Cookie、API Key 与完整聊天内容不会同步</small></span></div>
      </section>
    </section>
  `;
}

async function sendPrompt(prompt: string, mode?: SummaryMode): Promise<void> {
  const timestamp = Date.now();
  const assistantId = `assistant-${timestamp}`;
  state.messages.push({ id: `owner-${timestamp}`, role: 'owner', content: prompt });
  state.messages.push({ id: assistantId, role: 'assistant', content: '正在读取本周员工、账本与发票上下文…' });
  render();
  scrollMessagesToBottom();

  const range = getWeekRange();
  const expectedRequestId = workspaceRequestSequence + 1;
  const contextReady = await refreshWorkspaceData(false);
  const rangeChanged = range.startIso !== getWeekRange().startIso;
  const superseded = workspaceRequestSequence !== expectedRequestId;
  if (!contextReady || rangeChanged || superseded) {
    const toolSummary = rangeChanged || superseded
      ? '工作区数据在读取期间发生变化'
      : state.ownerContextError || '老板上下文读取失败';
    const failedMessage = state.messages.find((item) => item.id === assistantId);
    if (failedMessage) {
      failedMessage.content = rangeChanged || superseded
        ? '数据范围在读取期间发生变化，请基于当前页面重新提问。'
        : '本次未能完整读取员工、账本与发票上下文，因此没有生成管理结论。请检查本地数据服务后重试。';
      failedMessage.toolRun = {
        name: 'boss.owner_context',
        label: '老板全上下文',
        summary: toolSummary
      };
    }
    render();
    scrollMessagesToBottom();
    return;
  }

  const context = buildOwnerContext();
  const resolvedMode = mode ?? inferSummaryMode(prompt);
  const toolSummary = `${range.startIso} 至 ${range.endIso} · ${context.summary.taskCount} 条员工事件 · ${context.summary.needsReview} 笔待复核`;
  const message = state.messages.find((item) => item.id === assistantId);
  if (message) {
    message.content = summarizeOwnerContext(prompt, context, resolvedMode);
    message.toolRun = {
      name: 'boss.owner_context',
      label: '老板全上下文',
      summary: toolSummary || `${context.summary.taskCount} 个员工动作，${context.summary.needsReview} 笔账务待复核`
    };
  }
  render();
  scrollMessagesToBottom();
}

function scrollMessagesToBottom(): void {
  window.requestAnimationFrame(() => {
    const messageList = document.querySelector<HTMLDivElement>('#message-list');
    if (messageList) {
      messageList.scrollTop = messageList.scrollHeight;
    }
  });
}

function numberFromMatch(text: string, pattern: RegExp): number | null {
  const match = text.match(pattern);
  if (!match?.[1]) return null;
  const value = Number(match[1]);
  return Number.isFinite(value) ? value : null;
}

function textFromMatch(text: string, pattern: RegExp): string | null {
  const match = text.match(pattern);
  return match?.[1]?.trim().slice(0, 120) || null;
}

function parseInvoiceText(text: string, fileName: string): InvoiceDraft {
  const total = numberFromMatch(text, /(?:价税合计|总金额|合计金额|total)[:：\s]*(?:CNY|RMB|￥)?\s*([0-9]+(?:\.[0-9]{1,2})?)/i);
  const amount = numberFromMatch(text, /(?:不含税金额|金额|amount)[:：\s]*(?:CNY|RMB|￥)?\s*([0-9]+(?:\.[0-9]{1,2})?)/i);
  const tax = numberFromMatch(text, /(?:税额|tax)[:：\s]*(?:CNY|RMB|￥)?\s*([0-9]+(?:\.[0-9]{1,2})?)/i);
  const detected = [
    total,
    amount,
    tax,
    textFromMatch(text, /(?:发票号码|invoice\s*no\.?|number)[:：\s]*([A-Za-z0-9-]{6,})/i),
    textFromMatch(text, /(?:销售方|销方|seller)[:：\s]*(.+)/i),
    textFromMatch(text, /(?:购买方|购方|buyer)[:：\s]*(.+)/i)
  ].filter(Boolean).length;
  return {
    id: `inv-${Date.now()}`,
    fileName,
    status: total || amount ? 'extracted' : 'needs_review',
    source: 'invoice.upload_and_extract',
    uploadedAt: new Date().toLocaleString('zh-CN', { hour12: false }),
    fields: {
      invoiceNumber: textFromMatch(text, /(?:发票号码|invoice\s*no\.?|number)[:：\s]*([A-Za-z0-9-]{6,})/i),
      invoiceCode: textFromMatch(text, /(?:发票代码|invoice\s*code)[:：\s]*([A-Za-z0-9-]{6,})/i),
      invoiceDate: textFromMatch(text, /(?:开票日期|发票日期|date)[:：\s]*([0-9]{4}[-/.年][0-9]{1,2}[-/.月][0-9]{1,2})/i),
      seller: textFromMatch(text, /(?:销售方|销方|seller)[:：\s]*(.+)/i),
      buyer: textFromMatch(text, /(?:购买方|购方|buyer)[:：\s]*(.+)/i),
      amount,
      tax,
      total: total ?? amount,
      currency: 'CNY',
      categorySuggestion: text.includes('样品') || text.includes('达人') ? '达人样品' : '发票待分配',
      confidence: detected ? Math.min(0.95, 0.35 + detected * 0.08) : 0.2
    }
  };
}

function buildOcrRequiredDraft(file: File): InvoiceDraft {
  return {
    id: `inv-${Date.now()}`,
    fileName: file.name,
    status: 'needs_ai_ocr_adapter',
    source: 'invoice.upload_and_extract',
    uploadedAt: new Date().toLocaleString('zh-CN', { hour12: false }),
    fields: {
      invoiceNumber: null,
      invoiceCode: null,
      invoiceDate: null,
      seller: null,
      buyer: null,
      amount: null,
      tax: null,
      total: null,
      currency: 'CNY',
      categorySuggestion: null,
      confidence: 0
    }
  };
}

function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error('文件读取失败'));
    reader.onload = () => {
      const value = String(reader.result || '');
      resolve(value.includes(',') ? value.split(',')[1] || '' : value);
    };
    reader.readAsDataURL(file);
  });
}

function numberFromUnknown(value: unknown): number | null {
  if (value === null || value === undefined || value === '') return null;
  const numberValue = Number(value);
  return Number.isFinite(numberValue) ? numberValue : null;
}

function mapBackendInvoiceDraft(result: unknown, fileName: string): InvoiceDraft {
  const draft = isRecord(result) ? result : {};
  const fields = isRecord(draft.fields) ? draft.fields : {};
  const status = typeof draft.status === 'string' ? draft.status as InvoiceDraft['status'] : 'needs_review';
  const id = typeof draft.id === 'string' ? draft.id : `inv-${Date.now()}`;
  return {
    id,
    backendId: id,
    fileName,
    status,
    source: typeof draft.source_status === 'string' ? draft.source_status : 'invoice.upload_and_extract',
    uploadedAt: new Date().toLocaleString('zh-CN', { hour12: false }),
    fields: {
      invoiceNumber: typeof fields.invoice_number === 'string' ? fields.invoice_number : null,
      invoiceCode: typeof fields.invoice_code === 'string' ? fields.invoice_code : null,
      invoiceDate: typeof fields.invoice_date === 'string' ? fields.invoice_date : null,
      seller: typeof fields.seller === 'string' ? fields.seller : null,
      buyer: typeof fields.buyer === 'string' ? fields.buyer : null,
      amount: numberFromUnknown(fields.amount),
      tax: numberFromUnknown(fields.tax),
      total: numberFromUnknown(fields.total),
      currency: normalizeCurrencyCode(fields.currency),
      categorySuggestion: typeof fields.category_suggestion === 'string' ? fields.category_suggestion : null,
      confidence: numberFromUnknown(fields.confidence) ?? 0
    }
  };
}

function mergeInvoiceDraft(current: InvoiceDraft, incoming: InvoiceDraft): InvoiceDraft {
  const statusRank: Record<InvoiceDraft['status'], number> = {
    needs_file: 0,
    file_not_found: 0,
    needs_ai_ocr_adapter: 0,
    needs_review: 0,
    extracted: 1,
    ledger_draft: 2,
    posted_to_ledger: 3
  };
  if (statusRank[incoming.status] >= statusRank[current.status]) return incoming;
  return { ...incoming, status: current.status };
}

function mapBackendTransaction(result: unknown): Transaction {
  const tx = isRecord(result) ? result : {};
  const id = typeof tx.id === 'string' ? tx.id : `txn-${Date.now()}`;
  const txType = tx.type === 'income' ? 'income' : 'expense';
  const status = tx.status === 'synced' || tx.status === 'posted' ? tx.status : 'needs_review';
  return {
    id,
    backendId: id,
    date: typeof tx.tx_date === 'string' ? tx.tx_date : localIsoDate(),
    type: txType,
    category: typeof tx.category === 'string' ? tx.category : '发票待分配',
    counterparty: typeof tx.counterparty === 'string' ? tx.counterparty : '发票对方',
    amount: Math.abs(Number(tx.amount_cents || 0)) / 100,
    currency: normalizeCurrencyCode(tx.currency),
    source: typeof tx.source === 'string' ? tx.source : 'invoice.post_to_ledger',
    evidence: typeof tx.evidence_path === 'string' ? tx.evidence_path : undefined,
    status
  };
}

async function handleInvoiceUpload(file: File, source: 'chat' | 'accounting' = 'accounting'): Promise<void> {
  if (file.size > 15 * 1024 * 1024) {
    state.uploadStatus = `${file.name} 超过 15 MB 限制`;
    showToast('文件过大，请上传 15 MB 以内的票据');
    return;
  }
  const textLike = /(\.txt|\.csv|\.json|\.md)$/i.test(file.name) || file.type.startsWith('text/');
  state.uploadStatus = `${file.name} 上传识别中`;
  if (source === 'chat') {
    state.messages.push({
      id: `owner-upload-${Date.now()}`,
      role: 'owner',
      content: `上传附件：${file.name}`
    });
  }
  render();

  let draft: InvoiceDraft;
  let toolSummary = '';
  let persisted = true;
  try {
    const contentBase64 = await readFileAsBase64(file);
    const result = await callBackendTool<unknown>('invoice.upload_and_extract', {
      owner_code: OWNER_CODE,
      file_name: file.name,
      content_base64: contentBase64
    });
    draft = mapBackendInvoiceDraft(result, file.name);
    toolSummary = JSON.stringify(result);
  } catch (error) {
    persisted = false;
    draft = textLike ? parseInvoiceText(await file.text(), file.name) : buildOcrRequiredDraft(file);
    draft.source = 'local_preview_unsaved';
    toolSummary = error instanceof Error ? error.message : String(error);
  }

  const existingDraftIndex = state.invoiceDrafts.findIndex((item) => item.id === draft.id);
  if (existingDraftIndex >= 0) {
    draft = mergeInvoiceDraft(state.invoiceDrafts[existingDraftIndex], draft);
    state.invoiceDrafts.splice(existingDraftIndex, 1, draft);
  }
  else state.invoiceDrafts.unshift(draft);
  state.uploadStatus = !persisted
    ? `${file.name} 仅本地预览，后端未保存`
    : draft.status === 'needs_ai_ocr_adapter'
      ? `${file.name} 等待 OCR/AI 识别`
      : `${file.name} 已进入发票草稿`;
  state.messages.push({
    id: `invoice-${Date.now()}`,
    role: 'assistant',
    content: !persisted
      ? '后端保存失败，只生成了本地识别预览。该预览不能生成流水，请恢复本地 Rust 后端后重新上传。'
      : draft.status === 'needs_ai_ocr_adapter'
        ? '文件已进入发票草稿，但本地 OCR/视觉工具还没返回字段，金额不会被猜测。'
        : '发票已经通过工具抽取字段并进入草稿，可以生成流水后同步 Actual。',
    toolRun: {
      name: 'invoice.upload_and_extract',
      label: '上传发票识别',
      summary: toolSummary || JSON.stringify({
        id: draft.id,
        fileName: draft.fileName,
        status: draft.status,
        fields: draft.fields
      })
    }
  });
  render();
  scrollMessagesToBottom();
}

async function postInvoiceToLedger(invoiceId: string): Promise<void> {
  const draft = state.invoiceDrafts.find((item) => item.id === invoiceId);
  if (!draft || draft.status === 'posted_to_ledger' || invoicePostingIds.has(invoiceId)) return;
  const total = draft.fields.total ?? draft.fields.amount;
  if (!total) return;
  if (!draft.backendId) {
    showToast('票据尚未保存到后端，请恢复本地 Rust 后端后重新上传');
    return;
  }
  invoicePostingIds.add(invoiceId);
  render();
  try {
    const result = await callBackendTool<{ transaction?: unknown; invoice_id?: string }>('invoice.post_to_ledger', {
      owner_code: OWNER_CODE,
      invoice_id: draft.backendId,
      category: draft.fields.categorySuggestion || '发票待分配',
      status: 'needs_review'
    });
    const transaction = mapBackendTransaction(result.transaction);
    const existing = state.transactions.find((item) => item.id === transaction.id);
    if (existing) Object.assign(existing, transaction);
    else state.transactions.unshift(transaction);
    state.ledgerSummary = null;
    draft.status = transaction.status === 'needs_review' ? 'ledger_draft' : 'posted_to_ledger';
    state.messages.push({
      id: `invoice-ledger-${Date.now()}`,
      role: 'assistant',
      content: transaction.status === 'needs_review' ? '发票已生成待复核流水，确认入账后再同步 Actual。' : '发票对应流水已入账，可以同步 Actual。',
      toolRun: {
        name: 'invoice.post_to_ledger',
        label: '发票生成流水',
        summary: JSON.stringify(result)
      }
    });
    showToast(transaction.status === 'needs_review' ? '待复核流水已生成' : '发票已入账');
    void refreshWorkspaceData(false);
    scrollMessagesToBottom();
  } catch (error) {
    state.messages.push({
      id: `invoice-ledger-failed-${Date.now()}`,
      role: 'assistant',
      content: `Rust 后端生成流水失败：${error instanceof Error ? error.message : String(error)}`,
      toolRun: {
        name: 'invoice.post_to_ledger',
        label: '发票生成流水',
        summary: error instanceof Error ? error.message : String(error)
      }
    });
    render();
    scrollMessagesToBottom();
  } finally {
    invoicePostingIds.delete(invoiceId);
    render();
  }
}

async function approveTransaction(transactionId: string): Promise<void> {
  const transaction = state.transactions.find((item) => item.id === transactionId);
  if (!transaction || transaction.status !== 'needs_review') return;
  if (!transaction.backendId) {
    transaction.status = 'posted';
    showToast('演示流水已标记入账');
    return;
  }
  try {
    const result = await callBackendTool<unknown>('ledger.update_transaction_status', {
      owner_code: OWNER_CODE,
      transaction_id: transaction.backendId,
      status: 'posted',
      review_note: '老板端确认入账'
    });
    Object.assign(transaction, mapBackendTransaction(result));
    state.ledgerSummary = null;
    if (transaction.source.startsWith('invoice:')) {
      const invoiceId = transaction.source.slice('invoice:'.length);
      const draft = state.invoiceDrafts.find((item) => item.id === invoiceId);
      if (draft) draft.status = 'posted_to_ledger';
    }
    showToast('流水已确认入账');
    void refreshWorkspaceData(false);
  } catch (error) {
    showToast(`入账失败：${error instanceof Error ? error.message : String(error)}`);
  }
}

async function queueActualSync(transactionId: string): Promise<void> {
  const transaction = state.transactions.find((item) => item.id === transactionId);
  if (!transaction || transaction.status === 'synced' || transaction.status === 'needs_review') return;
  if (state.actualSyncJobs.some((item) => item.transactionId === transactionId && (item.status === 'queued' || item.status === 'running'))) return;
  const job: ActualSyncJob = {
    id: `actual-job-${Date.now()}`,
    transactionId,
    status: 'queued',
    reason: transaction.backendId ? '正在调用 Rust Actual 同步工具' : '演示流水没有后端账本 ID，已进入本地队列',
    createdAt: new Date().toISOString()
  };
  state.actualSyncJobs.unshift(job);
  render();

  if (transaction.backendId) {
    if (!actualStatus.directSyncReady) {
      await refreshActualStatus(false);
    }
    try {
      const result = await callBackendTool<Record<string, unknown>>('actual.sync_transaction', {
        owner_code: OWNER_CODE,
        transaction_id: transaction.backendId
      });
      const status = typeof result.status === 'string' ? result.status : '';
      if (status === 'synced') {
        job.status = 'synced';
        job.reason = '已通过 Actual CLI 写入本地 Actual server';
        transaction.status = 'synced';
      } else if (status === 'actual_cli_failed') {
        job.status = 'failed';
        job.reason = typeof result.stderr === 'string' && result.stderr.trim() ? result.stderr.trim() : 'Actual CLI 返回失败';
      } else if (status === 'dry_run') {
        job.status = 'dry_run';
        job.reason = 'Actual dry-run 已完成';
      } else {
        job.status = 'queued';
        job.reason = typeof result.reason === 'string' ? result.reason : 'Actual 返回未直连，已记录同步任务';
      }
      state.messages.push({
        id: `actual-sync-${Date.now()}`,
        role: 'assistant',
        content: job.status === 'synced' ? '流水已同步到 Actual。' : 'Actual 未完成直连同步，流水已进入同步队列。',
        toolRun: {
          name: 'actual.sync_transaction',
          label: '同步 Actual',
          summary: JSON.stringify(result)
        }
      });
    } catch (error) {
      job.status = 'failed';
      job.reason = error instanceof Error ? error.message : String(error);
      state.messages.push({
        id: `actual-sync-failed-${Date.now()}`,
        role: 'assistant',
        content: `Actual 同步失败：${job.reason}`,
        toolRun: {
          name: 'actual.sync_transaction',
          label: '同步 Actual',
          summary: job.reason
        }
      });
    }
    state.actualSyncJobCounts = {};
    if (!state.actualSyncJobs.some((item) => item.id === job.id)) state.actualSyncJobs.unshift(job);
    await refreshActualStatus(false);
    scrollMessagesToBottom();
    return;
  }

  state.messages.push({
    id: `actual-sync-${Date.now()}`,
    role: 'assistant',
    content: '这条是前端演示流水，没有后端账本 ID，不能直接写 Actual。请从上传发票生成真实流水后同步。',
    toolRun: {
      name: 'actual.sync_transaction',
      label: '同步 Actual',
      summary: JSON.stringify({ transactionId, status: job.status, requiredEnv: actualStatus.requiredEnv })
    }
  });
  render();
}

async function exportActualImportFile(): Promise<void> {
  try {
    const result = await callBackendTool<unknown>('actual.export_import_file', {
      owner_code: OWNER_CODE,
      month: localIsoDate().slice(0, 7)
    });
    state.messages.push({
      id: `actual-export-${Date.now()}`,
      role: 'assistant',
      content: 'Actual 导入文件已生成。',
      toolRun: {
        name: 'actual.export_import_file',
        label: '导出 Actual 导入文件',
        summary: JSON.stringify(result)
      }
    });
    showToast('Actual 导入文件已生成');
  } catch (error) {
    showToast(`导出失败：${error instanceof Error ? error.message : String(error)}`);
  }
}

function render(): void {
  appRoot.innerHTML = state.signedIn ? renderApp() : renderLogin();
  createIcons({ icons: iconNodes });
  bindEvents();
}

function bindEvents(): void {
  document.querySelectorAll<HTMLButtonElement>('[data-toggle-theme]').forEach((button) => {
    button.addEventListener('click', () => {
      state.themeMode = state.themeMode === 'dark' ? 'light' : 'dark';
      applyThemeMode(state.themeMode);
      try {
        window.localStorage.setItem(THEME_STORAGE_KEY, state.themeMode);
      } catch {
        // The current window still keeps the selected theme when storage is unavailable.
      }
      render();
    });
  });

  document.querySelector<HTMLFormElement>('#login-form')?.addEventListener('submit', (event) => {
    event.preventDefault();
    const form = new FormData(event.currentTarget as HTMLFormElement);
    const email = String(form.get('email') || '').trim();
    const password = String(form.get('password') || '');
    if (email !== 'boss@demo.local' || password !== 'demo-password') {
      state.loginError = '演示账号或密码不正确';
      render();
      return;
    }
    state.loginError = '';
    state.signedIn = true;
    render();
    void refreshWorkspaceData(false);
  });

  document.querySelector<HTMLButtonElement>('#logout')?.addEventListener('click', () => {
    workspaceRequestSequence += 1;
    state.signedIn = false;
    render();
  });

  document.querySelectorAll<HTMLButtonElement>('[data-view]').forEach((button) => {
    button.addEventListener('click', () => {
      const view = button.dataset.view as View | undefined;
      if (!view) return;
      state.currentView = view;
      if ((view === 'overview' || view === 'assistant') && state.weekOffset !== 0) {
        state.weekOffset = 0;
        render();
        void refreshWorkspaceData(false);
        return;
      }
      render();
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-refresh-workspace]').forEach((button) => {
    button.addEventListener('click', () => void refreshWorkspaceData(true));
  });

  document.querySelectorAll<HTMLButtonElement>('[data-overview-ai]').forEach((button) => {
    button.addEventListener('click', () => {
      state.currentView = 'assistant';
      render();
      void sendPrompt('生成今天的管理摘要', 'summary');
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-open-action]').forEach((button) => {
    button.addEventListener('click', () => {
      const action = currentBossActions().find((item) => item.id === button.dataset.openAction);
      if (!action) return;
      state.currentView = action.targetView;
      if (action.employeeId) state.selectedEmployeeId = action.employeeId;
      if (action.transactionId) state.ledgerFilter = 'needs_review';
      render();
    });
  });

  document.querySelectorAll<HTMLButtonElement>('#copy-code, #copy-code-secondary').forEach((button) => {
    button.addEventListener('click', async () => {
      try {
        if (!navigator.clipboard) throw new Error('clipboard_unavailable');
        await navigator.clipboard.writeText(OWNER_CODE);
        showToast('老板码已复制');
      } catch {
        showToast(`无法访问剪贴板，老板码为 ${OWNER_CODE}`);
      }
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-prompt]').forEach((button) => {
    button.addEventListener('click', () => {
      const prompt = button.dataset.prompt || button.textContent || '';
      const mode = (button.dataset.summaryMode as SummaryMode | undefined) ?? 'summary';
      void sendPrompt(prompt, mode);
    });
  });

  document.querySelector<HTMLFormElement>('#chat-form')?.addEventListener('submit', (event) => {
    event.preventDefault();
    const form = new FormData(event.currentTarget as HTMLFormElement);
    const prompt = String(form.get('prompt') || '').trim();
    if (!prompt) return;
    void sendPrompt(prompt);
  });

  document.querySelectorAll<HTMLButtonElement>('[data-run-tool]').forEach((button) => {
    button.addEventListener('click', () => {
      const name = button.dataset.runTool as ToolName | undefined;
      if (!name) return;
      if (name === 'actual.integration_status') {
        void refreshActualStatus(true);
        return;
      }
      if (name === 'boss.owner_context') {
        void refreshWorkspaceData(true);
        return;
      }
      const tool = callTool(name);
      state.messages.push({
        id: `tool-${Date.now()}`,
        role: 'assistant',
        content: `${tool.label} 已返回结构化结果。`,
        toolRun: {
          name: tool.name,
          label: tool.label,
          summary: JSON.stringify(tool.result)
        }
      });
      state.currentView = 'assistant';
      render();
      scrollMessagesToBottom();
    });
  });

  document.querySelector<HTMLInputElement>('#chat-attachment-input')?.addEventListener('change', async (event) => {
    const input = event.currentTarget as HTMLInputElement;
    const files = Array.from(input.files ?? []);
    input.value = '';
    const totalBytes = files.reduce((sum, file) => sum + file.size, 0);
    if (files.length > 5 || totalBytes > 30 * 1024 * 1024) {
      showToast('一次最多上传 5 个文件，合计不超过 30 MB');
      return;
    }
    for (const file of files) await handleInvoiceUpload(file, 'chat');
  });

  document.querySelector<HTMLInputElement>('#invoice-upload')?.addEventListener('change', (event) => {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0];
    if (!file) return;
    void handleInvoiceUpload(file);
  });

  document.querySelectorAll<HTMLButtonElement>('[data-post-invoice]').forEach((button) => {
    button.addEventListener('click', () => {
      const invoiceId = button.dataset.postInvoice || '';
      if (!invoiceId) return;
      void postInvoiceToLedger(invoiceId);
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-sync-transaction]').forEach((button) => {
    button.addEventListener('click', () => {
      const transactionId = button.dataset.syncTransaction || '';
      if (!transactionId) return;
      void queueActualSync(transactionId);
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-approve-transaction]').forEach((button) => {
    button.addEventListener('click', () => {
      const transactionId = button.dataset.approveTransaction || '';
      if (transactionId) void approveTransaction(transactionId);
    });
  });

  document.querySelector<HTMLButtonElement>('[data-export-actual]')?.addEventListener('click', () => {
    void exportActualImportFile();
  });

  document.querySelector<HTMLButtonElement>('[data-load-more-transactions]')?.addEventListener('click', () => {
    void loadMoreTransactions();
  });

  document.querySelectorAll<HTMLButtonElement>('[data-toggle-finance-settings]').forEach((button) => {
    button.addEventListener('click', () => {
      state.showFinanceSettings = !state.showFinanceSettings;
      render();
    });
  });

  document.querySelector<HTMLButtonElement>('[data-focus-entry]')?.addEventListener('click', () => {
    document.querySelector<HTMLElement>('#manual-entry-card')?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    window.setTimeout(() => document.querySelector<HTMLInputElement>('#manual-entry-card input[name="amount"]')?.focus(), 280);
  });

  document.querySelectorAll<HTMLButtonElement>('[data-employee-id]').forEach((button) => {
    button.addEventListener('click', () => {
      const employeeId = button.dataset.employeeId || '';
      if (!employeeId) return;
      state.selectedEmployeeId = employeeId;
      render();
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-open-employee]').forEach((button) => {
    button.addEventListener('click', () => {
      const employeeId = button.dataset.openEmployee || '';
      if (!employeeId) return;
      state.selectedEmployeeId = employeeId;
      state.currentView = 'employees';
      render();
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-week-shift]').forEach((button) => {
    button.addEventListener('click', () => {
      const shift = Number(button.dataset.weekShift || 0);
      state.weekOffset = Math.max(-8, Math.min(0, state.weekOffset + shift));
      void refreshWorkspaceData(false);
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-employee-filter]').forEach((button) => {
    button.addEventListener('click', () => {
      state.employeeFilter = button.dataset.employeeFilter as EmployeeFilter;
      render();
    });
  });

  document.querySelector<HTMLInputElement>('#employee-search')?.addEventListener('input', (event) => {
    if ((event as InputEvent).isComposing) return;
    const input = event.currentTarget as HTMLInputElement;
    state.employeeSearch = input.value;
    render();
    window.requestAnimationFrame(() => {
      const next = document.querySelector<HTMLInputElement>('#employee-search');
      next?.focus();
      next?.setSelectionRange(next.value.length, next.value.length);
    });
  });

  document.querySelectorAll<HTMLButtonElement>('[data-review-employee]').forEach((button) => {
    button.addEventListener('click', async () => {
      const employeeId = button.dataset.reviewEmployee || '';
      const reviewStatus = button.dataset.reviewStatus as ReviewStatus | undefined;
      if (!employeeId || !reviewStatus) return;
      const textarea = button.closest<HTMLFormElement>('[data-review-form]')?.querySelector<HTMLTextAreaElement>('textarea');
      const note = textarea?.value.trim() || '';
      if (reviewStatus === 'needs_supplement' && !note) {
        showToast('记录补充要求前请先填写具体内容');
        textarea?.focus();
        return;
      }
      button.closest<HTMLFormElement>('[data-review-form]')
        ?.querySelectorAll<HTMLButtonElement>('[data-review-employee]')
        .forEach((reviewButton) => { reviewButton.disabled = true; });
      await saveWeekReview(employeeId, reviewStatus, note);
    });
  });

  document.querySelector<HTMLTextAreaElement>('[data-review-form] textarea')?.addEventListener('input', (event) => {
    const textarea = event.currentTarget as HTMLTextAreaElement;
    const form = textarea.closest<HTMLFormElement>('[data-review-form]');
    const employeeId = form?.dataset.reviewForm || '';
    if (!employeeId) return;
    const reviewKey = employeeReviewKey(employeeId);
    state.employeeNotes[reviewKey] = textarea.value;
    state.employeeNoteDrafts[reviewKey] = textarea.value;
    writeStoredRecord('cohmira.employeeNotes', state.employeeNotes);
    writeStoredRecord('cohmira.employeeNoteDrafts', state.employeeNoteDrafts);
  });

  document.querySelectorAll<HTMLButtonElement>('[data-ledger-filter]').forEach((button) => {
    button.addEventListener('click', () => {
      state.ledgerFilter = button.dataset.ledgerFilter as LedgerFilter;
      render();
      void ensureLedgerFilterRows(state.ledgerFilter);
    });
  });

  document.querySelector<HTMLFormElement>('#transaction-form')?.addEventListener('submit', async (event) => {
    event.preventDefault();
    const formElement = event.currentTarget as HTMLFormElement;
    const form = new FormData(formElement);
    const amount = Number(form.get('amount') || 0);
    if (!Number.isFinite(amount) || amount <= 0) return;
    const submitButton = formElement.querySelector<HTMLButtonElement>('button[type="submit"]');
    if (submitButton) submitButton.disabled = true;
    try {
      const result = await callBackendTool<unknown>('ledger.add_transaction', {
        owner_code: OWNER_CODE,
        date: String(form.get('date') || localIsoDate()),
        type: String(form.get('type')) === 'income' ? 'income' : 'expense',
        category: String(form.get('category') || '未分类'),
        counterparty: String(form.get('counterparty') || ''),
        amount,
        source: '老板端手工录入',
        status: 'needs_review'
      });
      state.transactions.unshift(mapBackendTransaction(result));
      state.ledgerSummary = null;
      state.ledgerFilter = 'needs_review';
      showToast('流水已保存并进入待复核');
      void refreshWorkspaceData(false);
    } catch (error) {
      showToast(`保存失败：${error instanceof Error ? error.message : String(error)}`);
      if (submitButton) submitButton.disabled = false;
    }
  });
}

render();
