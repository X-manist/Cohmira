import { useState } from 'react';
import {
    CalendarClock,
    CheckCircle2,
    Clock3,
    Loader2,
    Pause,
    Play,
    RefreshCw,
    Settings2,
    Trash2,
    X,
} from 'lucide-react';
import { clsx } from 'clsx';
import { LONG_TEMPLATES, SCHEDULE_TEMPLATES, WEEKDAY_OPTIONS } from './config';
import type {
    LongDraft,
    RunnerLongCycleTask,
    RunnerScheduledTask,
    RunnerStatus,
    ScheduleDraft,
    ScheduleMode,
} from './types';

type AutomationTab = 'scheduled' | 'long-cycle' | 'settings';
type DraftPatch<T> = Partial<T>;

interface RedClawAutomationDrawerProps {
    open: boolean;
    status: RunnerStatus | null;
    loading: boolean;
    message: string;
    scheduledTasks: RunnerScheduledTask[];
    longTasks: RunnerLongCycleTask[];
    scheduleDraft: ScheduleDraft;
    scheduleAdvanced: boolean;
    isAddingSchedule: boolean;
    longDraft: LongDraft;
    longAdvanced: boolean;
    isAddingLong: boolean;
    runnerMaxAutomationPerTick: number;
    onOpen: () => void;
    onClose: () => void;
    onRefresh: () => void | Promise<void>;
    onRunnerToggle: () => void | Promise<void>;
    onScheduleDraftChange: (patch: DraftPatch<ScheduleDraft>) => void;
    onScheduleAdvancedChange: (value: boolean) => void;
    onApplyScheduleTemplate: (templateId: string) => void;
    onAddSchedule: () => void | Promise<void>;
    onToggleSchedule: (task: RunnerScheduledTask) => void | Promise<void>;
    onRemoveSchedule: (taskId: string) => void | Promise<void>;
    onLongDraftChange: (patch: DraftPatch<LongDraft>) => void;
    onLongAdvancedChange: (value: boolean) => void;
    onApplyLongTemplate: (templateId: string) => void;
    onAddLong: () => void | Promise<void>;
    onToggleLong: (task: RunnerLongCycleTask) => void | Promise<void>;
    onRemoveLong: (taskId: string) => void | Promise<void>;
    onRunnerMaxAutomationPerTickChange: (value: number) => void;
    onSaveRunnerConfig: () => void | Promise<void>;
}

const fieldClass = 'w-full rounded-xl border border-border bg-surface-primary px-3 py-2 text-[12px] text-text-primary outline-none placeholder:text-text-tertiary focus:border-accent-primary/70 focus:ring-2 focus:ring-accent-muted';
const labelClass = 'mb-1.5 block text-[11px] font-medium text-text-secondary';

const hasHighRiskTool = (value: string): boolean => value
    .split(/[,，\s]+/)
    .some((rawName) => {
        const name = (rawName.includes('__') ? rawName.split('__').pop() || rawName : rawName).toLowerCase();
        return ['generate_image', 'image_gen', 'image_selector', 'google_imagen', 'seedance_video', 'video_selector', 'video_stitch', 'talking_head', 'lip_sync', 'tts_selector', 'music_gen', 'upload_video', 'upload_note'].includes(name)
            || name.startsWith('publish_')
            || name.endsWith('_publish')
            || name.startsWith('upload_')
            || name.startsWith('generate_')
            || name.endsWith('_image')
            || name.endsWith('_video')
            || name.endsWith('_tts')
            || name.endsWith('_music');
    });

export function defaultOnceRunAtLocal(now = Date.now()): string {
    const date = new Date(now + 5 * 60_000);
    const localTime = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
    return localTime.toISOString().slice(0, 16);
}

export function formatRunnerDateTime(value?: string): string {
    if (!value) return '尚未安排';
    const parsed = new Date(value);
    if (Number.isNaN(parsed.getTime())) return value;
    return parsed.toLocaleString('zh-CN', { hour12: false });
}

function CapabilityBadge({ label, enabled, negativeLabel }: { label: string; enabled: boolean; negativeLabel: string }) {
    return (
        <div className={clsx(
            'rounded-xl border px-3 py-2',
            enabled
                ? 'border-status-success/30 bg-status-success/10'
                : 'border-status-warning/30 bg-status-warning/10',
        )}>
            <div className="flex items-center gap-1.5 text-[11px] font-medium text-text-primary">
                {enabled ? <CheckCircle2 className="h-3.5 w-3.5 text-status-success" /> : <Clock3 className="h-3.5 w-3.5 text-status-warning" />}
                {label}
            </div>
            <div className="mt-1 text-[10px] leading-4 text-text-secondary">{enabled ? '支持' : negativeLabel}</div>
        </div>
    );
}

function TaskResult({ task }: { task: RunnerScheduledTask | RunnerLongCycleTask }) {
    if (!task.lastRunAt && !task.lastError && !task.lastOutput) return null;
    return (
        <div className={clsx(
            'mt-2 rounded-lg border px-2.5 py-2 text-[10px] leading-4',
            task.lastResult === 'error'
                ? 'border-status-error/30 bg-status-error/10 text-text-primary'
                : 'border-border bg-surface-secondary text-text-secondary',
        )}>
            <div>上次执行：{formatRunnerDateTime(task.lastRunAt)}</div>
            {task.lastError && <div className="mt-1 break-words text-status-error">失败：{task.lastError}</div>}
            {!task.lastError && task.lastOutput && <div className="mt-1 line-clamp-3 break-words">{task.lastOutput}</div>}
            {task.lastReceiptPath && <div className="mt-1 break-all text-text-tertiary">回执：{task.lastReceiptPath.split(/[\\/]/).pop()}</div>}
        </div>
    );
}

function ScheduledTaskList({
    tasks,
    loading,
    onToggle,
    onRemove,
}: {
    tasks: RunnerScheduledTask[];
    loading: boolean;
    onToggle: (task: RunnerScheduledTask) => void | Promise<void>;
    onRemove: (taskId: string) => void | Promise<void>;
}) {
    if (tasks.length === 0) {
        return <div className="rounded-xl border border-dashed border-border px-4 py-6 text-center text-[11px] text-text-tertiary">还没有定时任务</div>;
    }
    return (
        <div className="space-y-2">
            {tasks.map((task) => (
                <div key={task.id} className="rounded-xl border border-border bg-surface-secondary/60 p-3">
                    <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0">
                            <div className="truncate text-[12px] font-semibold text-text-primary">{task.name}</div>
                            <div className="mt-1 text-[10px] text-text-secondary">
                                {task.mode === 'once' ? '单次' : task.mode === 'interval' ? `每 ${task.intervalMinutes} 分钟` : task.mode === 'daily' ? `每天 ${task.time}` : `每周 ${task.time}`}
                                {' · '}下次 {formatRunnerDateTime(task.nextRunAt)}
                            </div>
                            {task.requiredTools && task.requiredTools.length > 0 && (
                                <div className="mt-1 text-[10px] text-text-tertiary">必须调用：{task.requiredTools.join(', ')}</div>
                            )}
                        </div>
                        <div className="flex shrink-0 items-center gap-1">
                            <button
                                type="button"
                                disabled={loading}
                                onClick={() => void onToggle(task)}
                                aria-label={`${task.enabled ? '停用' : '启用'}定时任务 ${task.name}`}
                                className="rounded-lg border border-border bg-surface-primary p-1.5 text-text-secondary hover:bg-surface-tertiary hover:text-text-primary disabled:opacity-40"
                            >
                                {task.enabled ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
                            </button>
                            <button
                                type="button"
                                disabled={loading}
                                onClick={() => void onRemove(task.id)}
                                aria-label={`删除定时任务 ${task.name}`}
                                className="rounded-lg border border-border bg-surface-primary p-1.5 text-text-secondary hover:border-status-error/40 hover:bg-status-error/10 hover:text-status-error disabled:opacity-40"
                            >
                                <Trash2 className="h-3.5 w-3.5" />
                            </button>
                        </div>
                    </div>
                    <TaskResult task={task} />
                </div>
            ))}
        </div>
    );
}

function LongTaskList({
    tasks,
    loading,
    onToggle,
    onRemove,
}: {
    tasks: RunnerLongCycleTask[];
    loading: boolean;
    onToggle: (task: RunnerLongCycleTask) => void | Promise<void>;
    onRemove: (taskId: string) => void | Promise<void>;
}) {
    if (tasks.length === 0) {
        return <div className="rounded-xl border border-dashed border-border px-4 py-6 text-center text-[11px] text-text-tertiary">还没有长周期任务</div>;
    }
    return (
        <div className="space-y-2">
            {tasks.map((task) => (
                <div key={task.id} className="rounded-xl border border-border bg-surface-secondary/60 p-3">
                    <div className="flex items-start justify-between gap-3">
                        <div className="min-w-0">
                            <div className="truncate text-[12px] font-semibold text-text-primary">{task.name}</div>
                            <div className="mt-1 text-[10px] text-text-secondary">
                                第 {task.completedRounds}/{task.totalRounds} 轮 · 每 {task.intervalMinutes} 分钟 · 下次 {formatRunnerDateTime(task.nextRunAt)}
                            </div>
                            {task.requiredTools && task.requiredTools.length > 0 && (
                                <div className="mt-1 text-[10px] text-text-tertiary">每轮必须调用：{task.requiredTools.join(', ')}</div>
                            )}
                        </div>
                        <div className="flex shrink-0 items-center gap-1">
                            <button
                                type="button"
                                disabled={loading || task.status === 'completed'}
                                onClick={() => void onToggle(task)}
                                aria-label={`${task.enabled ? '暂停' : '继续'}长周期任务 ${task.name}`}
                                className="rounded-lg border border-border bg-surface-primary p-1.5 text-text-secondary hover:bg-surface-tertiary hover:text-text-primary disabled:opacity-40"
                            >
                                {task.enabled ? <Pause className="h-3.5 w-3.5" /> : <Play className="h-3.5 w-3.5" />}
                            </button>
                            <button
                                type="button"
                                disabled={loading}
                                onClick={() => void onRemove(task.id)}
                                aria-label={`删除长周期任务 ${task.name}`}
                                className="rounded-lg border border-border bg-surface-primary p-1.5 text-text-secondary hover:border-status-error/40 hover:bg-status-error/10 hover:text-status-error disabled:opacity-40"
                            >
                                <Trash2 className="h-3.5 w-3.5" />
                            </button>
                        </div>
                    </div>
                    <p className="mt-2 line-clamp-2 text-[10px] leading-4 text-text-secondary">{task.objective}</p>
                    <TaskResult task={task} />
                </div>
            ))}
        </div>
    );
}

export function RedClawAutomationDrawer(props: RedClawAutomationDrawerProps) {
    const [activeTab, setActiveTab] = useState<AutomationTab>('scheduled');
    const capabilities = props.status?.capabilities;
    const updateScheduleMode = (mode: ScheduleMode) => {
        props.onScheduleDraftChange({
            mode,
            ...(mode === 'once' && !props.scheduleDraft.runAtLocal
                ? { runAtLocal: defaultOnceRunAtLocal() }
                : {}),
        });
    };

    return (
        <>
            <button
                type="button"
                onClick={props.onOpen}
                className={clsx(
                    'absolute right-[92px] top-4 z-30 flex items-center gap-2 rounded-xl border px-3.5 py-1.5 text-[12px] font-bold shadow-sm backdrop-blur-xl transition-all active:scale-95',
                    props.open
                        ? 'border-transparent bg-accent-primary text-on-accent'
                        : 'border-border/80 bg-surface-elevated/92 text-text-secondary hover:bg-surface-primary hover:text-text-primary',
                )}
                aria-label="打开自动化任务面板"
            >
                <CalendarClock className="h-3.5 w-3.5" />
                <span>自动化</span>
            </button>

            {props.open && (
                <div className="absolute inset-0 z-50">
                    <button type="button" className="absolute inset-0 bg-black/30 backdrop-blur-[2px]" aria-label="关闭自动化任务面板" onClick={props.onClose} />
                    <section
                        role="dialog"
                        aria-modal="true"
                        aria-labelledby="redclaw-automation-title"
                        className="absolute bottom-4 right-4 top-4 flex w-[560px] max-w-[calc(100%-2rem)] flex-col overflow-hidden rounded-2xl border border-border bg-surface-primary shadow-[var(--ui-shadow-3)]"
                    >
                        <header className="border-b border-divider px-5 pb-4 pt-5">
                            <div className="flex items-start justify-between gap-3">
                                <div>
                                    <h2 id="redclaw-automation-title" className="text-[16px] font-semibold text-text-primary">自动化任务</h2>
                                    <p className="mt-1 text-[11px] leading-5 text-text-secondary">创建后由 Rust 调度器按时真实执行，结果与回执会保留在任务记录中。</p>
                                </div>
                                <div className="flex items-center gap-1.5">
                                    <button type="button" onClick={() => void props.onRefresh()} disabled={props.loading} aria-label="刷新自动化状态" className="rounded-lg p-2 text-text-secondary hover:bg-surface-secondary hover:text-text-primary disabled:opacity-40">
                                        <RefreshCw className={clsx('h-4 w-4', props.loading && 'animate-spin')} />
                                    </button>
                                    <button type="button" onClick={props.onClose} aria-label="关闭自动化任务面板" className="rounded-lg p-2 text-text-secondary hover:bg-surface-secondary hover:text-text-primary">
                                        <X className="h-4 w-4" />
                                    </button>
                                </div>
                            </div>

                            <div className="mt-4 grid grid-cols-1 gap-2 sm:grid-cols-3">
                                <CapabilityBadge label="自动执行" enabled={capabilities?.automaticExecution === true} negativeLabel="未接入" />
                                <CapabilityBadge label="跨重启保留" enabled={capabilities?.persistentDefinitions === true} negativeLabel="不保留" />
                                <CapabilityBadge label="退出后运行" enabled={capabilities?.survivesAppExit === true} negativeLabel="需保持应用运行" />
                            </div>
                            {capabilities?.statusText && <p className="mt-2 text-[10px] leading-4 text-text-tertiary">{capabilities.statusText}</p>}
                            {props.message && (
                                <div role="status" className="mt-3 rounded-xl border border-accent-primary/25 bg-accent-muted px-3 py-2 text-[11px] text-text-primary">
                                    {props.message}
                                </div>
                            )}
                        </header>

                        <nav aria-label="自动化任务类型" className="flex gap-1 border-b border-divider px-5 py-2">
                            {([
                                ['scheduled', '定时任务', CalendarClock],
                                ['long-cycle', '长周期', RefreshCw],
                                ['settings', '运行设置', Settings2],
                            ] as const).map(([key, label, Icon]) => (
                                <button
                                    key={key}
                                    type="button"
                                    onClick={() => setActiveTab(key)}
                                    aria-current={activeTab === key ? 'page' : undefined}
                                    className={clsx(
                                        'inline-flex items-center gap-1.5 rounded-lg px-3 py-2 text-[11px] font-medium transition',
                                        activeTab === key ? 'bg-accent-muted text-text-primary' : 'text-text-secondary hover:bg-surface-secondary hover:text-text-primary',
                                    )}
                                >
                                    <Icon className="h-3.5 w-3.5" />
                                    {label}
                                </button>
                            ))}
                        </nav>

                        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
                            {activeTab === 'scheduled' && (
                                <div className="space-y-5">
                                    <section aria-labelledby="scheduled-create-title" className="rounded-2xl border border-border bg-surface-elevated p-4">
                                        <h3 id="scheduled-create-title" className="text-[13px] font-semibold text-text-primary">新建定时任务</h3>
                                        <div className="mt-3 grid gap-3 sm:grid-cols-2">
                                            <label>
                                                <span className={labelClass}>模板</span>
                                                <select aria-label="定时任务模板" value={props.scheduleDraft.templateId} onChange={(event) => props.onApplyScheduleTemplate(event.target.value)} className={fieldClass}>
                                                    {SCHEDULE_TEMPLATES.map((template) => <option key={template.id} value={template.id}>{template.label}</option>)}
                                                </select>
                                            </label>
                                            <label>
                                                <span className={labelClass}>执行方式</span>
                                                <select aria-label="定时任务执行方式" value={props.scheduleDraft.mode} onChange={(event) => updateScheduleMode(event.target.value as ScheduleMode)} className={fieldClass}>
                                                    <option value="once">单次执行</option>
                                                    <option value="interval">固定间隔</option>
                                                    <option value="daily">每天</option>
                                                    <option value="weekly">每周</option>
                                                </select>
                                            </label>
                                        </div>

                                        <div className="mt-3">
                                            {props.scheduleDraft.mode === 'once' && (
                                                <label>
                                                    <span className={labelClass}>本地执行日期和时间</span>
                                                    <input aria-label="单次任务执行时间" type="datetime-local" value={props.scheduleDraft.runAtLocal} min={defaultOnceRunAtLocal(Date.now() - 4 * 60_000)} onChange={(event) => props.onScheduleDraftChange({ runAtLocal: event.target.value })} className={fieldClass} />
                                                </label>
                                            )}
                                            {props.scheduleDraft.mode === 'interval' && (
                                                <label>
                                                    <span className={labelClass}>间隔分钟（1–43200）</span>
                                                    <input aria-label="定时任务间隔分钟" type="number" min={1} max={43200} value={props.scheduleDraft.intervalMinutes} onChange={(event) => props.onScheduleDraftChange({ intervalMinutes: Number(event.target.value) })} className={fieldClass} />
                                                </label>
                                            )}
                                            {(props.scheduleDraft.mode === 'daily' || props.scheduleDraft.mode === 'weekly') && (
                                                <label>
                                                    <span className={labelClass}>每天本地执行时间</span>
                                                    <input aria-label="每日任务执行时间" type="time" value={props.scheduleDraft.time} onChange={(event) => props.onScheduleDraftChange({ time: event.target.value })} className={fieldClass} />
                                                </label>
                                            )}
                                        </div>

                                        {props.scheduleDraft.mode === 'weekly' && (
                                            <fieldset className="mt-3">
                                                <legend className={labelClass}>每周执行日</legend>
                                                <div className="flex flex-wrap gap-1.5">
                                                    {WEEKDAY_OPTIONS.map((weekday) => {
                                                        const selected = props.scheduleDraft.weekdays.includes(weekday.value);
                                                        return (
                                                            <button
                                                                key={weekday.value}
                                                                type="button"
                                                                aria-pressed={selected}
                                                                onClick={() => props.onScheduleDraftChange({ weekdays: selected ? props.scheduleDraft.weekdays.filter((value) => value !== weekday.value) : [...props.scheduleDraft.weekdays, weekday.value] })}
                                                                className={clsx('rounded-full border px-2.5 py-1 text-[10px]', selected ? 'border-accent-primary/60 bg-accent-muted text-text-primary' : 'border-border bg-surface-primary text-text-secondary')}
                                                            >
                                                                {weekday.label}
                                                            </button>
                                                        );
                                                    })}
                                                </div>
                                            </fieldset>
                                        )}

                                        <label className="mt-3 block">
                                            <span className={labelClass}>到点后执行的指令</span>
                                            <textarea aria-label="定时任务指令" rows={4} value={props.scheduleDraft.prompt} onChange={(event) => props.onScheduleDraftChange({ prompt: event.target.value })} className={fieldClass} />
                                        </label>

                                        <button type="button" onClick={() => props.onScheduleAdvancedChange(!props.scheduleAdvanced)} className="mt-2 text-[10px] text-accent-primary hover:underline">
                                            {props.scheduleAdvanced ? '收起任务名称' : '设置任务名称'}
                                        </button>
                                        {props.scheduleAdvanced && (
                                            <div className="mt-2 space-y-3">
                                                <label className="block">
                                                    <span className={labelClass}>任务名称</span>
                                                    <input aria-label="定时任务名称" value={props.scheduleDraft.name} onChange={(event) => props.onScheduleDraftChange({ name: event.target.value })} className={fieldClass} />
                                                </label>
                                                <label className="block">
                                                    <span className={labelClass}>必须且只允许调用的工具（逗号分隔）</span>
                                                    <input aria-label="定时任务必须调用的工具" value={props.scheduleDraft.requiredToolsText} onChange={(event) => props.onScheduleDraftChange({ requiredToolsText: event.target.value })} placeholder="generate_image, create_note" className={fieldClass} />
                                                    <span className="mt-1 block text-[10px] leading-4 text-text-tertiary">声明后，未真实调用指定工具会被记为失败，不能用纯文本冒充完成。</span>
                                                </label>
                                            </div>
                                        )}

                                        {hasHighRiskTool(props.scheduleDraft.requiredToolsText) && (
                                            <label className="mt-3 flex items-start gap-2 rounded-xl border border-status-warning/30 bg-status-warning/10 px-3 py-2 text-[10px] leading-4 text-text-secondary">
                                                <input aria-label="确认定时任务真实操作授权" type="checkbox" checked={props.scheduleDraft.realOperationConfirmed} onChange={(event) => props.onScheduleDraftChange({ realOperationConfirmed: event.target.checked })} className="mt-0.5 h-3.5 w-3.5 accent-accent-primary" />
                                                <span>我确认该任务会真实调用生成或发布服务，可能产生费用或外部平台副作用。</span>
                                            </label>
                                        )}

                                        <button type="button" disabled={props.isAddingSchedule || props.loading} onClick={() => void props.onAddSchedule()} className="mt-4 inline-flex w-full items-center justify-center gap-2 rounded-xl bg-accent-primary px-4 py-2.5 text-[12px] font-semibold text-on-accent hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50">
                                            {props.isAddingSchedule ? <Loader2 className="h-4 w-4 animate-spin" /> : <CalendarClock className="h-4 w-4" />}
                                            {props.isAddingSchedule ? '正在创建...' : '创建并启用定时任务'}
                                        </button>
                                    </section>

                                    <section aria-labelledby="scheduled-list-title">
                                        <div className="mb-2 flex items-center justify-between">
                                            <h3 id="scheduled-list-title" className="text-[12px] font-semibold text-text-primary">现有定时任务</h3>
                                            <span className="text-[10px] text-text-tertiary">{props.scheduledTasks.length} 项</span>
                                        </div>
                                        <ScheduledTaskList tasks={props.scheduledTasks} loading={props.loading} onToggle={props.onToggleSchedule} onRemove={props.onRemoveSchedule} />
                                    </section>
                                </div>
                            )}

                            {activeTab === 'long-cycle' && (
                                <div className="space-y-5">
                                    <section aria-labelledby="long-create-title" className="rounded-2xl border border-border bg-surface-elevated p-4">
                                        <h3 id="long-create-title" className="text-[13px] font-semibold text-text-primary">新增长周期任务</h3>
                                        <label className="mt-3 block">
                                            <span className={labelClass}>模板</span>
                                            <select aria-label="长周期任务模板" value={props.longDraft.templateId} onChange={(event) => props.onApplyLongTemplate(event.target.value)} className={fieldClass}>
                                                {LONG_TEMPLATES.map((template) => <option key={template.id} value={template.id}>{template.label}</option>)}
                                            </select>
                                        </label>
                                        <label className="mt-3 block">
                                            <span className={labelClass}>长期目标</span>
                                            <textarea aria-label="长周期任务目标" rows={3} value={props.longDraft.objective} onChange={(event) => props.onLongDraftChange({ objective: event.target.value })} className={fieldClass} />
                                        </label>
                                        <label className="mt-3 block">
                                            <span className={labelClass}>每轮执行指令</span>
                                            <textarea aria-label="长周期每轮指令" rows={4} value={props.longDraft.stepPrompt} onChange={(event) => props.onLongDraftChange({ stepPrompt: event.target.value })} className={fieldClass} />
                                        </label>
                                        <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-2">
                                            <label>
                                                <span className={labelClass}>每轮间隔分钟（1–43200）</span>
                                                <input aria-label="长周期任务间隔分钟" type="number" min={1} max={43200} value={props.longDraft.intervalMinutes} onChange={(event) => props.onLongDraftChange({ intervalMinutes: Number(event.target.value) })} className={fieldClass} />
                                            </label>
                                            <label>
                                                <span className={labelClass}>总轮数（1–200）</span>
                                                <input aria-label="长周期任务总轮数" type="number" min={1} max={200} value={props.longDraft.totalRounds} onChange={(event) => props.onLongDraftChange({ totalRounds: Number(event.target.value) })} className={fieldClass} />
                                            </label>
                                        </div>
                                        <button type="button" onClick={() => props.onLongAdvancedChange(!props.longAdvanced)} className="mt-2 text-[10px] text-accent-primary hover:underline">
                                            {props.longAdvanced ? '收起任务名称' : '设置任务名称'}
                                        </button>
                                        {props.longAdvanced && (
                                            <div className="mt-2 space-y-3">
                                                <label className="block">
                                                    <span className={labelClass}>任务名称</span>
                                                    <input aria-label="长周期任务名称" value={props.longDraft.name} onChange={(event) => props.onLongDraftChange({ name: event.target.value })} className={fieldClass} />
                                                </label>
                                                <label className="block">
                                                    <span className={labelClass}>每轮必须且只允许调用的工具（逗号分隔）</span>
                                                    <input aria-label="长周期每轮必须调用的工具" value={props.longDraft.requiredToolsText} onChange={(event) => props.onLongDraftChange({ requiredToolsText: event.target.value })} placeholder="seedance_video, create_note" className={fieldClass} />
                                                    <span className="mt-1 block text-[10px] leading-4 text-text-tertiary">每轮未真实调用指定工具时，该轮会被明确记为失败。</span>
                                                </label>
                                            </div>
                                        )}
                                        {hasHighRiskTool(props.longDraft.requiredToolsText) && (
                                            <label className="mt-3 flex items-start gap-2 rounded-xl border border-status-warning/30 bg-status-warning/10 px-3 py-2 text-[10px] leading-4 text-text-secondary">
                                                <input aria-label="确认长周期真实操作授权" type="checkbox" checked={props.longDraft.realOperationConfirmed} onChange={(event) => props.onLongDraftChange({ realOperationConfirmed: event.target.checked })} className="mt-0.5 h-3.5 w-3.5 accent-accent-primary" />
                                                <span>我确认每一轮都可真实调用这些生成或发布服务，并接受可能产生的费用或外部副作用。</span>
                                            </label>
                                        )}
                                        <button type="button" disabled={props.isAddingLong || props.loading} onClick={() => void props.onAddLong()} className="mt-4 inline-flex w-full items-center justify-center gap-2 rounded-xl bg-accent-primary px-4 py-2.5 text-[12px] font-semibold text-on-accent hover:bg-accent-hover disabled:cursor-not-allowed disabled:opacity-50">
                                            {props.isAddingLong ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
                                            {props.isAddingLong ? '正在创建...' : '创建并启用长周期任务'}
                                        </button>
                                    </section>
                                    <section aria-labelledby="long-list-title">
                                        <div className="mb-2 flex items-center justify-between">
                                            <h3 id="long-list-title" className="text-[12px] font-semibold text-text-primary">现有长周期任务</h3>
                                            <span className="text-[10px] text-text-tertiary">{props.longTasks.length} 项</span>
                                        </div>
                                        <LongTaskList tasks={props.longTasks} loading={props.loading} onToggle={props.onToggleLong} onRemove={props.onRemoveLong} />
                                    </section>
                                </div>
                            )}

                            {activeTab === 'settings' && (
                                <div className="space-y-4">
                                    <section className="rounded-2xl border border-border bg-surface-elevated p-4">
                                        <div className="flex items-center justify-between gap-4">
                                            <div>
                                                <h3 className="text-[13px] font-semibold text-text-primary">自动调度总开关</h3>
                                                <p className="mt-1 text-[10px] leading-4 text-text-secondary">关闭后不会启动新任务；正在执行的任务会被取消。</p>
                                            </div>
                                            <button type="button" disabled={props.loading || !props.status} onClick={() => void props.onRunnerToggle()} className={clsx('rounded-full border px-3 py-1.5 text-[11px] font-medium disabled:opacity-40', props.status?.enabled ? 'border-status-success/40 bg-status-success/10 text-text-primary' : 'border-border bg-surface-primary text-text-secondary')}>
                                                {props.status?.enabled ? '已开启' : '已关闭'}
                                            </button>
                                        </div>
                                    </section>

                                    <section className="rounded-2xl border border-border bg-surface-elevated p-4">
                                        <h3 className="text-[13px] font-semibold text-text-primary">调度与并发</h3>
                                        <div className="mt-3 grid grid-cols-1 gap-3 sm:grid-cols-2">
                                            <div className="rounded-xl border border-border bg-surface-primary px-3 py-2">
                                                <span className={labelClass}>到点检测</span>
                                                <div className="text-[12px] font-medium text-text-primary">每 0.5 秒（固定）</div>
                                            </div>
                                            <label>
                                                <span className={labelClass}>每轮最多执行任务</span>
                                                <select aria-label="每轮最多执行任务" value={props.runnerMaxAutomationPerTick} onChange={(event) => props.onRunnerMaxAutomationPerTickChange(Number(event.target.value))} className={fieldClass}>
                                                    {[1, 2, 3, 5].map((value) => <option key={value} value={value}>{value}</option>)}
                                                </select>
                                            </label>
                                        </div>
                                        <button type="button" disabled={props.loading} onClick={() => void props.onSaveRunnerConfig()} className="mt-3 w-full rounded-xl border border-accent-primary/40 bg-accent-muted px-4 py-2 text-[11px] font-medium text-text-primary hover:bg-accent-muted/80 disabled:opacity-40">保存运行设置</button>
                                    </section>

                                    <section className="rounded-2xl border border-status-warning/30 bg-status-warning/10 p-4">
                                        <h3 className="text-[13px] font-semibold text-text-primary">心跳检查 · 尚未接入</h3>
                                        <p className="mt-2 text-[11px] leading-5 text-text-secondary">当前 Rust 调度器尚未执行心跳任务，因此此处不提供可开启的假入口。定时与长周期任务不受影响。</p>
                                    </section>
                                </div>
                            )}
                        </div>
                    </section>
                </div>
            )}
        </>
    );
}
