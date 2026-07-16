import React, {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import {
  ArrowUp,
  ChevronDown,
  File as FileIcon,
  FileText,
  Film,
  ImageIcon,
  Loader2,
  Mic,
  Music2,
  Plus,
  Square,
  StopCircle,
  X,
} from 'lucide-react';
import { clsx } from 'clsx';
import { getForcedModelCapabilities, inferModelCapabilities, normalizeModelCapabilities, type ModelCapability } from '../../shared/modelCapabilities';
import { resolveAssetUrl } from '../utils/pathManager';
import { ChatComposerFrame, getChatComposerPalette, type ChatComposerTheme, type ChatComposerVariant } from './ChatComposerFrame';

export interface UploadedFileAttachment {
  type: 'uploaded-file';
  name: string;
  ext?: string;
  size?: number;
  thumbnailDataUrl?: string;
  workspaceRelativePath?: string;
  absolutePath?: string;
  originalAbsolutePath?: string;
  localUrl?: string;
  kind?: 'text' | 'image' | 'audio' | 'video' | 'binary' | string;
  mimeType?: string;
  storageMode?: 'staged' | string;
  directUploadEligible?: boolean;
  processingStrategy?: string;
  deliveryMode?: 'direct-input' | 'tool-read';
  summary?: string;
  requiresMultimodal?: boolean;
}

export interface ChatModelOption {
  key: string;
  modelName: string;
  sourceName: string;
  baseURL: string;
  apiKey: string;
  isDefault?: boolean;
}

export interface ChatSettingsSnapshot {
  api_endpoint?: string;
  api_key?: string;
  model_name?: string;
  ai_sources_json?: string;
  default_ai_source_id?: string;
}

export interface ChatComposerHandle {
  focus: () => void;
  blur: () => void;
  syncHeight: () => void;
  resetHeight: () => void;
  getTextarea: () => HTMLTextAreaElement | null;
}

export interface ChatComposerPresetPrompt {
  label: string;
  text: string;
}

export interface ChatComposerSlashCommand {
  name: string;
  description: string;
  insertion?: string;
}

type ComposerAttachmentVisualKind = 'image' | 'video' | 'audio' | 'text' | 'file';
type ChatComposerAudioState = 'idle' | 'recording' | 'transcribing';
const RECORDING_WAVE_BARS = [0.3, 0.58, 0.92, 0.42, 0.74, 0.98, 0.5, 0.8, 0.64, 0.9, 0.46, 0.7, 1, 0.62, 0.84, 0.54, 0.95, 0.4, 0.78, 0.34, 0.88, 0.56, 0.72, 0.44];

interface ComposerAttachmentPreviewProps {
  attachment: UploadedFileAttachment;
  darkEmbedded: boolean;
  variant: ChatComposerVariant;
  onRemove: () => void;
}

export interface ChatComposerProps {
  theme?: ChatComposerTheme;
  variant?: ChatComposerVariant;
  className?: string;
  value: string;
  onValueChange: (value: string) => void;
  onSubmit: () => void;
  placeholder: string;
  presetPrompt?: ChatComposerPresetPrompt | null;
  onPresetPromptChange?: ((value: ChatComposerPresetPrompt | null) => void) | null;
  attachment?: UploadedFileAttachment | null;
  attachments?: UploadedFileAttachment[];
  onPickAttachment?: (() => void | Promise<void>) | null;
  onClearAttachment?: (() => void) | null;
  onRemoveAttachment?: ((attachment: UploadedFileAttachment, index: number) => void) | null;
  modelOptions?: ChatModelOption[];
  selectedModelKey?: string;
  onSelectedModelKeyChange?: (key: string) => void;
  isBusy?: boolean;
  audioState?: ChatComposerAudioState;
  onAudioAction?: (() => void | Promise<void>) | null;
  onCancel?: (() => void | Promise<void>) | null;
  showCancelWhenBusy?: boolean;
  disabled?: boolean;
  readOnly?: boolean;
  trailingContent?: ReactNode;
  onFocus?: () => void;
  onPasteFiles?: (files: File[]) => void | Promise<void>;
  slashCommands?: ChatComposerSlashCommand[];
  suppressed?: boolean;
  suppressedLabel?: string;
  onResumeFromSuppressed?: (() => void) | null;
  textareaMaxHeight?: number;
}

const IMAGE_ATTACHMENT_EXT_RE = /\.(png|jpe?g|webp|gif|bmp|svg|avif)(?:[?#].*)?$/i;
const VIDEO_ATTACHMENT_EXT_RE = /\.(mp4|mov|webm|m4v|avi|mkv)(?:[?#].*)?$/i;
const AUDIO_ATTACHMENT_EXT_RE = /\.(mp3|wav|m4a|aac|flac|ogg|opus|webm)(?:[?#].*)?$/i;
const TEXT_ATTACHMENT_EXT_RE = /\.(txt|md|markdown|json|csv|tsv|doc|docx|pdf|rtf|xml|yaml|yml|ts|tsx|js|jsx|py|rs|java|go|c|cpp|h|hpp)(?:[?#].*)?$/i;

function modelSupportsChat(model: string | { id?: unknown; capability?: unknown; capabilities?: unknown }): boolean {
  if (typeof model === 'string') {
    const forced = getForcedModelCapabilities(model);
    const resolved = forced.length ? forced : inferModelCapabilities(model);
    return resolved.includes('chat');
  }
  const id = String(model?.id || '').trim();
  if (!id) return false;
  const forced = getForcedModelCapabilities(id);
  const explicitCapabilities = [
    ...(
      Array.isArray((model as { capabilities?: unknown[] }).capabilities)
        ? ((model as { capabilities?: Array<ModelCapability | string | null | undefined> }).capabilities || [])
        : []
    ),
    (model as { capability?: ModelCapability | string | null | undefined }).capability,
  ];
  const capabilities = explicitCapabilities.some((value) => String(value || '').trim())
    ? normalizeModelCapabilities(explicitCapabilities)
    : [];
  const resolved = forced.length ? forced : (capabilities.length ? capabilities : inferModelCapabilities(id));
  return resolved.includes('chat');
}

function getAttachmentSource(attachment: UploadedFileAttachment): string {
  const preferred = String(
    attachment.thumbnailDataUrl
      || attachment.localUrl
      || attachment.absolutePath
      || attachment.originalAbsolutePath
      || '',
  ).trim();
  if (!preferred) return '';
  if (preferred.startsWith('data:')) {
    return preferred;
  }
  return resolveAssetUrl(preferred);
}

function getAttachmentExtLabel(attachment: UploadedFileAttachment): string {
  const explicit = String(attachment.ext || '').trim().replace(/^\./, '');
  if (explicit) return explicit.toUpperCase();
  const matched = String(attachment.name || '').trim().match(/\.([a-zA-Z0-9]+)$/);
  return matched?.[1]?.toUpperCase() || '';
}

function getAttachmentVisualKind(attachment: UploadedFileAttachment): ComposerAttachmentVisualKind {
  const kind = String(attachment.kind || '').trim().toLowerCase();
  const mimeType = String(attachment.mimeType || '').trim().toLowerCase();
  const source = String(
    attachment.localUrl
      || attachment.absolutePath
      || attachment.originalAbsolutePath
      || attachment.name
      || '',
  ).trim().toLowerCase();

  if (kind === 'image' || mimeType.startsWith('image/') || IMAGE_ATTACHMENT_EXT_RE.test(source)) return 'image';
  if (kind === 'video' || mimeType.startsWith('video/') || VIDEO_ATTACHMENT_EXT_RE.test(source)) return 'video';
  if (kind === 'audio' || mimeType.startsWith('audio/') || AUDIO_ATTACHMENT_EXT_RE.test(source)) return 'audio';
  if (kind === 'text' || mimeType.startsWith('text/') || TEXT_ATTACHMENT_EXT_RE.test(source)) return 'text';
  return 'file';
}

function formatAttachmentSize(size?: number): string {
  if (typeof size !== 'number' || !Number.isFinite(size) || size <= 0) return '';
  if (size >= 1024 * 1024) return `${(size / (1024 * 1024)).toFixed(size >= 10 * 1024 * 1024 ? 0 : 1)} MB`;
  if (size >= 1024) return `${Math.round(size / 1024)} KB`;
  return `${Math.round(size)} B`;
}

function formatRecordingDuration(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `${minutes}:${String(seconds).padStart(2, '0')}`;
}

function getAttachmentKindLabel(kind: ComposerAttachmentVisualKind): string {
  switch (kind) {
    case 'image':
      return '图片';
    case 'video':
      return '视频';
    case 'audio':
      return '音频';
    case 'text':
      return '文档';
    default:
      return '文件';
  }
}

function getAttachmentKindIcon(kind: ComposerAttachmentVisualKind, className: string) {
  switch (kind) {
    case 'image':
      return <ImageIcon className={className} />;
    case 'video':
      return <Film className={className} />;
    case 'audio':
      return <Music2 className={className} />;
    case 'text':
      return <FileText className={className} />;
    default:
      return <FileIcon className={className} />;
  }
}

function ComposerRecordingStatus({
  darkEmbedded,
  elapsedMs,
}: {
  darkEmbedded: boolean;
  elapsedMs: number;
}) {
  return (
    <div className="flex min-w-0 flex-1 items-center gap-2 overflow-hidden px-1" aria-live="polite">
      <div className="flex items-center gap-1.5 shrink-0">
        <span className={clsx('h-2 w-2 rounded-full', darkEmbedded ? 'bg-red-400/90' : 'bg-[#dd6b5b]', 'animate-pulse')} />
      </div>
      <div className="flex min-w-0 flex-1 items-center">
        <div className="relative z-[1] flex h-5 min-w-0 flex-1 items-center justify-center gap-[3px] px-1">
          {RECORDING_WAVE_BARS.map((height, index) => (
            <span
              key={`${index}-${height}`}
              className={clsx(
                'recording-wave-bar w-[2px] shrink-0 rounded-full',
                darkEmbedded ? 'bg-white/68' : 'bg-[#697885]',
              )}
              style={{
                height: `${5 + Math.round(height * 9)}px`,
                animationDelay: `${index * 70}ms`,
              }}
            />
          ))}
        </div>
      </div>
      <div className={clsx('shrink-0 text-[11px] font-medium tabular-nums', darkEmbedded ? 'text-white/58' : 'text-[#8a94a0]')}>
        {formatRecordingDuration(elapsedMs)}
      </div>
    </div>
  );
}

function isImeComposingEvent(event: React.KeyboardEvent<HTMLTextAreaElement>): boolean {
  const synthetic = event as React.KeyboardEvent<HTMLTextAreaElement> & { isComposing?: boolean };
  const native = event.nativeEvent as KeyboardEvent & { isComposing?: boolean; keyCode?: number };
  return Boolean(native?.isComposing) || Boolean(synthetic.isComposing) || native?.keyCode === 229;
}

function decodeBase64DataUrl(dataUrl: string): string {
  const raw = String(dataUrl || '');
  const parts = raw.split(',');
  return parts.length > 1 ? parts[1] : raw;
}

export function blobToBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(decodeBase64DataUrl(String(reader.result || '')));
    reader.onerror = () => reject(reader.error || new Error('音频读取失败'));
    reader.readAsDataURL(blob);
  });
}

export function buildChatModelOptions(settings?: ChatSettingsSnapshot | null): ChatModelOption[] {
  if (!settings) return [];

  const options: ChatModelOption[] = [];
  const defaultSourceId = String(settings.default_ai_source_id || '').trim();
  const prefersOfficialDefault = defaultSourceId.toLowerCase() === 'redbox_official_auto';
  let hasExplicitDefaultSource = false;

  try {
    const parsed = JSON.parse(String(settings.ai_sources_json || '[]')) as Array<Record<string, unknown>>;
    if (Array.isArray(parsed)) {
      for (const item of parsed) {
        if (!item || typeof item !== 'object') continue;
        const sourceId = String(item.id || '').trim();
        if (sourceId && sourceId === defaultSourceId) {
          hasExplicitDefaultSource = true;
        }
        const sourceName = String(item.name || sourceId || 'AI 源').trim();
        const baseURL = String(item.baseURL || item.baseUrl || '').trim();
        const apiKey = String(item.apiKey || item.key || '').trim();
        const explicitModelsMeta = Array.isArray(item.modelsMeta)
          ? item.modelsMeta.filter((value): value is { id?: unknown; capability?: unknown; capabilities?: unknown } => Boolean(value && typeof value === 'object'))
          : [];
        const chatModelIdsFromMeta = explicitModelsMeta
          .filter((value) => modelSupportsChat(value))
          .map((value) => String(value.id || '').trim())
          .filter(Boolean);
        const fallbackCandidates = [
          ...((Array.isArray(item.models) ? item.models : []).map((value) => String(value || '').trim())),
          String(item.model || item.modelName || '').trim(),
        ]
          .filter(Boolean)
          .filter((value) => modelSupportsChat(value));
        const candidates = Array.from(new Set([
          ...chatModelIdsFromMeta,
          ...fallbackCandidates,
        ]));
        for (const modelName of candidates) {
          options.push({
            key: `${sourceId || baseURL || sourceName}::${modelName}`,
            modelName,
            sourceName,
            baseURL,
            apiKey,
            isDefault: Boolean(sourceId && sourceId === defaultSourceId && modelName === String(item.model || item.modelName || '').trim()),
          });
        }
      }
    }
  } catch {
    // ignore malformed ai_sources_json
  }

  const fallbackModel = String(settings.model_name || '').trim();
  if (
    !prefersOfficialDefault
    && !hasExplicitDefaultSource
    && fallbackModel
    && modelSupportsChat(fallbackModel)
  ) {
    options.push({
      key: `fallback::${fallbackModel}`,
      modelName: fallbackModel,
      sourceName: '当前默认源',
      baseURL: String(settings.api_endpoint || '').trim(),
      apiKey: String(settings.api_key || '').trim(),
      isDefault: true,
    });
  }

  const deduped = new Map<string, ChatModelOption>();
  for (const option of options) {
    deduped.set(option.key, option);
  }

  return Array.from(deduped.values());
}

function ComposerAttachmentPreview({
  attachment,
  darkEmbedded,
  variant,
  onRemove,
}: ComposerAttachmentPreviewProps) {
  const visualKind = getAttachmentVisualKind(attachment);
  const isImageAttachment = visualKind === 'image';
  const previewSrc = visualKind === 'image' ? getAttachmentSource(attachment) : '';
  const extLabel = getAttachmentExtLabel(attachment);
  const sizeLabel = formatAttachmentSize(attachment.size);
  const typeLabel = getAttachmentKindLabel(visualKind);
  const frameClass = isImageAttachment
    ? variant === 'empty' ? 'h-[88px] w-[88px]' : 'h-[72px] w-[72px]'
    : variant === 'empty' ? 'h-[92px] w-[70px]' : 'h-[78px] w-[58px]';
  const frameRadiusClass = isImageAttachment
    ? variant === 'empty' ? 'rounded-[18px]' : 'rounded-[16px]'
    : 'rounded-[22px]';
  const metaClass = darkEmbedded ? 'text-white/34' : 'text-text-tertiary/70';
  const titleClass = darkEmbedded ? 'text-white/88' : 'text-text-primary';
  const badgeClass = darkEmbedded
    ? 'border-white/10 bg-white/[0.05] text-white/58'
    : 'border-black/[0.06] bg-[#f7f2e7] text-[#7f715f]';
  const previewShellClass = darkEmbedded
    ? 'border-white/10 bg-[linear-gradient(180deg,rgba(255,255,255,0.08),rgba(255,255,255,0.03))] shadow-[0_12px_34px_rgba(0,0,0,0.35)]'
    : 'border-black/[0.07] bg-[linear-gradient(180deg,#fbf6ec,#f2eadb)] shadow-[0_12px_28px_rgba(110,84,44,0.12)]';
  const removeButtonClass = darkEmbedded
    ? 'border-white/12 bg-[#1b2026] text-white/62 hover:text-white hover:bg-[#222831]'
    : 'border-white bg-white text-[#786d5f] hover:text-[#2d2822] hover:bg-[#f8f4ea]';
  const infoTokens = [typeLabel, extLabel, sizeLabel].filter(Boolean);

  return (
    <div className="flex items-start gap-3">
      <div className="relative shrink-0">
        {previewSrc ? (
          <div className={clsx(
            'overflow-hidden border',
            frameClass,
            frameRadiusClass,
            isImageAttachment ? 'rotate-0' : (variant === 'empty' ? '-rotate-[4deg]' : '-rotate-[3deg]'),
            previewShellClass,
          )}>
            <img src={previewSrc} alt={attachment.name} className="h-full w-full object-cover" />
          </div>
        ) : (
          <div className={clsx(
            'flex items-center justify-center border',
            frameClass,
            frameRadiusClass,
            previewShellClass,
          )}>
            <div className="flex flex-col items-center gap-1.5 px-2 text-center">
              {getAttachmentKindIcon(visualKind, clsx(
                variant === 'empty' ? 'h-5 w-5' : 'h-[18px] w-[18px]',
                darkEmbedded ? 'text-white/68' : 'text-[#7f715f]',
              ))}
              <span className={clsx(
                'max-w-full truncate text-[10px] font-semibold tracking-[0.18em]',
                darkEmbedded ? 'text-white/42' : 'text-[#9d8f7b]',
              )}>
                {extLabel || typeLabel}
              </span>
            </div>
          </div>
        )}
        <button
          type="button"
          onClick={onRemove}
          className={clsx(
            'absolute -bottom-1 -right-1 flex h-7 w-7 items-center justify-center rounded-full border transition-colors',
            removeButtonClass,
          )}
          title="移除文件"
          aria-label={`移除 ${attachment.name}`}
        >
          <X className="h-3.5 w-3.5" />
        </button>
      </div>
      <div className="min-w-0 flex-1 pt-0.5">
        <div className="flex items-center gap-2">
          <div className={clsx('shrink-0 text-[9px] font-medium tracking-[0.12em]', metaClass)}>已添加文件</div>
          <div className={clsx(
            'min-w-0 truncate font-medium opacity-78',
            variant === 'empty' ? 'text-[11px]' : 'text-[10px]',
            titleClass,
          )} title={attachment.name}>
            {attachment.name}
          </div>
        </div>
        <div className="mt-2 mb-0.5">
          {infoTokens.length > 0 ? (
            <div className="flex flex-wrap items-center gap-1.5">
              {infoTokens.map((token) => (
                <span
                  key={token}
                  className={clsx('rounded-full border px-2 py-0.5 text-[10px] font-medium', badgeClass)}
                >
                  {token}
                </span>
              ))}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}

export const ChatComposer = forwardRef<ChatComposerHandle, ChatComposerProps>(function ChatComposer({
  theme = 'default',
  variant = 'main',
  className,
  value,
  onValueChange,
  onSubmit,
  placeholder,
  presetPrompt,
  onPresetPromptChange,
  attachment,
  attachments,
  onPickAttachment,
  onClearAttachment,
  onRemoveAttachment,
  modelOptions = [],
  selectedModelKey = '',
  onSelectedModelKeyChange,
  isBusy = false,
  audioState = 'idle',
  onAudioAction,
  onCancel,
  showCancelWhenBusy = Boolean(onCancel),
  disabled = false,
  readOnly = false,
  trailingContent,
  onFocus,
  onPasteFiles,
  slashCommands = [],
  suppressed = false,
  suppressedLabel = '对话已完成，点击后继续输入...',
  onResumeFromSuppressed,
  textareaMaxHeight = 300,
}, ref) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const modelPickerRef = useRef<HTMLDivElement>(null);
  const [showModelPicker, setShowModelPicker] = useState(false);
  const [isComposing, setIsComposing] = useState(false);
  const [recordingElapsedMs, setRecordingElapsedMs] = useState(0);
  const [slashSelectionIndex, setSlashSelectionIndex] = useState(0);
  const [dismissedSlashValue, setDismissedSlashValue] = useState<string | null>(null);
  const darkEmbedded = theme === 'dark';
  const palette = getChatComposerPalette(theme);
  const composerAttachments = attachments || (attachment ? [attachment] : []);
  const selectedModel = useMemo(
    () => modelOptions.find((item) => item.key === selectedModelKey) || null,
    [modelOptions, selectedModelKey],
  );
  const slashQuery = useMemo(() => {
    const match = value.match(/^\/([^\s]*)$/);
    return match ? match[1].toLocaleLowerCase() : null;
  }, [value]);
  const filteredSlashCommands = useMemo(() => {
    if (slashQuery === null) return [];
    const prefixMatches: ChatComposerSlashCommand[] = [];
    const fuzzyMatches: ChatComposerSlashCommand[] = [];
    for (const command of slashCommands) {
      const name = command.name.replace(/^\//, '').toLocaleLowerCase();
      if (!slashQuery || name.startsWith(slashQuery)) prefixMatches.push(command);
      else if (name.includes(slashQuery) || command.description.toLocaleLowerCase().includes(slashQuery)) {
        fuzzyMatches.push(command);
      }
    }
    return [...prefixMatches, ...fuzzyMatches].slice(0, 10);
  }, [slashCommands, slashQuery]);
  const showSlashMenu = filteredSlashCommands.length > 0 && dismissedSlashValue !== value;
  const presetPromptText = String(presetPrompt?.text || '').trim();
  const submitDisabled = disabled || isBusy || (!value.trim() && !presetPromptText && composerAttachments.length === 0);
  const showAttachmentButton = Boolean(onPickAttachment);
  const showModelSelector = Boolean(onSelectedModelKeyChange);
  const showAudioButton = Boolean(onAudioAction);
  const showCancelButton = Boolean(onCancel) && showCancelWhenBusy && isBusy;
  const canOpenModelPicker = showModelSelector && modelOptions.length > 0;
  const modelPickerClass = darkEmbedded
    ? 'absolute left-0 bottom-full mb-2 w-72 max-h-72 overflow-auto rounded-xl border border-white/10 bg-[#181b20] shadow-xl z-[130]'
    : 'absolute left-0 bottom-full mb-2 w-72 max-h-72 overflow-auto rounded-xl border border-border bg-surface-primary shadow-xl z-[130]';
  const subtleButtonClass = palette.subtleButton;
  const sendButtonClass = submitDisabled ? palette.sendButtonIdle : palette.sendButtonActive;

  const syncHeight = useCallback(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(textarea.scrollHeight, textareaMaxHeight)}px`;
  }, [textareaMaxHeight]);

  const resetHeight = useCallback(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = 'auto';
  }, []);

  useEffect(() => {
    syncHeight();
  }, [attachment, attachments, presetPromptText, syncHeight, suppressed, value, variant]);

  useEffect(() => {
    setSlashSelectionIndex(0);
    if (dismissedSlashValue && dismissedSlashValue !== value) setDismissedSlashValue(null);
  }, [dismissedSlashValue, slashQuery, value]);

  useEffect(() => {
    if (!showModelPicker) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (!modelPickerRef.current?.contains(event.target as Node)) {
        setShowModelPicker(false);
      }
    };
    document.addEventListener('mousedown', handlePointerDown);
    return () => document.removeEventListener('mousedown', handlePointerDown);
  }, [showModelPicker]);

  useEffect(() => {
    if (audioState !== 'recording') {
      setRecordingElapsedMs(0);
      return;
    }
    const startedAt = Date.now();
    setRecordingElapsedMs(0);
    const timer = window.setInterval(() => {
      setRecordingElapsedMs(Date.now() - startedAt);
    }, 120);
    return () => window.clearInterval(timer);
  }, [audioState]);

  useImperativeHandle(ref, () => ({
    focus: () => textareaRef.current?.focus(),
    blur: () => textareaRef.current?.blur(),
    syncHeight,
    resetHeight,
    getTextarea: () => textareaRef.current,
  }), [resetHeight, syncHeight]);

  const handleFormSubmit = useCallback((event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (submitDisabled) return;
    onSubmit();
  }, [onSubmit, submitDisabled]);

  const selectSlashCommand = useCallback((command: ChatComposerSlashCommand) => {
    const commandName = command.name.replace(/^\//, '');
    onValueChange(command.insertion || `/${commandName} `);
    setDismissedSlashValue(null);
    requestAnimationFrame(() => textareaRef.current?.focus());
  }, [onValueChange]);

  const handleKeyDown = useCallback((event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (showSlashMenu) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
        event.preventDefault();
        const direction = event.key === 'ArrowDown' ? 1 : -1;
        setSlashSelectionIndex((current) => (
          (current + direction + filteredSlashCommands.length) % filteredSlashCommands.length
        ));
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        setDismissedSlashValue(value);
        return;
      }
      if ((event.key === 'Enter' && !event.shiftKey) || event.key === 'Tab') {
        event.preventDefault();
        const selected = filteredSlashCommands[slashSelectionIndex] || filteredSlashCommands[0];
        if (selected) selectSlashCommand(selected);
        return;
      }
    }
    if (event.key === 'Enter' && !event.shiftKey && !isComposing && !isImeComposingEvent(event)) {
      event.preventDefault();
      if (!submitDisabled) {
        onSubmit();
      }
    }
  }, [filteredSlashCommands, isComposing, onSubmit, selectSlashCommand, showSlashMenu, slashSelectionIndex, submitDisabled, value]);

  const handlePaste = useCallback((event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    if (!onPasteFiles || disabled || readOnly || isBusy) return;
    const filesFromList = Array.from(event.clipboardData?.files || []);
    const filesFromItems = Array.from(event.clipboardData?.items || [])
      .filter((item) => item.kind === 'file')
      .map((item) => item.getAsFile())
      .filter((file): file is File => Boolean(file));
    const seen = new Set<string>();
    const files = [...filesFromList, ...filesFromItems].filter((file) => {
      const key = `${file.name || 'clipboard'}:${file.type}:${file.size}:${file.lastModified}`;
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
    if (files.length === 0) {
      const html = event.clipboardData?.getData('text/html') || '';
      const text = event.clipboardData?.getData('text/plain') || '';
      const types = Array.from(event.clipboardData?.types || []);
      const hasImageHint = /<img\b/i.test(html) || types.some((type) => type.startsWith('image/'));
      if (!hasImageHint && text.trim()) return;
      event.preventDefault();
      void onPasteFiles([]);
      return;
    }
    event.preventDefault();
    void onPasteFiles(files);
  }, [disabled, isBusy, onPasteFiles, readOnly]);

  const framedInput = composerAttachments.length > 0 || Boolean(presetPrompt);
  const wrapperClass = variant === 'empty' ? 'px-4 pt-4' : 'px-3.5 pt-3';
  const textareaClass = framedInput
    ? variant === 'empty'
      ? 'mt-3 w-full bg-transparent pr-1 pb-1 text-[16px] focus:outline-none resize-none min-h-[64px] max-h-[220px] overflow-y-auto'
      : 'mt-2.5 w-full bg-transparent pr-1 pb-1 text-[14px] focus:outline-none resize-none min-h-[52px] max-h-[180px] overflow-y-auto'
    : variant === 'empty'
      ? 'w-full bg-transparent px-4 py-3 text-[16px] focus:outline-none resize-none min-h-[100px] overflow-y-auto'
      : 'w-full bg-transparent px-3.5 py-2.5 text-[14px] focus:outline-none resize-none min-h-[72px] max-h-[280px] overflow-y-auto';

  const textarea = suppressed ? (
    <button
      type="button"
      onClick={() => onResumeFromSuppressed?.()}
      className={clsx(
        'w-full rounded-2xl py-6 text-left',
        variant === 'empty' ? 'px-4 text-[16px]' : 'px-3.5 text-[14px]',
        darkEmbedded ? 'text-white/45' : 'text-text-tertiary',
      )}
    >
      {suppressedLabel}
    </button>
  ) : (
    <textarea
      ref={textareaRef}
      value={value}
      onChange={(event) => {
        setDismissedSlashValue(null);
        onValueChange(event.target.value);
      }}
      onFocus={onFocus}
      onCompositionStart={() => setIsComposing(true)}
      onCompositionEnd={() => setIsComposing(false)}
      onKeyDown={handleKeyDown}
      onPaste={handlePaste}
      placeholder={presetPrompt ? '继续补充你的具体要求...' : placeholder}
      aria-label={presetPrompt ? '继续补充具体要求' : placeholder}
      className={clsx(textareaClass, palette.text)}
      spellCheck={false}
      autoCorrect="off"
      autoCapitalize="off"
      readOnly={readOnly || isBusy}
      aria-disabled={disabled || isBusy}
      rows={1}
    />
  );

  const presetPromptBlock = presetPrompt ? (
    <div className={clsx(
      'rounded-xl border px-3 py-2.5',
      darkEmbedded
        ? 'border-amber-300/25 bg-amber-300/8 text-amber-100'
        : 'border-amber-300/50 bg-amber-50/70 text-amber-900',
    )}>
      <div className="mb-1.5 flex items-center justify-between gap-2">
        <div className={clsx(
          'text-[11px] font-semibold',
          darkEmbedded ? 'text-amber-100/80' : 'text-amber-700',
        )}>
          {presetPrompt.label || '预设 Prompt'}
        </div>
        {onPresetPromptChange ? (
          <button
            type="button"
            onClick={() => onPresetPromptChange(null)}
            className={clsx(
              'rounded p-1 transition-colors',
              darkEmbedded ? 'text-amber-100/65 hover:bg-white/10 hover:text-amber-50' : 'text-amber-700/70 hover:bg-amber-100 hover:text-amber-900',
            )}
            title="移除预设"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        ) : null}
      </div>
      <textarea
        value={presetPrompt.text}
        onChange={(event) => onPresetPromptChange?.({ ...presetPrompt, text: event.target.value })}
        onFocus={onFocus}
        className={clsx(
          'max-h-32 min-h-[44px] w-full resize-none overflow-y-auto bg-transparent text-[13px] leading-5 focus:outline-none',
          darkEmbedded ? 'placeholder:text-amber-100/30' : 'placeholder:text-amber-700/40',
        )}
        placeholder="填写预设 Prompt"
        aria-label="预设 Prompt 内容"
        spellCheck={false}
        autoCorrect="off"
        autoCapitalize="off"
        readOnly={readOnly || isBusy}
        aria-disabled={disabled || isBusy}
        rows={2}
      />
    </div>
  ) : null;

  const inputContent = presetPromptBlock ? (
    <>
      {presetPromptBlock}
      {textarea}
    </>
  ) : textarea;

  return (
    <form onSubmit={handleFormSubmit} className={clsx('relative w-full', className)}>
      {showSlashMenu ? (
        <div className={clsx(
          'absolute bottom-full left-0 right-0 z-[145] mb-2 max-h-80 overflow-auto rounded-xl border p-1.5 shadow-2xl',
          darkEmbedded ? 'border-white/10 bg-[#181b20]' : 'border-border bg-surface-primary',
        )} role="listbox" aria-label="斜杠命令">
          {filteredSlashCommands.map((command, index) => {
            const active = index === slashSelectionIndex;
            const commandName = command.name.replace(/^\//, '');
            return (
              <button
                key={commandName}
                type="button"
                role="option"
                aria-selected={active}
                onMouseDown={(event) => event.preventDefault()}
                onMouseMove={() => setSlashSelectionIndex(index)}
                onClick={() => selectSlashCommand(command)}
                className={clsx(
                  'flex w-full items-start gap-3 rounded-lg px-3 py-2 text-left transition-colors',
                  active
                    ? darkEmbedded ? 'bg-white/10 text-white' : 'bg-accent-primary/10 text-text-primary'
                    : darkEmbedded ? 'text-white/72 hover:bg-white/6' : 'text-text-secondary hover:bg-surface-secondary/60',
                )}
              >
                <code className="min-w-[112px] shrink-0 text-[13px] font-semibold text-accent-primary">/{commandName}</code>
                <span className="text-[12px] leading-5 opacity-80">{command.description}</span>
              </button>
            );
          })}
        </div>
      ) : null}
      <ChatComposerFrame theme={theme} variant={variant}>
        {composerAttachments.length > 0 ? (
          <div className={wrapperClass}>
            <div className="mb-2 flex max-h-44 flex-wrap gap-x-4 gap-y-3 overflow-y-auto pr-1">
              {composerAttachments.map((item, index) => (
                <div key={`${item.absolutePath || item.originalAbsolutePath || item.localUrl || item.name}-${index}`} className="min-w-0 max-w-full basis-[220px] grow">
                  <ComposerAttachmentPreview
                    attachment={item}
                    darkEmbedded={darkEmbedded}
                    variant={variant}
                    onRemove={() => {
                      if (attachments) {
                        onRemoveAttachment?.(item, index);
                      } else {
                        onClearAttachment?.();
                      }
                    }}
                  />
                </div>
              ))}
            </div>
            {inputContent}
          </div>
        ) : presetPromptBlock ? (
          <div className={wrapperClass}>
            {inputContent}
          </div>
        ) : textarea}

        <div className={clsx('flex items-center gap-2', variant === 'empty' ? 'px-2 pb-1' : 'px-1.5 pb-0.5')}>
          <div className="flex shrink-0 items-center gap-1">
            {showAttachmentButton ? (
              <button type="button" onClick={() => void onPickAttachment?.()} className={clsx('p-2 transition-colors', subtleButtonClass)} title="添加文件">
                <Plus className="h-[18px] w-[18px]" />
              </button>
            ) : null}

            {showModelSelector ? (
              <div ref={modelPickerRef} className="relative flex items-center gap-4 px-2">
                <button
                  type="button"
                  onClick={() => {
                    if (!modelOptions.length) return;
                    setShowModelPicker((current) => !current);
                  }}
                  className={clsx('flex items-center gap-1.5 text-[13px] font-medium transition-colors', subtleButtonClass)}
                >
                  <span className="max-w-[180px] truncate">{selectedModel?.modelName || '默认模型'}</span>
                  <ChevronDown className={clsx('h-3.5 w-3.5 transition-transform', showModelPicker && 'rotate-180')} />
                </button>
                {showModelPicker && (
                  <div className={modelPickerClass}>
                    {canOpenModelPicker ? modelOptions.map((option) => {
                      const active = option.key === selectedModelKey;
                      return (
                        <button
                          key={option.key}
                          type="button"
                          onClick={() => {
                            onSelectedModelKeyChange?.(option.key);
                            setShowModelPicker(false);
                          }}
                          className={clsx(
                            'w-full px-3 py-2.5 text-left transition-colors',
                            active ? 'bg-accent-primary/10 text-text-primary' : darkEmbedded ? 'text-white/68 hover:bg-white/6' : 'text-text-secondary hover:bg-surface-secondary/50',
                          )}
                        >
                          <div className="truncate text-sm font-medium">{option.modelName}</div>
                          <div className="truncate text-[11px] text-text-tertiary">{option.sourceName}</div>
                        </button>
                      );
                    }) : (
                      <div className="px-3 py-2 text-sm text-text-tertiary">请先在设置里配置模型源</div>
                    )}
                  </div>
                )}
              </div>
            ) : null}
          </div>

          {audioState === 'recording' ? (
            <ComposerRecordingStatus darkEmbedded={darkEmbedded} elapsedMs={recordingElapsedMs} />
          ) : (
            <div className="flex-1" />
          )}

          <div className="flex shrink-0 items-center gap-2">
            {showCancelButton ? (
              <button
                type="button"
                onClick={() => void onCancel?.()}
                className={clsx(
                  'rounded-lg p-2 transition-colors',
                  darkEmbedded ? 'text-red-400 hover:bg-red-500/10' : 'text-red-500 hover:bg-red-50',
                )}
                title="停止生成"
              >
                <StopCircle className="h-5 w-5" />
              </button>
            ) : showAudioButton ? (
              <button
                type="button"
                onClick={() => void onAudioAction?.()}
                disabled={audioState === 'transcribing' || disabled}
                className={clsx(
                  'p-2 transition-colors',
                  audioState === 'recording' ? 'text-red-500 hover:text-red-600' : subtleButtonClass,
                  (audioState === 'transcribing' || disabled) && 'cursor-not-allowed opacity-60',
                )}
                title={
                  audioState === 'transcribing'
                    ? '语音转录中'
                    : audioState === 'recording'
                      ? '停止录音并转写'
                      : '语音输入'
                }
              >
                {audioState === 'transcribing' ? (
                  <Loader2 className="h-[18px] w-[18px] animate-spin" />
                ) : audioState === 'recording' ? (
                  <Square className="h-[18px] w-[18px] fill-current" />
                ) : (
                  <Mic className="h-[18px] w-[18px]" />
                )}
              </button>
            ) : null}

            {trailingContent}

            <button
              type="submit"
              disabled={submitDisabled}
              aria-label={isBusy ? '正在生成' : '发送消息'}
              title={isBusy ? '正在生成' : '发送消息'}
              className={clsx('flex h-9 w-9 items-center justify-center rounded-full transition-all duration-200', sendButtonClass)}
            >
              {isBusy ? <Loader2 className="h-4 w-4 animate-spin text-[#b4b2a8]" /> : <ArrowUp className="h-5 w-5" />}
            </button>
          </div>
        </div>
      </ChatComposerFrame>
    </form>
  );
});
