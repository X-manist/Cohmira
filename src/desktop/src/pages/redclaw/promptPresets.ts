import {
    DEFAULT_REDCLAW_PROMPT_PRESETS,
    REDCLAW_OPERATION_SHORTCUTS,
    type RedClawPromptPreset,
} from './config';

export type { RedClawPromptPreset };

export const REDCLAW_PROMPT_PRESETS_STORAGE_KEY = 'redclaw:prompt-presets:v1';

function normalizePresetId(value: unknown, index: number): string {
    const text = String(value || '').trim();
    if (text) return text;
    return `custom-${Date.now()}-${index}`;
}

export function normalizeRedClawPromptPresets(value: unknown): RedClawPromptPreset[] {
    const rawItems = Array.isArray(value) ? value : [];
    const normalized = rawItems
        .map((item, index) => {
            if (!item || typeof item !== 'object') return null;
            const record = item as Record<string, unknown>;
            const label = String(record.label || '').trim();
            const text = String(record.text || '');
            return {
                id: normalizePresetId(record.id, index),
                label,
                text,
            };
        })
        .filter((item): item is RedClawPromptPreset => Boolean(item));
    return normalized.length > 0 ? normalized : DEFAULT_REDCLAW_PROMPT_PRESETS;
}

export function loadRedClawPromptPresets(): RedClawPromptPreset[] {
    if (typeof window === 'undefined') return DEFAULT_REDCLAW_PROMPT_PRESETS;
    try {
        const raw = window.localStorage.getItem(REDCLAW_PROMPT_PRESETS_STORAGE_KEY);
        if (!raw) return DEFAULT_REDCLAW_PROMPT_PRESETS;
        return normalizeRedClawPromptPresets(JSON.parse(raw));
    } catch {
        return DEFAULT_REDCLAW_PROMPT_PRESETS;
    }
}

export function saveRedClawPromptPresets(presets: RedClawPromptPreset[]): RedClawPromptPreset[] {
    const normalized = normalizeRedClawPromptPresets(presets);
    if (typeof window !== 'undefined') {
        window.localStorage.setItem(REDCLAW_PROMPT_PRESETS_STORAGE_KEY, JSON.stringify(normalized));
        window.dispatchEvent(new CustomEvent('redclaw:prompt-presets-updated', { detail: normalized }));
    }
    return normalized;
}

export function createRedClawPromptPreset(): RedClawPromptPreset {
    return {
        id: `custom-${Date.now()}`,
        label: '新预设',
        text: '请在这里填写这类创作任务的前置 Prompt。',
    };
}

export function buildRedClawShortcutList(presets: RedClawPromptPreset[]) {
    return [
        ...normalizeRedClawPromptPresets(presets)
            .filter((preset) => preset.label.trim() && preset.text.trim())
            .map((preset) => ({
                label: preset.label.trim(),
                text: preset.text.trim(),
                action: 'inject' as const,
                presetPrompt: true,
            })),
        ...REDCLAW_OPERATION_SHORTCUTS,
    ];
}
