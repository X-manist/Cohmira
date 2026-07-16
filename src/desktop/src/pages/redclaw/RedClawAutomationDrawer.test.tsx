import type { ComponentProps } from 'react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { longDraftFromTemplate, LONG_TEMPLATES, scheduleDraftFromTemplate, SCHEDULE_TEMPLATES } from './config';
import { defaultOnceRunAtLocal, RedClawAutomationDrawer } from './RedClawAutomationDrawer';

type DrawerProps = ComponentProps<typeof RedClawAutomationDrawer>;

function buildProps(): DrawerProps {
    return {
        open: true,
        status: {
            enabled: true,
            lockState: 'owner',
            blockedBy: null,
            intervalMinutes: 1,
            keepAliveWhenNoWindow: false,
            maxProjectsPerTick: 2,
            maxAutomationPerTick: 2,
            isTicking: false,
            currentProjectId: null,
            lastTickAt: null,
            nextTickAt: null,
            lastError: null,
            capabilities: {
                automaticExecution: true,
                persistentDefinitions: true,
                survivesAppExit: false,
                statusText: 'Rust 调度器真实执行并保存回执',
            },
        },
        loading: false,
        message: '',
        scheduledTasks: [],
        longTasks: [],
        scheduleDraft: scheduleDraftFromTemplate(SCHEDULE_TEMPLATES[0]),
        scheduleAdvanced: false,
        isAddingSchedule: false,
        longDraft: longDraftFromTemplate(LONG_TEMPLATES[0]),
        longAdvanced: false,
        isAddingLong: false,
        runnerMaxAutomationPerTick: 2,
        onOpen: vi.fn(),
        onClose: vi.fn(),
        onRefresh: vi.fn(),
        onRunnerToggle: vi.fn(),
        onScheduleDraftChange: vi.fn(),
        onScheduleAdvancedChange: vi.fn(),
        onApplyScheduleTemplate: vi.fn(),
        onAddSchedule: vi.fn(),
        onToggleSchedule: vi.fn(),
        onRemoveSchedule: vi.fn(),
        onLongDraftChange: vi.fn(),
        onLongAdvancedChange: vi.fn(),
        onApplyLongTemplate: vi.fn(),
        onAddLong: vi.fn(),
        onToggleLong: vi.fn(),
        onRemoveLong: vi.fn(),
        onRunnerMaxAutomationPerTickChange: vi.fn(),
        onSaveRunnerConfig: vi.fn(),
    };
}

describe('RedClawAutomationDrawer', () => {
    beforeEach(() => vi.clearAllMocks());

    it('shows the real execution, persistence, and process-exit capabilities', () => {
        render(<RedClawAutomationDrawer {...buildProps()} />);

        expect(screen.getByText('自动执行')).toBeInTheDocument();
        expect(screen.getByText('跨重启保留')).toBeInTheDocument();
        expect(screen.getByText('退出后运行')).toBeInTheDocument();
        expect(screen.getByText('需保持应用运行')).toBeInTheDocument();
        expect(screen.getByText('Rust 调度器真实执行并保存回执')).toBeInTheDocument();
    });

    it('lets a user select one-time execution and create the scheduled task', () => {
        const props = buildProps();
        render(<RedClawAutomationDrawer {...props} />);

        fireEvent.change(screen.getByLabelText('定时任务执行方式'), { target: { value: 'once' } });
        expect(props.onScheduleDraftChange).toHaveBeenCalledWith(expect.objectContaining({
            mode: 'once',
            runAtLocal: expect.stringMatching(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/),
        }));

        fireEvent.click(screen.getByRole('button', { name: '创建并启用定时任务' }));
        expect(props.onAddSchedule).toHaveBeenCalledTimes(1);
    });

    it('exposes long-cycle objective, step, interval, rounds, and create action', () => {
        const props = buildProps();
        render(<RedClawAutomationDrawer {...props} />);

        fireEvent.click(screen.getByRole('button', { name: '长周期' }));
        fireEvent.change(screen.getByLabelText('长周期任务目标'), { target: { value: '五分钟真实任务' } });
        fireEvent.change(screen.getByLabelText('长周期任务间隔分钟'), { target: { value: '5' } });
        fireEvent.change(screen.getByLabelText('长周期任务总轮数'), { target: { value: '3' } });
        fireEvent.click(screen.getByRole('button', { name: '创建并启用长周期任务' }));

        expect(props.onLongDraftChange).toHaveBeenCalledWith({ objective: '五分钟真实任务' });
        expect(props.onLongDraftChange).toHaveBeenCalledWith({ intervalMinutes: 5 });
        expect(props.onLongDraftChange).toHaveBeenCalledWith({ totalRounds: 3 });
        expect(props.onAddLong).toHaveBeenCalledTimes(1);
    });

    it('keeps a daily long-cycle interval without truncating it to 180 minutes', () => {
        const props = buildProps();
        render(<RedClawAutomationDrawer {...props} />);

        fireEvent.click(screen.getByRole('button', { name: '长周期' }));
        const interval = screen.getByLabelText('长周期任务间隔分钟');
        expect(interval).toHaveAttribute('max', '43200');
        fireEvent.change(interval, { target: { value: '1440' } });

        expect(props.onLongDraftChange).toHaveBeenCalledWith({ intervalMinutes: 1440 });
    });

    it('exposes required-tool enforcement and never offers a fake heartbeat switch', () => {
        const props = buildProps();
        const { rerender } = render(<RedClawAutomationDrawer {...props} />);

        fireEvent.click(screen.getByRole('button', { name: '设置任务名称' }));
        expect(props.onScheduleAdvancedChange).toHaveBeenCalledWith(true);
        rerender(<RedClawAutomationDrawer {...props} scheduleAdvanced />);
        fireEvent.change(screen.getByLabelText('定时任务必须调用的工具'), { target: { value: 'generate_image, create_note' } });
        expect(props.onScheduleDraftChange).toHaveBeenCalledWith({ requiredToolsText: 'generate_image, create_note' });

        fireEvent.click(screen.getByRole('button', { name: '运行设置' }));
        expect(screen.getByText('心跳检查 · 尚未接入')).toBeInTheDocument();
        expect(screen.queryByLabelText('启用心跳检查')).not.toBeInTheDocument();
    });

    it('exposes the scheduler master switch and persisted runtime settings', () => {
        const props = buildProps();
        render(<RedClawAutomationDrawer {...props} />);

        fireEvent.click(screen.getByRole('button', { name: '运行设置' }));
        fireEvent.click(screen.getByRole('button', { name: '已开启' }));
        fireEvent.click(screen.getByRole('button', { name: '保存运行设置' }));

        expect(props.onRunnerToggle).toHaveBeenCalledTimes(1);
        expect(screen.getByText('每 0.5 秒（固定）')).toBeInTheDocument();
        expect(props.onSaveRunnerConfig).toHaveBeenCalledTimes(1);
    });

    it('builds a local one-time default five minutes in the future', () => {
        const now = new Date('2026-07-16T08:00:00.000Z').getTime();
        const value = defaultOnceRunAtLocal(now);
        const parsed = new Date(value).getTime();

        expect(parsed - now).toBe(5 * 60_000);
    });

    it('is reachable from RedClaw and wires creation to the real runner bridge', () => {
        const source = readFileSync(resolve(process.cwd(), 'src/pages/RedClaw.tsx'), 'utf8');

        expect(source).toContain('<RedClawAutomationDrawer');
        expect(source).toContain('onAddSchedule={addScheduleTask}');
        expect(source).toContain('onAddLong={addLongTask}');
        expect(source).toContain('redclawRunner.addScheduled');
        expect(source).toContain('redclawRunner.addLongCycle');
        expect(source).toContain('redclawRunner.start');
        expect(source).toContain('redclawRunner.stop');
    });
});
