export interface AudioCaptureCapability {
  success?: boolean;
  available?: boolean;
  activeRecording?: boolean;
  platform?: string;
  reason?: string | null;
  message?: string;
  error?: string;
  deviceName?: string;
  sampleRate?: number;
  channels?: number;
  sampleFormat?: string;
}

export interface AudioRecordingClip {
  audioBase64: string;
  mimeType: string;
  fileName: string;
  durationMs?: number;
  byteLength?: number;
  sampleRate?: number;
  channels?: number;
  deviceName?: string;
  strategy?: string;
}

type AudioCaptureActionResult = {
  success?: boolean;
  error?: string;
  reason?: string;
  message?: string;
};

type AudioCaptureStopResult = AudioCaptureActionResult & {
  clip?: AudioRecordingClip;
  discarded?: boolean;
  durationMs?: number;
};

let fallbackStream: MediaStream | null = null;
let fallbackRecorder: MediaRecorder | null = null;
let fallbackChunks: Blob[] = [];
let fallbackStartedAt = 0;
let fallbackMimeType = '';

function hasBrowserAudioRecorder(): boolean {
  return typeof navigator !== 'undefined'
    && Boolean(navigator.mediaDevices?.getUserMedia)
    && typeof MediaRecorder !== 'undefined';
}

function shouldUseBrowserRecorderFallback(resultOrError: unknown): boolean {
  const text = resultOrError instanceof Error
    ? resultOrError.message
    : typeof resultOrError === 'string'
      ? resultOrError
      : resultOrError && typeof resultOrError === 'object'
        ? JSON.stringify(resultOrError)
        : '';
  const normalized = text.toLowerCase();
  return normalized.includes('host_unavailable')
    || normalized.includes('no handler registered')
    || normalized.includes('audio capture unavailable')
    || normalized.includes('audio action failed');
}

function pickBrowserRecorderMimeType(): string {
  const candidates = [
    'audio/webm;codecs=opus',
    'audio/webm',
    'audio/ogg;codecs=opus',
    'audio/mp4',
  ];
  return candidates.find((type) => MediaRecorder.isTypeSupported(type)) || '';
}

function extensionForMimeType(mimeType: string): string {
  const normalized = String(mimeType || '').toLowerCase();
  if (normalized.includes('ogg')) return 'ogg';
  if (normalized.includes('mp4') || normalized.includes('mpeg4')) return 'm4a';
  if (normalized.includes('wav')) return 'wav';
  return 'webm';
}

function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const dataUrl = String(reader.result || '');
      resolve(dataUrl.includes(',') ? dataUrl.split(',').pop() || '' : dataUrl);
    };
    reader.onerror = () => reject(reader.error || new Error('读取录音数据失败'));
    reader.readAsDataURL(blob);
  });
}

function cleanupBrowserRecorder(): void {
  fallbackRecorder = null;
  fallbackChunks = [];
  fallbackStartedAt = 0;
  fallbackMimeType = '';
  if (fallbackStream) {
    fallbackStream.getTracks().forEach((track) => track.stop());
    fallbackStream = null;
  }
}

async function startBrowserAudioRecording(): Promise<void> {
  if (!hasBrowserAudioRecorder()) {
    throw new Error('当前运行环境不支持浏览器麦克风录音');
  }
  if (fallbackRecorder && fallbackRecorder.state !== 'inactive') {
    throw new Error('已有录音任务正在进行');
  }
  fallbackStream = await navigator.mediaDevices.getUserMedia({
    audio: {
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
  });
  fallbackMimeType = pickBrowserRecorderMimeType();
  fallbackChunks = [];
  fallbackStartedAt = Date.now();
  fallbackRecorder = new MediaRecorder(
    fallbackStream,
    fallbackMimeType ? { mimeType: fallbackMimeType } : undefined,
  );
  fallbackRecorder.addEventListener('dataavailable', (event) => {
    if (event.data && event.data.size > 0) {
      fallbackChunks.push(event.data);
    }
  });
  fallbackRecorder.start(250);
}

async function stopBrowserAudioRecording(): Promise<AudioRecordingClip> {
  const recorder = fallbackRecorder;
  if (!recorder || recorder.state === 'inactive') {
    throw new Error('当前没有进行中的录音');
  }
  const stopped = new Promise<void>((resolve) => {
    recorder.addEventListener('stop', () => resolve(), { once: true });
  });
  recorder.stop();
  await stopped;
  const mimeType = fallbackMimeType || recorder.mimeType || 'audio/webm';
  const blob = new Blob(fallbackChunks, { type: mimeType });
  const durationMs = fallbackStartedAt ? Date.now() - fallbackStartedAt : undefined;
  const audioBase64 = await blobToBase64(blob);
  const ext = extensionForMimeType(mimeType);
  cleanupBrowserRecorder();
  return {
    audioBase64,
    mimeType,
    fileName: `chat_audio_${Date.now()}.${ext}`,
    durationMs,
    byteLength: blob.size,
    strategy: 'browser-media-recorder',
  };
}

async function cancelBrowserAudioRecording(): Promise<void> {
  if (fallbackRecorder && fallbackRecorder.state !== 'inactive') {
    const stopped = new Promise<void>((resolve) => {
      fallbackRecorder?.addEventListener('stop', () => resolve(), { once: true });
    });
    fallbackRecorder.stop();
    await stopped;
  }
  cleanupBrowserRecorder();
}

export async function getAudioCaptureCapability(): Promise<AudioCaptureCapability> {
  const browserAvailable = hasBrowserAudioRecorder();
  try {
    const result = await window.ipcRenderer.audio.getCaptureCapability();
    if (result?.available || !browserAvailable || !shouldUseBrowserRecorderFallback(result)) {
      return result;
    }
  } catch (error) {
    if (!browserAvailable || !shouldUseBrowserRecorderFallback(error)) {
      throw error;
    }
  }
  return {
    success: true,
    available: true,
    activeRecording: Boolean(fallbackRecorder && fallbackRecorder.state !== 'inactive'),
    platform: 'browser',
    deviceName: '系统麦克风',
  };
}

export async function startHostAudioRecording(): Promise<void> {
  try {
    const result = await window.ipcRenderer.audio.startRecording();
    if (result?.success) return;
    if (!shouldUseBrowserRecorderFallback(result)) {
      throw new Error(describeAudioCaptureFailure(result));
    }
  } catch (error) {
    if (!shouldUseBrowserRecorderFallback(error)) {
      throw error;
    }
  }
  await startBrowserAudioRecording();
}

export async function stopHostAudioRecording(): Promise<AudioRecordingClip> {
  if (fallbackRecorder && fallbackRecorder.state !== 'inactive') {
    return stopBrowserAudioRecording();
  }
  const result = await window.ipcRenderer.audio.stopRecording() as AudioCaptureStopResult;
  if (!result?.success || !result.clip) {
    throw new Error(describeAudioCaptureFailure(result));
  }
  return result.clip;
}

export async function cancelHostAudioRecording(): Promise<void> {
  if (fallbackRecorder && fallbackRecorder.state !== 'inactive') {
    await cancelBrowserAudioRecording();
    return;
  }
  const result = await window.ipcRenderer.audio.cancelRecording();
  if (!result?.success) {
    throw new Error(describeAudioCaptureFailure(result));
  }
}

export async function openMicrophonePrivacySettings(): Promise<void> {
  const result = await window.ipcRenderer.audio.openMicrophoneSettings();
  if (!result?.success) {
    throw new Error(result?.error || '无法打开系统麦克风设置');
  }
}

export function buildAudioDataUrl(clip: AudioRecordingClip): string {
  return `data:${clip.mimeType};base64,${clip.audioBase64}`;
}

export function describeAudioCaptureFailure(
  error: unknown,
  capability?: AudioCaptureCapability | null,
): string {
  const message = error instanceof Error
    ? error.message
    : typeof error === 'string'
      ? error
      : (error && typeof error === 'object' && 'error' in error && typeof (error as { error?: unknown }).error === 'string')
        ? String((error as { error?: string }).error)
        : '';
  if (message) {
    return normalizeAudioCaptureMessage(message);
  }

  const reason = String(capability?.reason || '').trim().toLowerCase();
  if (reason === 'no_input_device') {
    return '未检测到可用麦克风设备';
  }
  if (reason === 'permission_denied') {
    return '系统未授予麦克风权限，请在系统设置中允许商媒运营助手使用麦克风';
  }
  return '麦克风录音不可用，请检查设备和系统权限';
}

function normalizeAudioCaptureMessage(message: string): string {
  const normalized = String(message || '').trim().toLowerCase();
  if (!normalized) return '麦克风录音不可用';
  if (normalized.includes('already_recording')) {
    return '已有录音任务正在进行';
  }
  if (normalized.includes('not_recording')) {
    return '当前没有进行中的录音';
  }
  if (normalized.includes('permission')) {
    return '系统未授予麦克风权限，请在系统设置中允许商媒运营助手使用麦克风';
  }
  if (normalized.includes('no_input_device') || normalized.includes('未检测到可用麦克风设备')) {
    return '未检测到可用麦克风设备';
  }
  return message;
}
