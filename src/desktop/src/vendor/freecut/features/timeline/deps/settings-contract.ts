import { create } from 'zustand';
import { HOTKEYS, type HotkeyBindingMap } from '@/config/hotkeys';
import type { MediaTranscriptModel } from '@/types/storage';

type RedBoxSettingsState = {
  editorDensity: 'compact' | 'default';
  showWaveforms: boolean;
  showFilmstrips: boolean;
  defaultWhisperModel: MediaTranscriptModel;
  maxUndoHistory: number;
  snapEnabled: boolean;
};

type RedBoxSettingsActions = {
  syncRedBoxSettings: (patch: Partial<RedBoxSettingsState>) => void;
};

export const useSettingsStore = create<RedBoxSettingsState & RedBoxSettingsActions>((set) => ({
  editorDensity: 'compact',
  showWaveforms: true,
  showFilmstrips: true,
  defaultWhisperModel: 'whisper-base',
  maxUndoHistory: 80,
  snapEnabled: true,
  syncRedBoxSettings: (patch) => set((state) => ({ ...state, ...patch })),
}));

export function syncRedBoxTimelineSettings(patch: Partial<RedBoxSettingsState>) {
  useSettingsStore.getState().syncRedBoxSettings(patch);
}

export function useResolvedHotkeys(): HotkeyBindingMap {
  return HOTKEYS;
}
