import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { AlertCircle, Clock3, ListTodo, Loader2, MessageSquareText, Play, RefreshCw, Send } from 'lucide-react';
import { appAlert } from '../utils/appDialogs';

type TaskListResponse = Awaited<ReturnType<typeof window.ipcRenderer.redclawRunner.taskList>>;
type TaskListItem = NonNullable<TaskListResponse['items']>[number];
type TaskStatsResponse = Awaited<ReturnType<typeof window.ipcRenderer.redclawRunner.taskStats>>;
type BossWeekReview = Awaited<ReturnType<typeof window.ipcRenderer.bossSync.listWeekReviews>>['reviews'][number];

type TaskFilterKey = 'all' | 'generation' | 'scheduled' | 'long_cycle' | 'draft' | 'active' | 'cooldown';

function formatDateTime(value?: string | number | null): string {
    if (!value) return '-';
    const raw = typeof value === 'number' ? value : (/^\d{10,}$/.test(value) ? Number(value) : value);
    const ts = typeof raw === 'number' ? raw : Date.parse(raw);
    if (!Number.isFinite(ts)) return String(value);
    return new Date(ts).toLocaleString('zh-CN', { hour12: false });
}

function kindLabel(kind: string): string {
    if (kind === 'generation') return '生成任务';
    if (kind === 'generation_history') return '生成记录';
    if (kind === 'task') return '执行任务';
    if (kind === 'draft') return '任务草稿';
    return kind === 'long_cycle' ? '长周期任务' : '定时任务';
}

function lifecycleLabel(item: TaskListItem): string {
    if (item.requiresConfirmation) return '待确认';
    if (item.cooldown?.state === 'active') return '冷却中';
    if (['succeeded', 'completed'].includes(String(item.latestExecution?.status || ''))) return '已完成';
    return item.enabled ? '已启用' : '已停用';
}

function lifecycleTone(item: TaskListItem): string {
    if (item.requiresConfirmation) return 'bg-status-warning/15 text-text-primary';
    if (item.cooldown?.state === 'active') return 'bg-status-error/15 text-text-primary';
    if (['succeeded', 'completed'].includes(String(item.latestExecution?.status || ''))) return 'bg-status-success/15 text-text-primary';
    return item.enabled ? 'bg-status-success/15 text-text-primary' : 'bg-surface-tertiary/60 text-text-secondary';
}

function policyLabel(value?: string | null): string {
    switch ((value || '').trim()) {
        case 'allow':
            return '允许';
        case 'require_confirm':
            return '需确认';
        case 'reject':
            return '拒绝';
        default:
            return '未标注';
    }
}

function executionStatusLabel(value?: string | null): string {
    switch ((value || '').trim()) {
        case 'queued':
            return '排队中';
        case 'leased':
            return '已领取';
        case 'running':
            return '执行中';
        case 'retrying':
            return '等待重试';
        case 'succeeded':
        case 'completed':
            return '已成功';
        case 'failed':
            return '失败';
        case 'cancelled':
            return '已取消';
        case 'dead_lettered':
            return '死信';
        default:
            return value || '暂无';
    }
}

function triggerLabel(item: TaskListItem): string {
    if (item.kind === 'generation') return '应用内后台执行';
    if (item.kind === 'generation_history') return '历史完成产物';
    if (item.kind === 'task') return '后台执行';
    if (item.kind === 'long_cycle') {
        return item.triggerKind === 'interval' ? '按轮次推进' : item.triggerKind || '多轮推进';
    }
    switch ((item.triggerKind || '').trim()) {
        case 'interval':
            return '按间隔';
        case 'daily':
            return '每天';
        case 'weekly':
            return '每周';
        case 'once':
            return '单次';
        default:
            return item.triggerKind || '未设置';
    }
}

function actionTypeLabel(value?: string | null): string {
    const raw = String(value || '').trim();
    if (!raw) return '';
    return raw
        .split(/[_-]+/)
        .filter(Boolean)
        .map((part) => part[0]?.toUpperCase() + part.slice(1))
        .join(' ');
}

function taskContent(item: TaskListItem): string {
    const values = [item.goal, item.prompt, item.objective, item.stepPrompt]
        .map((value) => String(value || '').trim())
        .filter(Boolean);
    return values[0] || '当前任务没有附带说明内容。';
}

function scheduleSummary(item: TaskListItem): string {
    if (item.kind === 'generation_history') {
        return `生成记录 · 完成于 ${formatDateTime(item.latestExecution?.updatedAt || item.updatedAt)}`;
    }
    if (item.kind === 'generation' || item.kind === 'task') {
        return `应用内后台任务 · 更新于 ${formatDateTime(item.updatedAt)}`;
    }
    if (item.kind === 'long_cycle') {
        const completed = Number(item.completedRounds || 0);
        const total = Number(item.totalRounds || 0);
        const progress = total > 0 ? `第 ${completed}/${total} 轮` : '多轮推进';
        return `${progress} · 下次 ${formatDateTime(item.nextDueAt)}`;
    }
    switch ((item.triggerKind || '').trim()) {
        case 'interval':
            return `按固定间隔触发 · 下次 ${formatDateTime(item.nextDueAt)}`;
        case 'daily':
            return `每天固定时间 · 下次 ${formatDateTime(item.nextDueAt)}`;
        case 'weekly':
            return `每周固定时间 · 下次 ${formatDateTime(item.nextDueAt)}`;
        case 'once':
            return `单次执行 · 计划 ${formatDateTime(item.nextDueAt)}`;
        default:
            return `下次 ${formatDateTime(item.nextDueAt)}`;
    }
}

function shortFingerprint(value?: string | null): string {
    const raw = String(value || '').trim();
    if (!raw) return '-';
    if (raw.length <= 18) return raw;
    return `${raw.slice(0, 8)}...${raw.slice(-8)}`;
}

function sortRank(item: TaskListItem): number {
    if (item.requiresConfirmation) return 0;
    if (item.cooldown?.state === 'active') return 1;
    if (item.enabled) return 2;
    return 3;
}

function matchesFilter(item: TaskListItem, filter: TaskFilterKey): boolean {
    switch (filter) {
        case 'scheduled':
            return item.kind === 'scheduled';
        case 'generation':
            return item.kind === 'generation' || item.kind === 'generation_history';
        case 'long_cycle':
            return item.kind === 'long_cycle';
        case 'draft':
            return item.requiresConfirmation;
        case 'active':
            return item.enabled && item.cooldown?.state !== 'active' && !item.requiresConfirmation;
        case 'cooldown':
            return item.cooldown?.state === 'active';
        default:
            return true;
    }
}

function isCompletedForBossReport(item: TaskListItem | null): item is TaskListItem {
    if (!item || item.requiresConfirmation) return false;
    return ['succeeded', 'completed'].includes(String(item.latestExecution?.status || ''));
}

function localIsoDate(value?: string | number | Date | null): string {
    const parsed = value instanceof Date ? new Date(value.getTime()) : value ? new Date(value) : new Date();
    const date = Number.isNaN(parsed.getTime()) ? new Date() : parsed;
    const year = date.getFullYear();
    const month = String(date.getMonth() + 1).padStart(2, '0');
    const day = String(date.getDate()).padStart(2, '0');
    return `${year}-${month}-${day}`;
}

function currentWeekRange(): { date_from: string; date_to: string } {
    const today = new Date();
    const monday = new Date(today.getFullYear(), today.getMonth(), today.getDate());
    monday.setDate(monday.getDate() - ((monday.getDay() + 6) % 7));
    const sunday = new Date(monday);
    sunday.setDate(monday.getDate() + 6);
    return { date_from: localIsoDate(monday), date_to: localIsoDate(sunday) };
}

function defaultBossSummary(item: TaskListItem): string {
    const detail = taskContent(item).trim();
    return detail && detail !== '当前任务没有附带说明内容。'
        ? `完成「${item.title}」。${detail}`
        : `完成「${item.title}」。`;
}

function reviewStatusLabel(status: string): string {
    if (status === 'reviewed') return '已审阅';
    if (status === 'needs_supplement') return '需补充';
    return status === 'pending' ? '待审阅' : status;
}

async function runTaskNow(item: TaskListItem): Promise<void> {
    if (!item.sourceTaskId || !item.sourceKind) {
        throw new Error('当前任务没有可立即执行的源任务。');
    }
    if (item.sourceKind === 'scheduled') {
        await window.ipcRenderer.redclawRunner.runScheduledNow({ taskId: item.sourceTaskId });
        return;
    }
    if (item.sourceKind === 'long_cycle') {
        await window.ipcRenderer.redclawRunner.runLongCycleNow({ taskId: item.sourceTaskId });
        return;
    }
    if (item.sourceKind === 'generation') {
        await window.ipcRenderer.generation.retryJob(item.sourceTaskId);
        return;
    }
    throw new Error('当前任务类型暂不支持立即执行。');
}

async function setTaskEnabled(item: TaskListItem, enabled: boolean): Promise<void> {
    if (!item.sourceTaskId || !item.sourceKind) {
        throw new Error(enabled ? '当前任务没有可恢复的源任务。' : '当前任务没有可停用的源任务。');
    }
    if (item.sourceKind === 'scheduled') {
        await window.ipcRenderer.redclawRunner.setScheduledEnabled({ taskId: item.sourceTaskId, enabled });
        return;
    }
    if (item.sourceKind === 'long_cycle') {
        await window.ipcRenderer.redclawRunner.setLongCycleEnabled({ taskId: item.sourceTaskId, enabled });
        return;
    }
    if (item.sourceKind === 'generation') {
        if (enabled) {
            await window.ipcRenderer.generation.retryJob(item.sourceTaskId);
        } else {
            await window.ipcRenderer.generation.cancelJob(item.sourceTaskId);
        }
        return;
    }
    throw new Error('当前任务类型暂不支持启停。');
}

function StatCard({
    label,
    value,
}: {
    label: string;
    value: number;
}) {
    return (
        <div className="inline-flex min-w-fit items-center gap-2.5 rounded-full border border-border bg-surface-primary px-3.5 py-2 shadow-[var(--ui-shadow-1)]">
            <div data-e2e-contrast="workboard-stat-label" className="whitespace-nowrap text-[10px] uppercase tracking-[0.16em] text-text-secondary">{label}</div>
            <div data-e2e-contrast="workboard-stat-value" className="text-[18px] font-semibold leading-none text-text-primary">{value}</div>
        </div>
    );
}

function DetailRow({
    label,
    value,
}: {
    label: string;
    value: string;
}) {
    return (
        <div className="rounded-[18px] border border-border bg-surface-secondary/60 px-3.5 py-2.5">
            <div className="text-[10px] uppercase tracking-[0.16em] text-text-tertiary">{label}</div>
            <div className="mt-1 break-words text-[13px] leading-5 text-text-primary">{value}</div>
        </div>
    );
}

export function Workboard({ isActive = true }: { isActive?: boolean }) {
    const [items, setItems] = useState<TaskListItem[]>([]);
    const [stats, setStats] = useState<TaskStatsResponse | null>(null);
    const [runnerStatus, setRunnerStatus] = useState<Record<string, any> | null>(null);
    const [loading, setLoading] = useState(true);
    const [error, setError] = useState('');
    const [lastUpdatedAt, setLastUpdatedAt] = useState('');
    const [selectedId, setSelectedId] = useState('');
    const [filter, setFilter] = useState<TaskFilterKey>('all');
    const [actionState, setActionState] = useState<{ id: string; action: string } | null>(null);
    const [bossSummary, setBossSummary] = useState('');
    const [bossMaterialCount, setBossMaterialCount] = useState('0');
    const [bossQualityScore, setBossQualityScore] = useState('0');
    const [bossCostCents, setBossCostCents] = useState('0');
    const [bossReviews, setBossReviews] = useState<BossWeekReview[]>([]);
    const [bossSyncMessage, setBossSyncMessage] = useState('');
    const [bossReviewsLoading, setBossReviewsLoading] = useState(false);
    const itemsRef = useRef<TaskListItem[]>([]);
    const loadRequestRef = useRef(0);

    useEffect(() => {
        itemsRef.current = items;
    }, [items]);

    const load = useCallback(async () => {
        const requestId = loadRequestRef.current + 1;
        loadRequestRef.current = requestId;
        if (itemsRef.current.length === 0) {
            setLoading(true);
        }
        setError('');
        try {
            const [taskListResult, taskStatsResult, runnerStatusResult] = await Promise.all([
                window.ipcRenderer.redclawRunner.taskList({ includeDrafts: true }),
                window.ipcRenderer.redclawRunner.taskStats(),
                window.ipcRenderer.redclawRunner.getStatus(),
            ]);
            if (requestId !== loadRequestRef.current) return;
            const nextItems = Array.isArray(taskListResult?.items) ? [...taskListResult.items] : [];
            nextItems.sort((left, right) => {
                const rankDelta = sortRank(left) - sortRank(right);
                if (rankDelta !== 0) return rankDelta;
                const leftDueAt = Date.parse(left.nextDueAt || '') || Number.MAX_SAFE_INTEGER;
                const rightDueAt = Date.parse(right.nextDueAt || '') || Number.MAX_SAFE_INTEGER;
                if (leftDueAt !== rightDueAt) return leftDueAt - rightDueAt;
                return Date.parse(right.updatedAt || '') - Date.parse(left.updatedAt || '');
            });
            setItems(nextItems);
            setStats(taskStatsResult || null);
            setRunnerStatus(runnerStatusResult && typeof runnerStatusResult === 'object' ? runnerStatusResult : null);
            setLastUpdatedAt(new Date().toISOString());
            setSelectedId((prev) => (prev && nextItems.some((item) => item.definitionId === prev) ? prev : nextItems[0]?.definitionId || ''));
        } catch (loadError) {
            if (requestId !== loadRequestRef.current) return;
            setError(loadError instanceof Error ? loadError.message : String(loadError));
        } finally {
            if (requestId === loadRequestRef.current) {
                setLoading(false);
            }
        }
    }, []);

    useEffect(() => {
        if (!isActive) return;
        void load();
    }, [isActive, load]);

    const filteredItems = useMemo(
        () => items.filter((item) => matchesFilter(item, filter)),
        [filter, items],
    );

    useEffect(() => {
        if (!filteredItems.length) {
            setSelectedId('');
            return;
        }
        if (!selectedId || !filteredItems.some((item) => item.definitionId === selectedId)) {
            setSelectedId(filteredItems[0].definitionId);
        }
    }, [filteredItems, selectedId]);

    const selectedItem = useMemo(
        () => filteredItems.find((item) => item.definitionId === selectedId) || filteredItems[0] || null,
        [filteredItems, selectedId],
    );

    useEffect(() => {
        if (!selectedItem) {
            setBossSummary('');
            return;
        }
        setBossSummary(defaultBossSummary(selectedItem));
        setBossMaterialCount(
            ['generation', 'generation_history'].includes(selectedItem.kind) ? '1' : '0',
        );
        setBossQualityScore('0');
        setBossCostCents('0');
        setBossSyncMessage('');
    }, [selectedItem?.definitionId, selectedItem?.latestExecution?.executionId]);

    const filterOptions = useMemo(() => ([
        { key: 'all' as const, label: '全部任务', count: items.length },
        { key: 'generation' as const, label: '生成与记录', count: items.filter((item) => item.kind === 'generation' || item.kind === 'generation_history').length },
        { key: 'scheduled' as const, label: '定时任务', count: items.filter((item) => item.kind === 'scheduled').length },
        { key: 'long_cycle' as const, label: '长周期', count: items.filter((item) => item.kind === 'long_cycle').length },
        { key: 'draft' as const, label: '待确认', count: items.filter((item) => item.requiresConfirmation).length },
        { key: 'cooldown' as const, label: '冷却中', count: items.filter((item) => item.cooldown?.state === 'active').length },
    ]), [items]);

    const topStats = useMemo(() => ({
        totalDefinitions: stats?.definitions?.total ?? items.length,
        generation: items.filter((item) => item.kind === 'generation' || item.kind === 'generation_history').length,
        scheduled: items.filter((item) => item.kind === 'scheduled').length,
        longCycle: items.filter((item) => item.kind === 'long_cycle').length,
        active: stats?.definitions?.active ?? items.filter((item) => item.enabled).length,
        runningExecutions: stats?.executions?.running ?? 0,
        failedExecutions: stats?.executions?.failed ?? 0,
    }), [items, stats]);

    const executeAction = useCallback(async (
        item: TaskListItem,
        action: string,
        fn: () => Promise<void>,
    ) => {
        try {
            setActionState({ id: item.definitionId, action });
            await fn();
            await load();
        } catch (actionError) {
            void appAlert(actionError instanceof Error ? actionError.message : String(actionError));
        } finally {
            setActionState((current) => (
                current?.id === item.definitionId && current?.action === action
                    ? null
                    : current
            ));
        }
    }, [load]);

    const reportSelectedTask = useCallback(async () => {
        if (!isCompletedForBossReport(selectedItem)) {
            await appAlert('只有已有成功执行记录的任务可以上报，草稿、失败和未执行任务不会自动上报。');
            return;
        }
        const summary = bossSummary.trim();
        if (!summary) {
            await appAlert('请先填写本次工作摘要。');
            return;
        }
        const materialCount = Number(bossMaterialCount);
        const qualityScore = Number(bossQualityScore);
        const costCents = Number(bossCostCents);
        if (![materialCount, qualityScore, costCents].every(Number.isInteger)
            || materialCount < 0
            || costCents < 0
            || qualityScore < 0
            || qualityScore > 100) {
            await appAlert('产出件数和成本必须是非负整数，质量评分必须是 0 到 100 的整数。');
            return;
        }
        try {
            setActionState({ id: selectedItem.definitionId, action: 'boss-report' });
            setBossSyncMessage('');
            const executionId = String(selectedItem.latestExecution?.executionId || 'completed');
            const result = await window.ipcRenderer.bossSync.reportWorkEvent({
                event_id: `workboard:${selectedItem.definitionId}:${executionId}`,
                event_date: localIsoDate(selectedItem.latestExecution?.updatedAt || selectedItem.updatedAt),
                task_type: selectedItem.kind,
                summary,
                material_count: materialCount,
                cost_cents: costCents,
                quality_score: qualityScore,
            });
            setBossSyncMessage(`已上报老板端：${result.event_date || '本次任务'}。再次提交会幂等更新同一条记录。`);
        } catch (reportError) {
            const message = reportError instanceof Error ? reportError.message : String(reportError);
            setBossSyncMessage(message);
            await appAlert(message, { title: '上报失败' });
        } finally {
            setActionState((current) => (
                current?.id === selectedItem.definitionId && current?.action === 'boss-report'
                    ? null
                    : current
            ));
        }
    }, [bossCostCents, bossMaterialCount, bossQualityScore, bossSummary, selectedItem]);

    const loadBossReviews = useCallback(async () => {
        try {
            setBossReviewsLoading(true);
            setBossSyncMessage('');
            const result = await window.ipcRenderer.bossSync.listWeekReviews(currentWeekRange());
            const reviews = Array.isArray(result.reviews) ? result.reviews : [];
            setBossReviews(reviews);
            setBossSyncMessage(reviews.length > 0 ? `已加载本周 ${reviews.length} 条老板反馈。` : '本周还没有老板反馈。');
        } catch (reviewError) {
            const message = reviewError instanceof Error ? reviewError.message : String(reviewError);
            setBossSyncMessage(message);
            await appAlert(message, { title: '读取老板反馈失败' });
        } finally {
            setBossReviewsLoading(false);
        }
    }, []);

    return (
        <div className="workboard-shell min-h-full bg-background text-text-primary xl:h-full xl:min-h-0">
            <div className="flex min-h-full flex-col gap-4 px-3 py-4 sm:px-6 sm:py-5 xl:h-full xl:min-h-0">
                <div className="flex flex-wrap items-start justify-between gap-3">
                    <div>
                        <div className="inline-flex items-center gap-1.5 rounded-full border border-border bg-surface-primary px-2.5 py-1 text-[11px] text-text-secondary">
                            <ListTodo className="h-3 w-3" />
                            商媒运营助手任务中心
                        </div>
                    </div>
                    <div className="flex flex-wrap items-center gap-2">
                        <div className="rounded-full border border-border bg-surface-primary px-2.5 py-1 text-[11px] text-text-secondary">
                            更新于 {formatDateTime(lastUpdatedAt)}
                        </div>
                        <button
                            onClick={() => void load()}
                            className="inline-flex h-[32px] items-center gap-1.5 rounded-full border border-border bg-surface-primary px-3 text-[11px] text-text-secondary shadow-[var(--ui-shadow-1)] hover:bg-surface-secondary"
                        >
                            <RefreshCw className={`h-3 w-3 ${loading ? 'animate-spin' : ''}`} />
                            刷新
                        </button>
                    </div>
                </div>

                <div className="overflow-x-auto pb-1">
                    <div className="flex min-w-max items-center gap-2.5">
                        <StatCard label="任务总数" value={topStats.totalDefinitions} />
                        <StatCard label="生成与记录" value={topStats.generation} />
                        <StatCard label="定时任务" value={topStats.scheduled} />
                        <StatCard label="长周期" value={topStats.longCycle} />
                        <StatCard label="已启用" value={topStats.active} />
                        <StatCard label="执行中" value={topStats.runningExecutions} />
                        <StatCard label="失败执行" value={topStats.failedExecutions} />
                    </div>
                </div>

                <div className="flex flex-wrap items-center gap-1.5">
                    {filterOptions.map((option) => (
                        <button
                            key={option.key}
                            onClick={() => setFilter(option.key)}
                            className={`rounded-full border px-3 py-1.5 text-[12px] transition ${
                                filter === option.key
                                    ? 'border-accent-primary/60 bg-accent-muted text-text-primary'
                                    : 'border-border bg-surface-primary text-text-secondary hover:bg-surface-secondary'
                            }`}
                        >
                            {option.label}
                            <span className="ml-2 text-xs opacity-70">{option.count}</span>
                        </button>
                    ))}
                </div>

                {error && (
                    <div className="inline-flex items-center gap-2 rounded-xl border border-status-error/30 bg-status-error/10 px-3 py-2.5 text-[13px] text-text-primary">
                        <AlertCircle className="h-3.5 w-3.5" />
                        {error}
                    </div>
                )}

                <div className="grid gap-2 md:grid-cols-2">
                    <div className="rounded-xl border border-status-success/30 bg-status-success/10 px-3.5 py-2.5 text-[12px] leading-5 text-text-secondary">
                        <span className="font-medium">长时间生成任务：</span>
                        支持应用内后台执行；应用退出后不会继续运行，当前任务历史也不会跨重启保留。
                    </div>
                    <div className="rounded-xl border border-status-warning/30 bg-status-warning/10 px-3.5 py-2.5 text-[12px] leading-5 text-text-secondary">
                        <span className="font-medium">定时与长周期任务：</span>
                        {runnerStatus?.capabilities?.automaticExecution === true
                            ? runnerStatus?.capabilities?.persistentDefinitions === true
                                ? 'Rust 自动调度器已启用，任务定义与执行记录会跨重启保留；应用进程退出期间不会执行。'
                                : '自动调度器已启用。'
                            : '目前仅支持创建和管理任务定义，自动调度执行器尚未接入，重启后不会保留。'}
                    </div>
                </div>

                <div className="flex-none xl:min-h-0 xl:flex-1 xl:overflow-hidden">
                    <div className="grid min-h-0 gap-3 xl:h-full xl:grid-cols-[minmax(320px,400px)_minmax(0,1fr)]">
                        <div className="min-h-[360px] overflow-hidden rounded-[24px] border border-border bg-surface-primary xl:min-h-0">
                            <div className="flex items-center justify-between border-b border-divider px-4 py-3">
                                <div>
                                    <div className="text-[13px] font-medium text-text-primary">任务列表</div>
                                    <div className="mt-0.5 text-[11px] text-text-secondary">按当前筛选展示统一任务定义</div>
                                </div>
                                <div className="text-[11px] text-text-tertiary">{filteredItems.length} 项</div>
                            </div>

                            <div className="max-h-[440px] overflow-y-auto px-2.5 py-2.5 xl:h-[calc(100%-61px)] xl:max-h-none">
                                {loading && items.length === 0 ? (
                                    <div className="flex h-full min-h-[240px] items-center justify-center text-[13px] text-text-secondary">
                                        <Loader2 className="mr-2 h-3.5 w-3.5 animate-spin" />
                                        正在加载任务列表
                                    </div>
                                ) : filteredItems.length === 0 ? (
                                    <div className="flex h-full min-h-[240px] items-center justify-center px-5 text-center text-[13px] leading-6 text-text-secondary">
                                        当前筛选下没有任务。你可以切换筛选查看其他任务状态。
                                    </div>
                                ) : (
                                    <div className="space-y-2.5">
                                        {filteredItems.map((item) => {
                                            const active = selectedItem?.definitionId === item.definitionId;
                                            const actionType = actionTypeLabel(item.actionType);
                                            return (
                                                <button
                                                    key={item.definitionId}
                                                    onClick={() => setSelectedId(item.definitionId)}
                                                    className={`w-full rounded-[18px] border px-3 py-2.5 text-left transition ${
                                                        active
                                                            ? 'border-accent-primary/60 bg-accent-muted/70 shadow-[var(--ui-shadow-1)]'
                                                            : 'border-border bg-surface-secondary/50 hover:border-accent-primary/40 hover:bg-surface-secondary'
                                                    }`}
                                                >
                                                    <div className="flex flex-wrap items-center gap-1.5">
                                                        <span className="rounded-full bg-accent-muted px-2 py-0.5 text-[10px] font-medium text-text-primary">
                                                            {kindLabel(item.kind)}
                                                        </span>
                                                        {actionType && (
                                                            <span className="rounded-full bg-surface-tertiary/60 px-2 py-0.5 text-[10px] font-medium text-text-secondary">
                                                                {actionType}
                                                            </span>
                                                        )}
                                                        <span className={`rounded-full px-2 py-0.5 text-[10px] font-medium ${lifecycleTone(item)}`}>
                                                            {lifecycleLabel(item)}
                                                        </span>
                                                    </div>

                                                    <div data-e2e-contrast="workboard-task-title" className="mt-2 truncate text-[13px] font-semibold text-text-primary">
                                                        {item.title}
                                                    </div>

                                                    <div className="mt-1.5 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[11px] text-text-secondary">
                                                        <span className="inline-flex items-center gap-1.5">
                                                            <Clock3 className="h-3 w-3" />
                                                            {triggerLabel(item)}
                                                        </span>
                                                        <span>下次 {formatDateTime(item.nextDueAt)}</span>
                                                        <span>策略 {policyLabel(item.policyDecision)}</span>
                                                        {item.latestExecution && (
                                                            <span>执行 {executionStatusLabel(item.latestExecution.status)}</span>
                                                        )}
                                                    </div>

                                                    {item.cooldown?.state === 'active' && (
                                                        <div className="mt-1.5 rounded-[14px] border border-status-error/30 bg-status-error/10 px-2.5 py-1.5 text-[10px] leading-4 text-text-primary">
                                                            冷却中：连续失败 {Number(item.cooldown.consecutiveFailures || 0)} 次。
                                                        </div>
                                                    )}
                                                </button>
                                            );
                                        })}
                                    </div>
                                )}
                            </div>
                        </div>

                        <div className="min-h-0 overflow-y-auto rounded-[24px] border border-border bg-surface-primary px-5 py-5">
                            {!selectedItem ? (
                                <div className="flex h-full min-h-[320px] items-center justify-center px-6 text-center text-[13px] leading-6 text-text-secondary">
                                    选择左侧任务后，这里会显示调度规则、策略信息和最近执行状态。
                                </div>
                            ) : (
                                <div className="space-y-5">
                                    <div className="flex flex-wrap items-start justify-between gap-3">
                                        <div>
                                            <div className="flex flex-wrap items-center gap-1.5">
                                                <span className="rounded-full bg-accent-muted px-2.5 py-0.5 text-[11px] font-medium text-text-primary">
                                                    {kindLabel(selectedItem.kind)}
                                                </span>
                                                {selectedItem.actionType && (
                                                    <span className="rounded-full bg-surface-tertiary/60 px-2.5 py-0.5 text-[11px] font-medium text-text-secondary">
                                                        {actionTypeLabel(selectedItem.actionType)}
                                                    </span>
                                                )}
                                                <span className={`rounded-full px-2.5 py-0.5 text-[11px] font-medium ${lifecycleTone(selectedItem)}`}>
                                                    {lifecycleLabel(selectedItem)}
                                                </span>
                                            </div>
                                            <h2 data-e2e-contrast="workboard-selected-title" className="mt-2.5 text-[21px] font-semibold tracking-[-0.03em] text-text-primary">
                                                {selectedItem.title}
                                            </h2>
                                            <p className="mt-1.5 max-w-[680px] text-[13px] leading-6 text-text-secondary">
                                                {taskContent(selectedItem)}
                                            </p>
                                        </div>

                                        <div className="flex flex-wrap items-center gap-1.5">
                                            {selectedItem.requiresConfirmation && selectedItem.draftId && (
                                                <>
                                                    <button
                                                        onClick={() => void executeAction(selectedItem, 'confirm', async () => {
                                                            await window.ipcRenderer.redclawRunner.taskConfirm({
                                                                draftId: selectedItem.draftId as string,
                                                                confirm: true,
                                                            });
                                                        })}
                                                        className="rounded-full border border-accent-primary/50 bg-accent-muted px-3.5 py-1.5 text-[12px] text-text-primary hover:bg-accent-muted/80"
                                                    >
                                                        {actionState?.id === selectedItem.definitionId && actionState?.action === 'confirm'
                                                            ? '确认中...'
                                                            : '确认任务'}
                                                    </button>
                                                    <button
                                                        onClick={() => void executeAction(selectedItem, 'discard', async () => {
                                                            await window.ipcRenderer.redclawRunner.taskConfirm({
                                                                draftId: selectedItem.draftId as string,
                                                                confirm: false,
                                                            });
                                                        })}
                                                        className="rounded-full border border-border bg-surface-primary px-3.5 py-1.5 text-[12px] text-text-secondary hover:bg-surface-secondary"
                                                    >
                                                        {actionState?.id === selectedItem.definitionId && actionState?.action === 'discard'
                                                            ? '处理中...'
                                                            : '丢弃草稿'}
                                                    </button>
                                                </>
                                            )}

                                            {!selectedItem.requiresConfirmation
                                                && ['scheduled', 'long_cycle'].includes(String(selectedItem.sourceKind || ''))
                                                && runnerStatus?.capabilities?.automaticExecution === true && (
                                                <button
                                                    onClick={() => void executeAction(selectedItem, 'run-now', () => runTaskNow(selectedItem))}
                                                    className="inline-flex items-center rounded-full border border-accent-primary/50 bg-accent-muted px-3.5 py-1.5 text-[12px] text-text-primary hover:bg-accent-muted/80"
                                                >
                                                    <Play className="mr-1.5 h-3.5 w-3.5" />
                                                    {actionState?.id === selectedItem.definitionId && actionState?.action === 'run-now'
                                                        ? '执行中...'
                                                        : '立即执行'}
                                                </button>
                                            )}

                                            {selectedItem.sourceKind === 'generation'
                                                && ['failed', 'cancelled', 'dead_lettered'].includes(String(selectedItem.latestExecution?.status || '')) && (
                                                <button
                                                    onClick={() => void executeAction(selectedItem, 'retry', () => runTaskNow(selectedItem))}
                                                    className="inline-flex items-center rounded-full border border-accent-primary/50 bg-accent-muted px-3.5 py-1.5 text-[12px] text-text-primary hover:bg-accent-muted/80"
                                                >
                                                    <Play className="mr-1.5 h-3.5 w-3.5" />
                                                    {actionState?.id === selectedItem.definitionId && actionState?.action === 'retry' ? '重试中...' : '重试任务'}
                                                </button>
                                            )}

                                            {selectedItem.sourceKind === 'generation'
                                                && ['queued', 'leased', 'running', 'retrying'].includes(String(selectedItem.latestExecution?.status || '')) && (
                                                <button
                                                    onClick={() => void executeAction(selectedItem, 'cancel', () => setTaskEnabled(selectedItem, false))}
                                                    className="rounded-full border border-border bg-surface-primary px-3.5 py-1.5 text-[12px] text-text-secondary hover:bg-surface-secondary"
                                                >
                                                    {actionState?.id === selectedItem.definitionId && actionState?.action === 'cancel' ? '处理中...' : '取消任务'}
                                                </button>
                                            )}

                                            {!selectedItem.requiresConfirmation
                                                && ['scheduled', 'long_cycle'].includes(String(selectedItem.sourceKind || ''))
                                                && selectedItem.enabled && (
                                                <button
                                                    onClick={() => void executeAction(selectedItem, 'pause', () => setTaskEnabled(selectedItem, false))}
                                                    className="rounded-full border border-border bg-surface-primary px-3.5 py-1.5 text-[12px] text-text-secondary hover:bg-surface-secondary"
                                                >
                                                    {actionState?.id === selectedItem.definitionId && actionState?.action === 'pause'
                                                        ? '处理中...'
                                                        : '停用任务'}
                                                </button>
                                            )}

                                            {!selectedItem.requiresConfirmation
                                                && ['scheduled', 'long_cycle'].includes(String(selectedItem.sourceKind || ''))
                                                && !selectedItem.enabled && (
                                                <button
                                                    onClick={() => void executeAction(selectedItem, 'resume', () => setTaskEnabled(selectedItem, true))}
                                                    className="rounded-full border border-border bg-surface-primary px-3.5 py-1.5 text-[12px] text-text-secondary hover:bg-surface-secondary"
                                                >
                                                    {actionState?.id === selectedItem.definitionId && actionState?.action === 'resume'
                                                        ? '处理中...'
                                                        : '恢复任务'}
                                                </button>
                                            )}
                                        </div>
                                    </div>

                                    <section className="rounded-[20px] border border-status-success/30 bg-status-success/10 px-4 py-4">
                                        <div className="flex flex-wrap items-start justify-between gap-3">
                                            <div>
                                                <div className="text-[13px] font-medium text-text-primary">老板端工作闭环</div>
                                                <div className="mt-1 text-[11px] leading-5 text-text-secondary">
                                                    由员工端 Rust 后端安全连接；只会在你点击后上报成功任务。
                                                </div>
                                            </div>
                                            <button
                                                type="button"
                                                onClick={() => void loadBossReviews()}
                                                disabled={bossReviewsLoading}
                                                className="inline-flex items-center rounded-full border border-border bg-surface-primary px-3.5 py-1.5 text-[12px] text-text-secondary hover:bg-surface-secondary disabled:cursor-wait disabled:opacity-60"
                                            >
                                                <MessageSquareText className="mr-1.5 h-3.5 w-3.5" />
                                                {bossReviewsLoading ? '读取中...' : '查看老板反馈'}
                                            </button>
                                        </div>

                                        {isCompletedForBossReport(selectedItem) ? (
                                            <div className="mt-3 space-y-3">
                                                <label className="block">
                                                    <span className="text-[11px] font-medium text-text-primary">工作摘要（提交前可编辑）</span>
                                                    <textarea
                                                        value={bossSummary}
                                                        onChange={(event) => setBossSummary(event.target.value)}
                                                        rows={3}
                                                        className="mt-1.5 w-full resize-y rounded-[14px] border border-border bg-surface-primary px-3 py-2 text-[13px] leading-5 text-text-primary outline-none placeholder:text-text-tertiary focus:border-accent-primary"
                                                        placeholder="说明完成内容、结果与需要老板关注的事项"
                                                    />
                                                </label>
                                                <div className="grid gap-2 sm:grid-cols-3">
                                                    <label className="text-[11px] text-text-secondary">
                                                        产出件数
                                                        <input
                                                            type="number"
                                                            min="0"
                                                            step="1"
                                                            value={bossMaterialCount}
                                                            onChange={(event) => setBossMaterialCount(event.target.value)}
                                                            className="mt-1 w-full rounded-xl border border-border bg-surface-primary px-3 py-1.5 text-[12px] text-text-primary outline-none focus:border-accent-primary"
                                                        />
                                                    </label>
                                                    <label className="text-[11px] text-text-secondary">
                                                        质量评分（0 表示未评）
                                                        <input
                                                            type="number"
                                                            min="0"
                                                            max="100"
                                                            step="1"
                                                            value={bossQualityScore}
                                                            onChange={(event) => setBossQualityScore(event.target.value)}
                                                            className="mt-1 w-full rounded-xl border border-border bg-surface-primary px-3 py-1.5 text-[12px] text-text-primary outline-none focus:border-accent-primary"
                                                        />
                                                    </label>
                                                    <label className="text-[11px] text-text-secondary">
                                                        成本（分）
                                                        <input
                                                            type="number"
                                                            min="0"
                                                            step="1"
                                                            value={bossCostCents}
                                                            onChange={(event) => setBossCostCents(event.target.value)}
                                                            className="mt-1 w-full rounded-xl border border-border bg-surface-primary px-3 py-1.5 text-[12px] text-text-primary outline-none focus:border-accent-primary"
                                                        />
                                                    </label>
                                                </div>
                                                <div className="flex flex-wrap items-center justify-between gap-2">
                                                    <div className="text-[10px] leading-4 text-text-tertiary">
                                                        需配置 BOSS_OWNER_CODE、EMPLOYEE_ID、EMPLOYEE_NAME；远程连接另需 HTTPS 与 32+ 字节的 BOSS_EMPLOYEE_TOKEN。
                                                    </div>
                                                    <button
                                                        type="button"
                                                        onClick={() => void reportSelectedTask()}
                                                        disabled={actionState?.id === selectedItem.definitionId && actionState?.action === 'boss-report'}
                                                        className="inline-flex items-center rounded-full border border-accent-primary/50 bg-accent-muted px-3.5 py-1.5 text-[12px] font-medium text-text-primary hover:bg-accent-muted/80 disabled:cursor-wait disabled:opacity-60"
                                                    >
                                                        <Send className="mr-1.5 h-3.5 w-3.5" />
                                                        {actionState?.id === selectedItem.definitionId && actionState?.action === 'boss-report'
                                                            ? '上报中...'
                                                            : '上报老板'}
                                                    </button>
                                                </div>
                                            </div>
                                        ) : (
                                            <div className="mt-3 rounded-[14px] border border-border bg-surface-primary px-3 py-2 text-[11px] leading-5 text-text-secondary">
                                                当前任务没有成功完成的执行记录。草稿、失败、取消和未执行任务不能上报。
                                            </div>
                                        )}

                                        {bossSyncMessage && (
                                            <div className="mt-3 rounded-[14px] border border-border bg-surface-primary px-3 py-2 text-[11px] leading-5 text-text-secondary">
                                                {bossSyncMessage}
                                            </div>
                                        )}

                                        {bossReviews.length > 0 && (
                                            <div className="mt-3 space-y-2">
                                                {bossReviews.map((review) => (
                                                    <div key={review.id} className="rounded-[14px] border border-border bg-surface-primary px-3 py-2.5">
                                                        <div className="flex flex-wrap items-center justify-between gap-2 text-[10px] text-text-tertiary">
                                                            <span>{review.week_start} 至 {review.week_end}</span>
                                                            <span className="rounded-full bg-status-success/15 px-2 py-0.5 font-medium text-text-primary">
                                                                {reviewStatusLabel(review.status)}
                                                            </span>
                                                        </div>
                                                        <div className="mt-1.5 text-[12px] leading-5 text-text-primary">
                                                            {review.note || '老板未填写补充说明。'}
                                                        </div>
                                                    </div>
                                                ))}
                                            </div>
                                        )}
                                    </section>

                                    <div className="grid gap-2.5 md:grid-cols-2 xl:grid-cols-3">
                                        <DetailRow label="任务分类" value={kindLabel(selectedItem.kind)} />
                                        <DetailRow label="调度方式" value={triggerLabel(selectedItem)} />
                                        <DetailRow label="策略判定" value={policyLabel(selectedItem.policyDecision)} />
                                        <DetailRow label="任务时区" value={selectedItem.timezone || 'local'} />
                                        <DetailRow label="错过窗口策略" value={selectedItem.missedRunPolicy || 'single'} />
                                        <DetailRow label="任务指纹" value={shortFingerprint(selectedItem.definitionFingerprint)} />
                                    </div>

                                    <div className="grid gap-3 xl:grid-cols-[minmax(0,1.3fr)_minmax(260px,0.9fr)]">
                                        <div className="space-y-3">
                                            <section className="rounded-[20px] border border-border bg-surface-secondary/60 px-4 py-4">
                                                <div className="text-[13px] font-medium text-text-primary">任务内容</div>
                                                <div className="mt-3 space-y-2.5 text-[13px] leading-6 text-text-secondary">
                                                    {selectedItem.goal && (
                                                        <div>
                                                            <div className="text-[10px] uppercase tracking-[0.16em] text-text-tertiary">Goal</div>
                                                            <div className="mt-1">{selectedItem.goal}</div>
                                                        </div>
                                                    )}
                                                    {selectedItem.prompt && (
                                                        <div>
                                                            <div className="text-[10px] uppercase tracking-[0.16em] text-text-tertiary">Prompt</div>
                                                            <div className="mt-1">{selectedItem.prompt}</div>
                                                        </div>
                                                    )}
                                                    {selectedItem.objective && (
                                                        <div>
                                                            <div className="text-[10px] uppercase tracking-[0.16em] text-text-tertiary">Objective</div>
                                                            <div className="mt-1">{selectedItem.objective}</div>
                                                        </div>
                                                    )}
                                                    {selectedItem.stepPrompt && (
                                                        <div>
                                                            <div className="text-[10px] uppercase tracking-[0.16em] text-text-tertiary">Step Prompt</div>
                                                            <div className="mt-1">{selectedItem.stepPrompt}</div>
                                                        </div>
                                                    )}
                                                    {!selectedItem.goal && !selectedItem.prompt && !selectedItem.objective && !selectedItem.stepPrompt && (
                                                        <div>当前任务没有更多结构化内容。</div>
                                                    )}
                                                </div>
                                            </section>

                                            <section className="rounded-[20px] border border-border bg-surface-secondary/60 px-4 py-4">
                                                <div className="text-[13px] font-medium text-text-primary">策略与风险</div>
                                                <div className="mt-3 space-y-2.5 text-[13px] leading-6 text-text-secondary">
                                                    <div>策略结论：{policyLabel(selectedItem.policyDecision)}</div>
                                                    {Array.isArray(selectedItem.policyWarnings) && selectedItem.policyWarnings.length > 0 && (
                                                        <div>
                                                            <div className="text-[10px] uppercase tracking-[0.16em] text-text-tertiary">Warnings</div>
                                                                <div className="mt-1 space-y-1">
                                                                    {selectedItem.policyWarnings.map((warning: unknown, index: number) => (
                                                                        <div key={`${selectedItem.definitionId}-warning-${index}`}>- {String(warning || '')}</div>
                                                                    ))}
                                                                </div>
                                                            </div>
                                                    )}
                                                    {selectedItem.riskRationale && (
                                                        <div>
                                                            <div className="text-[10px] uppercase tracking-[0.16em] text-text-tertiary">Risk Rationale</div>
                                                            <div className="mt-1">{selectedItem.riskRationale}</div>
                                                        </div>
                                                    )}
                                                    {selectedItem.lastUpdatedReason && (
                                                        <div>
                                                            <div className="text-[10px] uppercase tracking-[0.16em] text-text-tertiary">Last Updated Reason</div>
                                                            <div className="mt-1">{selectedItem.lastUpdatedReason}</div>
                                                        </div>
                                                    )}
                                                </div>
                                            </section>
                                        </div>

                                        <div className="space-y-3">
                                            <section className="rounded-[20px] border border-border bg-surface-secondary/60 px-4 py-4">
                                                <div className="text-[13px] font-medium text-text-primary">调度信息</div>
                                                <div className="mt-3 space-y-2.5 text-[13px] leading-6 text-text-secondary">
                                                    <div>{scheduleSummary(selectedItem)}</div>
                                                    <div>创建于 {formatDateTime(selectedItem.createdAt)}</div>
                                                    <div>更新于 {formatDateTime(selectedItem.updatedAt)}</div>
                                                    {selectedItem.kind === 'long_cycle' && (
                                                        <div>
                                                            轮次进度 {Number(selectedItem.completedRounds || 0)} / {Number(selectedItem.totalRounds || 0)}
                                                        </div>
                                                    )}
                                                </div>
                                            </section>

                                            <section className="rounded-[20px] border border-border bg-surface-secondary/60 px-4 py-4">
                                                <div className="text-[13px] font-medium text-text-primary">最近执行</div>
                                                <div className="mt-3 space-y-2.5 text-[13px] leading-6 text-text-secondary">
                                                    {selectedItem.latestExecution ? (
                                                        <>
                                                            <div>状态：{executionStatusLabel(selectedItem.latestExecution.status)}</div>
                                                            <div>计划时间：{formatDateTime(selectedItem.latestExecution.scheduledForAt)}</div>
                                                            <div>最近心跳：{formatDateTime(selectedItem.latestExecution.lastHeartbeatAt)}</div>
                                                            <div>尝试次数：{Number(selectedItem.latestExecution.attemptNo || 0)}</div>
                                                            {selectedItem.latestExecution.lastError && (
                                                                <div className="rounded-[16px] border border-status-error/30 bg-status-error/10 px-3 py-2 text-[11px] leading-5 text-text-primary">
                                                                    {selectedItem.latestExecution.lastError}
                                                                </div>
                                                            )}
                                                        </>
                                                    ) : (
                                                        <div>当前还没有执行记录。</div>
                                                    )}
                                                </div>
                                            </section>

                                            <section className="rounded-[20px] border border-border bg-surface-secondary/60 px-4 py-4">
                                                <div className="text-[13px] font-medium text-text-primary">冷却状态</div>
                                                <div className="mt-3 text-[13px] leading-6 text-text-secondary">
                                                    {selectedItem.cooldown?.state === 'active' ? (
                                                        <div>
                                                            连续失败 {Number(selectedItem.cooldown.consecutiveFailures || 0)} 次，
                                                            激活于 {formatDateTime(selectedItem.cooldown.activatedAt)}，
                                                            原因为 {selectedItem.cooldown.reason || '连续失败进入冷却'}。
                                                        </div>
                                                    ) : (
                                                        <div>当前没有进入冷却。</div>
                                                    )}
                                                </div>
                                            </section>
                                        </div>
                                    </div>
                                </div>
                            )}
                        </div>
                    </div>
                </div>
            </div>
        </div>
    );
}
