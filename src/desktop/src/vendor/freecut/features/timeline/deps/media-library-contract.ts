import { create } from 'zustand';
import type {
  MediaMetadata,
  MediaTranscript,
  MediaTranscriptModel,
} from '@/types/storage';
import type { TranscriptionProgressSnapshot } from '@/shared/utils/transcription-progress';

export type RedBoxMediaInput = {
  id: string;
  name: string;
  src: string;
  mimeType: string;
  duration: number;
  fps: number;
  width?: number;
  height?: number;
  thumbnailUrl?: string;
  blobUrl?: string;
  proxyUrl?: string | null;
  isBroken?: boolean;
  transcriptStatus?: 'idle' | 'processing' | 'ready' | 'error' | null;
};

export type RedBoxMediaItem = MediaMetadata & {
  name?: string;
  src?: string;
  thumbnailUrl?: string;
  blobUrl?: string;
  proxyUrl?: string | null;
  isBroken?: boolean;
  transcriptStatus?: 'idle' | 'processing' | 'ready' | 'error' | null;
};

type TranscriptStatus = 'idle' | 'transcribing' | 'ready' | 'error';
type TranscriptProgress = TranscriptionProgressSnapshot;

type NotificationPayload = {
  type: 'success' | 'error' | 'warning' | 'info';
  message: string;
};

type RedBoxMediaLibraryState = {
  mediaItems: RedBoxMediaItem[];
  mediaById: Record<string, RedBoxMediaItem>;
  currentProjectId: string | null;
  importHandlesForPlacement: (handles: FileSystemFileHandle[]) => Promise<MediaMetadata[]>;
  transcriptStatus: Map<string, TranscriptStatus>;
  transcriptProgress: Map<string, TranscriptProgress>;
  proxyStatus: Map<string, 'idle' | 'processing' | 'ready' | 'error'>;
  brokenMediaIds: Set<string>;
  orphanedClips: OrphanedClipInfo[];
};

type RedBoxMediaLibraryActions = {
  syncMediaItems: (items: RedBoxMediaInput[]) => void;
  setTranscriptStatus: (mediaId: string, status: TranscriptStatus) => void;
  setTranscriptProgress: (mediaId: string, progress: TranscriptProgress) => void;
  clearTranscriptProgress: (mediaId: string) => void;
  showNotification: (payload: NotificationPayload) => void;
  setOrphanedClips: (clips: OrphanedClipInfo[]) => void;
  openOrphanedClipsDialog: () => void;
  closeOrphanedClipsDialog: () => void;
};

export type CompositionDragData = {
  type: 'composition';
  compositionId: string;
  name: string;
  durationInFrames: number;
};

export type TimelineTemplateDragData = {
  type: 'timeline-template';
  itemType: 'text' | 'shape' | 'adjustment';
  label: string;
  [key: string]: unknown;
};

export type MediaDragData = CompositionDragData | TimelineTemplateDragData | {
  type: 'media-item';
  mediaId: string;
  mediaType: 'video' | 'audio' | 'image';
  fileName: string;
  duration: number;
} | {
  type: 'media-items';
  items: Array<{
    mediaId: string;
    mediaType: 'video' | 'audio' | 'image';
    fileName: string;
    duration: number;
  }>;
};

export type OrphanedClipInfo = {
  clipId?: string;
  mediaId: string;
  itemId: string;
  fileName?: string;
  itemType?: string;
  trackId?: string;
};

export type ExtractedMediaFileEntry = {
  file: File;
  mediaId: string;
  mimeType: string;
  mediaType: 'video' | 'audio' | 'image';
  label: string;
  handle?: FileSystemFileHandle;
};

function normalizeMediaItem(item: RedBoxMediaInput): RedBoxMediaItem {
  const now = Date.now();
  return {
    ...item,
    id: item.id,
    storageType: 'opfs',
    fileName: item.name,
    fileSize: 0,
    mimeType: item.mimeType,
    duration: item.duration,
    width: item.width ?? 0,
    height: item.height ?? 0,
    fps: item.fps,
    codec: '',
    bitrate: 0,
    tags: [],
    createdAt: now,
    updatedAt: now,
  };
}

export const useMediaLibraryStore = create<RedBoxMediaLibraryState & RedBoxMediaLibraryActions>((set) => ({
  mediaItems: [],
  mediaById: {},
  currentProjectId: null,
  importHandlesForPlacement: async () => [],
  transcriptStatus: new Map(),
  transcriptProgress: new Map(),
  proxyStatus: new Map(),
  brokenMediaIds: new Set(),
  orphanedClips: [],
  syncMediaItems: (items) => set({
    mediaItems: items.map(normalizeMediaItem),
    mediaById: Object.fromEntries(items.map(normalizeMediaItem).map((item) => [item.id, item])),
  }),
  setTranscriptStatus: (mediaId, status) => set((state) => ({
    transcriptStatus: new Map(state.transcriptStatus).set(mediaId, status),
  })),
  setTranscriptProgress: (mediaId, progress) => set((state) => ({
    transcriptProgress: new Map(state.transcriptProgress).set(mediaId, progress),
  })),
  clearTranscriptProgress: (mediaId) => set((state) => {
    const transcriptProgress = new Map(state.transcriptProgress);
    transcriptProgress.delete(mediaId);
    return { transcriptProgress };
  }),
  showNotification: ({ type, message }) => {
    if (type === 'error') console.error(`[media] ${message}`);
    else if (type === 'warning') console.warn(`[media] ${message}`);
    else console.info(`[media] ${message}`);
  },
  setOrphanedClips: (orphanedClips) => set({ orphanedClips }),
  openOrphanedClipsDialog: () => undefined,
  closeOrphanedClipsDialog: () => undefined,
}));

export function syncRedBoxMediaLibrary(items: RedBoxMediaInput[]) {
  useMediaLibraryStore.getState().syncMediaItems(items);
}

function resolveMediaUrlSync(mediaIdOrUrl: string): string {
  return useMediaLibraryStore.getState().mediaById[mediaIdOrUrl]?.src || mediaIdOrUrl;
}

export async function resolveMediaUrl(mediaIdOrUrl: string): Promise<string> {
  return resolveMediaUrlSync(mediaIdOrUrl);
}

export function resolveProxyUrl(mediaIdOrUrl: string): string {
  return useMediaLibraryStore.getState().mediaById[mediaIdOrUrl]?.proxyUrl || resolveMediaUrlSync(mediaIdOrUrl);
}

export async function resolveMediaUrls<T>(value: T): Promise<T> {
  return value;
}

export function cleanupBlobUrls(): void {}

let currentDragData: MediaDragData | null = null;

export function getMediaDragData(): MediaDragData | null {
  return currentDragData;
}

export function setMediaDragData(data: MediaDragData | null) {
  currentDragData = data;
}

export function clearMediaDragData() {
  currentDragData = null;
}

export function getMediaType(mimeType: string | undefined): 'video' | 'audio' | 'image' | 'unknown' {
  const normalized = String(mimeType || '').toLowerCase();
  if (normalized.startsWith('video/')) return 'video';
  if (normalized.startsWith('audio/')) return 'audio';
  if (normalized.startsWith('image/')) return 'image';
  return 'unknown';
}

export function getMimeType(file: File): string {
  return file.type || 'application/octet-stream';
}

export async function extractValidMediaFileEntriesFromDataTransfer(dataTransfer: DataTransfer | null) {
  if (!dataTransfer?.files?.length) {
    return { supported: false, entries: [] as ExtractedMediaFileEntry[], errors: [] as string[] };
  }

  const entries = Array.from(dataTransfer.files)
    .map((file) => {
      const mimeType = getMimeType(file);
      const mediaType = getMediaType(mimeType);
      if (mediaType === 'unknown') return null;
      return {
        file,
        mediaId: `${file.name}-${file.size}-${file.lastModified}`,
        mimeType,
        mediaType,
        label: file.name,
      } satisfies ExtractedMediaFileEntry;
    })
    .filter(Boolean) as ExtractedMediaFileEntry[];

  return {
    supported: entries.length > 0,
    entries,
    errors: [] as string[],
  };
}

export function supportsFileSystemDragDrop() {
  return false;
}

export const mediaLibraryService = {
  async getMedia(mediaId: string) {
    return useMediaLibraryStore.getState().mediaById[mediaId] || null;
  },
  async getMediaForProject(_projectId?: string) {
    return useMediaLibraryStore.getState().mediaItems;
  },
  async getMediaBlobUrl(mediaId: string) {
    const item = useMediaLibraryStore.getState().mediaById[mediaId];
    return item?.blobUrl || item?.src || null;
  },
  async getThumbnailBlobUrl(mediaId: string) {
    return useMediaLibraryStore.getState().mediaById[mediaId]?.thumbnailUrl || null;
  },
  async getMediaFile(mediaId: string) {
    const media = useMediaLibraryStore.getState().mediaById[mediaId];
    if (!media) return null;
    if (media.fileHandle) return media.fileHandle.getFile();
    return null;
  },
};

export const mediaProcessorService = {
  async processMedia(
    file: File,
    mimeType: string,
    _options?: { generateThumbnail?: boolean },
  ) {
    const mediaType = getMediaType(mimeType);
    return {
      metadata: {
        type: mediaType,
        mimeType,
        duration: 0,
        fps: 30,
        width: 0,
        height: 0,
        title: file.name,
      },
    };
  },
};

export const mediaTranscriptionService = {
  async getTranscript(_mediaId: string): Promise<MediaTranscript | null> {
    return null;
  },
  async transcribeMedia(
    _mediaId: string,
    _options: { model: MediaTranscriptModel; onProgress?: (progress: TranscriptProgress) => void },
  ): Promise<MediaTranscript | null> {
    return null;
  },
  async insertTranscriptAsCaptions(
    _mediaId: string,
    _options: { clipIds: string[]; replaceExisting?: boolean },
  ) {
    return {
      insertedItemCount: 0,
      removedItemCount: 0,
    };
  },
};

export function getMediaTranscriptionModelLabel(model: string) {
  return model || 'disabled';
}

export function getMediaTranscriptionModelOptions() {
  return [
    { value: 'whisper-tiny', label: 'Tiny' },
    { value: 'whisper-base', label: 'Base' },
    { value: 'whisper-small', label: 'Small' },
    { value: 'whisper-large', label: 'Large' },
  ] satisfies Array<{ value: MediaTranscriptModel; label: string }>;
}

export const opfsService = {
  async getFile(_path?: string) {
    return null;
  },
  async saveFile(_path: string, _file: Blob | ArrayBuffer) {
    return undefined;
  },
};
