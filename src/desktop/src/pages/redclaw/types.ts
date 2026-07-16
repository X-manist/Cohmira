export type ScheduleMode = 'interval' | 'daily' | 'weekly' | 'once';
export type RunnerResult = 'success' | 'error' | 'skipped';
export type SidebarTab = 'skills';

export interface RunnerScheduledTask {
    id: string;
    name: string;
    enabled: boolean;
    mode: ScheduleMode;
    prompt: string;
    requiredTools?: string[];
    allowedTools?: string[];
    authorizedTools?: string[];
    intervalMinutes?: number;
    time?: string;
    weekdays?: number[];
    runAt?: string;
    createdAt: string;
    updatedAt: string;
    lastRunAt?: string;
    lastResult?: RunnerResult;
    lastOutput?: string;
    lastError?: string;
    nextRunAt?: string;
    lastReceiptPath?: string;
    executionStatus?: 'scheduled' | 'paused' | 'running' | 'completed' | 'failed';
    startedAt?: string;
}

export interface RunnerLongCycleTask {
    id: string;
    name: string;
    enabled: boolean;
    status: 'running' | 'paused' | 'completed';
    objective: string;
    stepPrompt: string;
    requiredTools?: string[];
    allowedTools?: string[];
    authorizedTools?: string[];
    intervalMinutes: number;
    totalRounds: number;
    completedRounds: number;
    createdAt: string;
    updatedAt: string;
    lastRunAt?: string;
    lastResult?: RunnerResult;
    lastOutput?: string;
    lastError?: string;
    nextRunAt?: string;
    lastReceiptPath?: string;
    executionStatus?: 'scheduled' | 'paused' | 'running' | 'completed' | 'failed';
    startedAt?: string;
}

export interface RunnerStatus {
    enabled: boolean;
    lockState: 'owner' | 'passive';
    blockedBy: string | null;
    intervalMinutes: number;
    keepAliveWhenNoWindow: boolean;
    maxProjectsPerTick: number;
    maxAutomationPerTick?: number;
    isTicking: boolean;
    currentProjectId: string | null;
    currentAutomationTaskId?: string | null;
    nextAutomationFireAt?: string | null;
    inFlightTaskIds?: string[];
    inFlightLongCycleTaskIds?: string[];
    heartbeatInFlight?: boolean;
    lastTickAt: string | null;
    nextTickAt: string | null;
    nextMaintenanceAt?: string | null;
    lastError: string | null;
    heartbeat?: {
        enabled: boolean;
        intervalMinutes: number;
        suppressEmptyReport: boolean;
        reportToMainSession: boolean;
        prompt?: string;
        lastRunAt?: string;
        nextRunAt?: string;
        lastDigest?: string;
    };
    scheduledTasks?: Record<string, RunnerScheduledTask>;
    longCycleTasks?: Record<string, RunnerLongCycleTask>;
    capabilities?: {
        scheduledDefinitions?: boolean;
        longCycleDefinitions?: boolean;
        automaticExecution?: boolean;
        persistentDefinitions?: boolean;
        survivesAppExit?: boolean;
        cancellableExecution?: boolean;
        executionReceipts?: boolean;
        statusText?: string;
    };
}

export interface ScheduleTemplate {
    id: string;
    label: string;
    description: string;
    name: string;
    mode: ScheduleMode;
    intervalMinutes?: number;
    time?: string;
    weekdays?: number[];
    requiredTools?: string[];
    prompt: string;
}

export interface LongTemplate {
    id: string;
    label: string;
    description: string;
    name: string;
    objective: string;
    stepPrompt: string;
    intervalMinutes: number;
    totalRounds: number;
    requiredTools?: string[];
}

export interface ScheduleDraft {
    templateId: string;
    name: string;
    mode: ScheduleMode;
    intervalMinutes: number;
    time: string;
    weekdays: number[];
    runAtLocal: string;
    prompt: string;
    requiredToolsText: string;
    realOperationConfirmed: boolean;
}

export interface LongDraft {
    templateId: string;
    name: string;
    objective: string;
    stepPrompt: string;
    intervalMinutes: number;
    totalRounds: number;
    requiredToolsText: string;
    realOperationConfirmed: boolean;
}
