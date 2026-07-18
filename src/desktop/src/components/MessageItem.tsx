import React, { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { clsx } from 'clsx';
import { Components, UrlTransform } from 'react-markdown';
import { Copy, Check, Edit, FileText, FileWarning, FolderOpen, Image, Send, X } from 'lucide-react';
import { ProcessTimeline, ProcessItem } from './ProcessTimeline';
import { SkillActivatedBadge, ThinkingIndicator } from './ThinkingBubble';
import { TodoList, PlanStep } from './TodoList';
import { resolveAssetUrl, isLocalAssetUrl } from '../utils/pathManager';
import { getLiquidGlassMenuItemClassName, LiquidGlassMenuPanel, LiquidGlassMenuSeparator } from '@/components/ui/liquid-glass-menu';
import { StreamingMarkdown } from './chat/StreamingMarkdown';
import { normalizeGenerationParameterBlocks } from '../utils/generationParameterFormatting';
import McpAppFrame, {
  getMcpAppDescriptor,
  type McpAppToolOutput,
} from './McpAppFrame';
import './chat-message.css';

const copyTextWithClipboard = async (text: string): Promise<boolean> => {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    try {
      const textarea = document.createElement('textarea');
      textarea.value = text;
      textarea.style.position = 'fixed';
      textarea.style.opacity = '0';
      document.body.appendChild(textarea);
      textarea.focus();
      textarea.select();
      const ok = document.execCommand('copy');
      document.body.removeChild(textarea);
      return ok;
    } catch {
      return false;
    }
  }
};

const extractNodeText = (value: React.ReactNode): string => {
  if (value == null || typeof value === 'boolean') return '';
  if (typeof value === 'string' || typeof value === 'number') return String(value);
  if (Array.isArray(value)) return value.map(extractNodeText).join('');
  if (React.isValidElement(value)) {
    return extractNodeText((value.props as { children?: React.ReactNode }).children);
  }
  return '';
};

const IMAGE_ATTACHMENT_EXT_RE = /\.(png|jpe?g|webp|gif|bmp|svg|avif)(?:[?#].*)?$/i;
const VIDEO_ATTACHMENT_EXT_RE = /\.(mp4|webm|mov|m4v)(?:[?#].*)?$/i;
const AUDIO_ATTACHMENT_EXT_RE = /\.(mp3|wav|m4a|aac|flac|ogg|opus)(?:[?#].*)?$/i;
const PDF_ATTACHMENT_EXT_RE = /\.pdf(?:[?#].*)?$/i;
const HTML_ATTACHMENT_EXT_RE = /\.html?(?:[?#].*)?$/i;
const RENDERABLE_ARTIFACT_EXT_RE = /\.(png|jpe?g|webp|gif|bmp|svg|avif|mp4|webm|mov|m4v|mp3|wav|m4a|aac|flac|ogg|opus|pdf|html?)(?:[?#].*)?$/i;
const URL_ARTIFACT_SOURCE_RE = /(?:redbox-asset:\/\/[^\s<>"'`,)\]}]+|local-file:\/\/[^\s<>"'`,)\]}]+|file:\/\/[^\s<>"'`,)\]}]+|https?:\/\/[^\s<>"'`,)\]}]+)/gi;
const ABSOLUTE_ARTIFACT_PATH_RE = /(\/(?:Volumes|Users|private|tmp|var)\/[^\s<>"'`,)\]}]+\.(?:png|jpe?g|webp|gif|bmp|svg|avif|mp4|webm|mov|m4v|mp3|wav|m4a|aac|flac|ogg|opus|pdf|html?)(?:[?#][^\s<>"'`,)\]}]+)?)/gi;
const WINDOWS_ABSOLUTE_ARTIFACT_PATH_RE = /([a-zA-Z]:[\\/][^\s<>"'`,)\]}]+\.(?:png|jpe?g|webp|gif|bmp|svg|avif|mp4|webm|mov|m4v|mp3|wav|m4a|aac|flac|ogg|opus|pdf|html?)(?:[?#][^\s<>"'`,)\]}]+)?)/gi;
const WINDOWS_UNC_ARTIFACT_PATH_RE = /(\\\\[^\\\s<>"'`,)\]}]+[\\/][^\s<>"'`,)\]}]+\.(?:png|jpe?g|webp|gif|bmp|svg|avif|mp4|webm|mov|m4v|mp3|wav|m4a|aac|flac|ogg|opus|pdf|html?)(?:[?#][^\s<>"'`,)\]}]+)?)/gi;
const MEDIA_LIBRARY_RELATIVE_SOURCE_RE = /^(?:(?:media[\\/])?generated|imported|projects|video-projects)[\\/]/i;
const MEDIA_LIBRARY_RELATIVE_ARTIFACT_PATH_RE = /(^|[\s(（:："'`])((?:(?:media[\\/])?generated|imported|projects|video-projects)[\\/][^\n<>"'`,)\]}]+?\.(?:png|jpe?g|webp|gif|bmp|svg|avif|mp4|webm|mov|m4v|mp3|wav|m4a|aac|flac|ogg|opus|pdf|html?)(?:[?#][^\s<>"'`,)\]}]+)?)/gim;

type RenderableArtifactKind = 'image' | 'video' | 'audio' | 'pdf' | 'html';

interface RenderableArtifactPreview {
  source: string;
  kind: RenderableArtifactKind;
  label: string;
  start: number;
  end: number;
  hidden?: boolean;
}

type ArtifactContentPart = {
  type: 'text';
  content: string;
} | {
  type: 'artifact';
  artifact: RenderableArtifactPreview;
};

const MEDIA_LIBRARY_FOCUS_EVENT = 'media-library:focus-asset';
const MEDIA_LIBRARY_FOCUS_STORAGE_KEY = 'media-library:focus-asset:v1';

const artifactPathBasename = (value: string): string => {
  const source = String(value || '').trim();
  if (!source) return '生成产物';
  const withoutQuery = source.split(/[?#]/)[0];
  const decoded = (() => {
    try {
      return decodeURIComponent(withoutQuery);
    } catch {
      return withoutQuery;
    }
  })();
  return decoded.split(/[\\/]/).filter(Boolean).pop() || '生成产物';
};

const artifactDisplayLabel = (label: string | undefined, source: string): string => {
  const rawLabel = String(label || '').trim();
  if (
    rawLabel
    && !isRenderableArtifactSource(rawLabel)
    && normalizeComparableArtifactSource(rawLabel) !== normalizeComparableArtifactSource(source)
  ) {
    return rawLabel;
  }
  return artifactPathBasename(source);
};

const getRenderableArtifactKind = (value: string): RenderableArtifactKind | null => {
  const source = String(value || '').trim();
  if (!source || /^javascript:/i.test(source)) return null;
  const lower = source.toLowerCase();
  if (/^data:image\//i.test(source)) return 'image';
  if (/^data:video\//i.test(source)) return 'video';
  if (/^data:audio\//i.test(source)) return 'audio';
  if (/^data:application\/pdf/i.test(source)) return 'pdf';
  if (/^data:text\/html/i.test(source)) return 'html';
  if (IMAGE_ATTACHMENT_EXT_RE.test(lower)) return 'image';
  if (VIDEO_ATTACHMENT_EXT_RE.test(lower)) return 'video';
  if (AUDIO_ATTACHMENT_EXT_RE.test(lower)) return 'audio';
  if (PDF_ATTACHMENT_EXT_RE.test(lower)) return 'pdf';
  if (HTML_ATTACHMENT_EXT_RE.test(lower)) return 'html';
  return null;
};

const isRenderableArtifactSource = (value: string): boolean => {
  const source = String(value || '').trim();
  const kind = getRenderableArtifactKind(source);
  if (!source || !kind) return false;
  // Remote documentation pages found inside tool output are links, not generated
  // artifacts. Embedding them also produces noisy X-Frame-Options errors.
  if (kind === 'html' && /^https?:/i.test(source)) return false;
  if (/^(https?:|data:|blob:|file:|redbox-asset:|local-file:)/i.test(source)) return true;
  return isLocalAssetUrl(source) || RENDERABLE_ARTIFACT_EXT_RE.test(source);
};

const isAlreadyRenderedMarkdownArtifact = (content: string, source: string): boolean => {
  const text = String(content || '');
  const raw = String(source || '').trim();
  if (!text || !raw) return false;
  return text.includes(`](${raw})`) || text.includes(`src="${raw}"`) || text.includes(`src='${raw}'`);
};

const normalizeComparableArtifactSource = (value: string): string => {
  let source = String(value || '').trim();
  if (!source) return '';
  source = source.split(/[?#]/)[0];
  try {
    source = decodeURIComponent(source);
  } catch {
    // Keep the original if it is not URI-encoded.
  }
  return source
    .replace(/^file:\/\/\/?/i, '/')
    .replace(/^local-file:\/\/\/?/i, '/')
    .replace(/^redbox-asset:\/\/asset\//i, '/')
    .replace(/\\/g, '/')
    .replace(/\/+/g, '/')
    .replace(/\/$/, '')
    .toLowerCase();
};

const MANAGED_MEDIA_FILE_PREFIX_RE = /^media_\d+_[a-f0-9]+_/i;

const logicalArtifactKey = (source: string): string => {
  const kind = getRenderableArtifactKind(source);
  if (!kind) return '';
  const basename = artifactPathBasename(source).toLowerCase();
  const logicalBasename = basename.replace(MANAGED_MEDIA_FILE_PREFIX_RE, '');
  return `${kind}:${logicalBasename}`;
};

const artifactSourcePriority = (source: string): number => {
  const normalized = normalizeComparableArtifactSource(source);
  if (normalized.includes('/.redconvert/spaces/') && normalized.includes('/media/generated/')) return 3;
  if (MEDIA_LIBRARY_RELATIVE_SOURCE_RE.test(normalized)) return 3;
  if (normalized.startsWith('/tmp/') || normalized.startsWith('/private/tmp/')) return 1;
  return 2;
};

const findCodeFenceRangeForIndex = (content: string, index: number): { start: number; end: number; inner: string } | null => {
  const fenceRe = /```[^\n`]*\n([\s\S]*?)```/g;
  for (const match of content.matchAll(fenceRe)) {
    const start = match.index ?? 0;
    const end = start + match[0].length;
    if (index >= start && index < end) {
      return { start, end, inner: match[1] || '' };
    }
  }
  return null;
};

const findInlineCodeRangeForIndex = (content: string, index: number): { start: number; end: number; inner: string } | null => {
  const inlineCodeRe = /`([^`\n]+)`/g;
  for (const match of content.matchAll(inlineCodeRe)) {
    const start = match.index ?? 0;
    const end = start + match[0].length;
    if (index >= start && index < end) {
      return { start, end, inner: match[1] || '' };
    }
  }
  return null;
};

const expandArtifactCandidateRange = (
  content: string,
  candidate: { source: string; start: number; end: number },
): { start: number; end: number } => {
  let { start, end } = candidate;
  const fence = findCodeFenceRangeForIndex(content, candidate.start);
  if (fence) {
    const fenceLines = fence.inner
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean);
    const nonArtifactFenceLines = fenceLines.filter((line) => (
      line !== candidate.source
      && line !== '复制'
      && line.toLowerCase() !== 'copy'
    ));
    if (fence.inner.trim() === candidate.source || nonArtifactFenceLines.length === 0) {
      start = fence.start;
      end = fence.end;
    }
  }
  const inlineCode = findInlineCodeRangeForIndex(content, candidate.start);
  if (inlineCode?.inner.trim() === candidate.source) {
    start = inlineCode.start;
    end = inlineCode.end;
  }
  return { start, end };
};

export const extractRenderableArtifactsFromText = (content: string): RenderableArtifactPreview[] => {
  const text = String(content || '');
  if (!text) return [];

  const candidates: Array<{ source: string; start: number; end: number }> = [];
  for (const match of text.matchAll(URL_ARTIFACT_SOURCE_RE)) {
    const source = match[0];
    const start = match.index ?? 0;
    candidates.push({ source, start, end: start + source.length });
  }
  for (const match of text.matchAll(ABSOLUTE_ARTIFACT_PATH_RE)) {
    if (match[1]) {
      const source = match[1];
      const matchStart = match.index ?? 0;
      const offset = match[0].indexOf(source);
      const start = matchStart + Math.max(0, offset);
      candidates.push({ source, start, end: start + source.length });
    }
  }
  for (const pattern of [WINDOWS_ABSOLUTE_ARTIFACT_PATH_RE, WINDOWS_UNC_ARTIFACT_PATH_RE]) {
    for (const match of text.matchAll(pattern)) {
      if (!match[1]) continue;
      const source = match[1];
      const matchStart = match.index ?? 0;
      const offset = match[0].indexOf(source);
      const start = matchStart + Math.max(0, offset);
      candidates.push({ source, start, end: start + source.length });
    }
  }
  for (const match of text.matchAll(MEDIA_LIBRARY_RELATIVE_ARTIFACT_PATH_RE)) {
    if (match[2]) {
      const source = match[2];
      const matchStart = match.index ?? 0;
      const offset = match[0].indexOf(source);
      const start = matchStart + Math.max(0, offset);
      candidates.push({ source, start, end: start + source.length });
    }
  }

  const groupedCandidates = new Map<string, Array<{ source: string; start: number; end: number }>>();
  for (const rawCandidate of candidates) {
    const source = String(rawCandidate.source || '').trim();
    if (!isRenderableArtifactSource(source) || isAlreadyRenderedMarkdownArtifact(text, source)) continue;
    const exactKey = normalizeComparableArtifactSource(resolveAssetUrl(source) || source);
    const key = logicalArtifactKey(source) || exactKey;
    if (!key) continue;
    const group = groupedCandidates.get(key) || [];
    const duplicate = group.some((item) => (
      item.start === rawCandidate.start
      && item.end === rawCandidate.end
      && normalizeComparableArtifactSource(item.source) === normalizeComparableArtifactSource(source)
    ));
    if (!duplicate) {
      group.push({ ...rawCandidate, source });
      groupedCandidates.set(key, group);
    }
  }

  const previews: RenderableArtifactPreview[] = [];
  for (const group of groupedCandidates.values()) {
    const ordered = [...group].sort((a, b) => a.start - b.start);
    const firstCandidate = ordered[0];
    if (!firstCandidate) continue;
    const preferredCandidate = ordered.reduce((preferred, candidate) => (
      artifactSourcePriority(candidate.source) > artifactSourcePriority(preferred.source)
        ? candidate
        : preferred
    ));
    const source = String(preferredCandidate.source || '').trim();
    const kind = getRenderableArtifactKind(source);
    if (!kind) continue;
    const firstRange = expandArtifactCandidateRange(text, firstCandidate);
    previews.push({
      source,
      kind,
      label: artifactPathBasename(source),
      start: firstRange.start,
      end: firstRange.end,
    });

    for (const duplicateCandidate of ordered.slice(1)) {
      const duplicateRange = expandArtifactCandidateRange(text, duplicateCandidate);
      if (previews.some((item) => duplicateRange.start < item.end && duplicateRange.end > item.start)) continue;
      const duplicateKind = getRenderableArtifactKind(duplicateCandidate.source);
      if (!duplicateKind) continue;
      previews.push({
        source: duplicateCandidate.source,
        kind: duplicateKind,
        label: artifactPathBasename(duplicateCandidate.source),
        start: duplicateRange.start,
        end: duplicateRange.end,
        hidden: true,
      });
    }
  }
  return previews.sort((a, b) => a.start - b.start).slice(0, 8);
};

const splitRenderableArtifactContent = (content: string): ArtifactContentPart[] => {
  const text = String(content || '');
  const artifacts = extractRenderableArtifactsFromText(text);
  if (artifacts.length === 0) return [{ type: 'text', content: text }];

  const parts: ArtifactContentPart[] = [];
  let cursor = 0;
  for (const artifact of artifacts) {
    if (artifact.start > cursor) {
      parts.push({ type: 'text', content: text.slice(cursor, artifact.start) });
    }
    parts.push({ type: 'artifact', artifact });
    cursor = artifact.end;
  }
  if (cursor < text.length) {
    parts.push({ type: 'text', content: text.slice(cursor) });
  }
  return parts.filter((part) => part.type === 'artifact' || part.content.length > 0);
};

const ARTIFACT_SOURCE_FIELD_RE = /^(previewUrl|preview_url|absolutePath|absolute_path|localUrl|local_url|url|downloadUrl|download_url|outputUrl|output_url|outputPath|output_path|filePath|file_path|path|relativePath|relative_path|source|href|htmlPath|html_path|pdfPath|pdf_path|videoUrl|video_url|imageUrl|image_url)$/i;
const ARTIFACT_LABEL_FIELD_RE = /^(title|label|name|fileName|filename|id)$/i;

const createRenderableArtifactPreview = (
  source: string,
  label?: string,
  start = 0,
  end = String(source || '').trim().length,
): RenderableArtifactPreview | null => {
  const rawSource = String(source || '').trim();
  const kind = getRenderableArtifactKind(rawSource);
  if (!rawSource || !kind || !isRenderableArtifactSource(rawSource)) return null;
  return {
    source: rawSource,
    kind,
    label: artifactDisplayLabel(label, rawSource),
    start,
    end,
  };
};

const dedupeRenderableArtifacts = (
  artifacts: RenderableArtifactPreview[],
  excludedSources: Set<string> = new Set(),
): RenderableArtifactPreview[] => {
  const exactIndexes = new Map<string, number>();
  const logicalIndexes = new Map<string, number>();
  const output: RenderableArtifactPreview[] = [];
  for (const artifact of artifacts) {
    if (artifact.hidden) continue;
    const key = normalizeComparableArtifactSource(resolveAssetUrl(artifact.source) || artifact.source);
    if (!key || excludedSources.has(key) || exactIndexes.has(key)) continue;
    const logicalKey = logicalArtifactKey(artifact.source);
    const existingIndex = logicalKey ? logicalIndexes.get(logicalKey) : undefined;
    if (existingIndex !== undefined) {
      const existing = output[existingIndex];
      if (artifactSourcePriority(artifact.source) > artifactSourcePriority(existing.source)) {
        exactIndexes.delete(normalizeComparableArtifactSource(resolveAssetUrl(existing.source) || existing.source));
        output[existingIndex] = artifact;
        exactIndexes.set(key, existingIndex);
      }
      continue;
    }
    const nextIndex = output.length;
    exactIndexes.set(key, nextIndex);
    if (logicalKey) logicalIndexes.set(logicalKey, nextIndex);
    output.push(artifact);
  }
  return output;
};

const pickArtifactLabelFromRecord = (record: Record<string, unknown>, fallback?: string): string => {
  for (const [key, value] of Object.entries(record)) {
    if (!ARTIFACT_LABEL_FIELD_RE.test(key)) continue;
    const text = String(value || '').trim();
    if (text && !isRenderableArtifactSource(text)) return text;
  }
  return String(fallback || '').trim();
};

export const extractRenderableArtifactsFromUnknown = (
  value: unknown,
  options: { label?: string; limit?: number } = {},
): RenderableArtifactPreview[] => {
  const previews: RenderableArtifactPreview[] = [];
  const seenObjects = new WeakSet<object>();
  const limit = Math.max(1, options.limit || 16);

  const visit = (node: unknown, depth: number, inheritedLabel?: string, fieldName?: string) => {
    if (previews.length >= limit || depth > 8 || node === undefined || node === null) return;

    if (typeof node === 'string' || typeof node === 'number' || typeof node === 'boolean') {
      const text = String(node || '').trim();
      if (!text) return;
      const direct = createRenderableArtifactPreview(text, inheritedLabel);
      if (direct && (!fieldName || ARTIFACT_SOURCE_FIELD_RE.test(fieldName) || text.length <= 4096)) {
        previews.push(direct);
      }
      if (text.length <= 60_000) {
        for (const artifact of extractRenderableArtifactsFromText(text)) {
          if (artifact.hidden) continue;
          previews.push({
            ...artifact,
            label: artifactDisplayLabel(inheritedLabel || artifact.label, artifact.source),
          });
        }
      }
      return;
    }

    if (Array.isArray(node)) {
      for (const item of node) {
        visit(item, depth + 1, inheritedLabel, fieldName);
        if (previews.length >= limit) break;
      }
      return;
    }

    if (typeof node !== 'object') return;
    if (seenObjects.has(node)) return;
    seenObjects.add(node);

    const record = node as Record<string, unknown>;
    const chatVisibility = String(record.chatVisibility || record.chat_visibility || '').trim().toLowerCase();
    if (chatVisibility === 'library_only') return;
    const recordLabel = pickArtifactLabelFromRecord(record, inheritedLabel);
    const entries = Object.entries(record);
    const sourceEntries = entries.filter(([key]) => ARTIFACT_SOURCE_FIELD_RE.test(key));
    const otherEntries = entries.filter(([key]) => !ARTIFACT_SOURCE_FIELD_RE.test(key));
    for (const [key, child] of [...sourceEntries, ...otherEntries]) {
      visit(child, depth + 1, recordLabel, key);
      if (previews.length >= limit) break;
    }
  };

  visit(value, 0, options.label);
  return dedupeRenderableArtifacts(previews).slice(0, limit);
};

const artifactKeysFromContentParts = (parts: ArtifactContentPart[]): Set<string> => {
  const keys = new Set<string>();
  for (const part of parts) {
    if (part.type !== 'artifact' || part.artifact.hidden) continue;
    const key = normalizeComparableArtifactSource(resolveAssetUrl(part.artifact.source) || part.artifact.source);
    if (key) keys.add(key);
  }
  return keys;
};

const collectMessageToolArtifacts = (
  timeline: ProcessItem[],
  tools: ToolEvent[],
  excludedSources: Set<string>,
): RenderableArtifactPreview[] => {
  const artifacts: RenderableArtifactPreview[] = [];
  for (const item of [...timeline].reverse()) {
    if (item.type === 'tool-call') {
      const label = item.toolData?.name || item.title || '工具产物';
      artifacts.push(...extractRenderableArtifactsFromUnknown(item.toolData?.output, { label, limit: 16 }));
      artifacts.push(...extractRenderableArtifactsFromUnknown(item.content, { label, limit: 4 }));
      continue;
    }
    artifacts.push(...extractRenderableArtifactsFromUnknown(item.content, { label: item.title || '生成产物', limit: 4 }));
  }
  for (const tool of [...(tools || [])].reverse()) {
    artifacts.push(...extractRenderableArtifactsFromUnknown(tool.output, { label: tool.name || '工具产物', limit: 16 }));
  }
  return dedupeRenderableArtifacts(artifacts, excludedSources).slice(0, 2);
};

interface MediaAssetMatch {
  id: string;
  source: string;
}

const normalizeLocalPathForClassification = (value: string): string => {
  let source = String(value || '').trim();
  try {
    source = decodeURIComponent(source);
  } catch {
    // Keep malformed URI text intact so it can still be classified safely.
  }
  return source
    .replace(/^file:\/\/localhost\//i, '/')
    .replace(/^file:\/\/\/?/i, '')
    .replace(/\\/g, '/');
};

const isTemporaryMediaSource = (value: string): boolean => {
  const source = normalizeLocalPathForClassification(value);
  return /^\/?(?:private\/)?tmp\//i.test(source)
    || /^[a-zA-Z]:\/(?:Users\/[^/]+\/AppData\/Local\/Temp|Windows\/Temp)\//i.test(source);
};

const findMediaAssetMatch = async (source: string, limit = 5000): Promise<MediaAssetMatch | null> => {
  const target = normalizeComparableArtifactSource(source);
  if (!target) return null;
  const targetName = artifactPathBasename(source).toLowerCase();
  const targetLogicalKey = logicalArtifactKey(source);
  try {
    const result = await window.ipcRenderer.invoke('media:list', { limit }) as {
      success?: boolean;
      assets?: Array<Record<string, unknown>>;
    };
    const assets = Array.isArray(result?.assets) ? result.assets : [];
    const matched = assets.find((asset) => {
      const id = String(asset.id || '').trim();
      if (!id || asset.exists !== true) return false;
      const rawSources = [
        asset.absolutePath,
        asset.previewUrl,
        asset.relativePath,
        asset.localUrl,
        asset.url,
      ].map((value) => String(value || '').trim()).filter(Boolean);
      return rawSources.some((rawCandidate) => {
        const candidate = normalizeComparableArtifactSource(rawCandidate);
        return candidate === target
          || candidate.endsWith(target)
          || target.endsWith(candidate)
          || (targetName && candidate.endsWith(`/${targetName}`) && target.endsWith(`/${targetName}`))
          || Boolean(targetLogicalKey && logicalArtifactKey(rawCandidate) === targetLogicalKey);
      });
    });
    const id = String(matched?.id || '').trim();
    const matchedSource = [
      matched?.absolutePath,
      matched?.localUrl,
      matched?.previewUrl,
      matched?.url,
      matched?.relativePath,
    ].map((value) => String(value || '').trim()).find(isRenderableArtifactSource) || '';
    return id && matchedSource ? { id, source: matchedSource } : null;
  } catch {
    return null;
  }
};

const INTERNAL_PROTOCOL_BLOCKS = [
  /<tool_call>[\s\S]*?<\/tool_call>/gi,
  /<activated_skill\b[\s\S]*?<\/activated_skill>/gi,
];

const stripInternalProtocolMarkup = (value: string): string => {
  let sanitized = String(value || '');
  for (const pattern of INTERNAL_PROTOCOL_BLOCKS) {
    sanitized = sanitized.replace(pattern, '');
  }
  return sanitized.replace(/\n{3,}/g, '\n\n').trim();
};

function InlineCopyButton({ text, label = '复制' }: { text: string; label?: string }) {
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    if (!text.trim()) return;
    const ok = await copyTextWithClipboard(text);
    if (!ok) return;
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  };

  return (
    <button
      type="button"
      onClick={() => void handleCopy()}
      className="inline-flex items-center gap-1 rounded-md border border-border/60 bg-surface-primary/92 px-1.5 py-0.5 text-[11px] text-text-tertiary shadow-sm transition-colors hover:border-border hover:bg-surface-primary hover:text-text-primary"
      title={label}
    >
      {copied ? <Check className="h-3.5 w-3.5 text-green-500" /> : <Copy className="h-3.5 w-3.5" />}
      <span>{copied ? '已复制' : label}</span>
    </button>
  );
}

const MarkdownCodeBlockContext = React.createContext(false);

function LocalPathLink({ source, label }: { source: string; label?: string }) {
  const path = String(source || '').trim();

  const reveal = async () => {
    if (!path) return;
    try {
      await window.ipcRenderer.invoke('file:show-in-folder', { source: path });
    } catch (error) {
      console.error('Failed to reveal local path in folder:', error);
    }
  };

  return (
    <button
      type="button"
      onClick={() => void reveal()}
      className="inline-flex max-w-full items-center gap-1 align-baseline text-accent-primary underline decoration-accent-primary/35 underline-offset-2 hover:decoration-accent-primary"
      title="在文件夹中显示"
    >
      <FolderOpen className="h-3.5 w-3.5 shrink-0" />
      <span className="break-all text-left font-mono text-[0.92em]">{label || path}</span>
    </button>
  );
}

function MarkdownCode({ className, children, ...props }: any) {
  const isBlock = React.useContext(MarkdownCodeBlockContext);
  const text = extractNodeText(children).replace(/\n$/, '').trim();
  if (!isBlock && text && isLocalAssetUrl(text) && !getRenderableArtifactKind(text)) {
    return <LocalPathLink source={text} />;
  }

  return (
    <code
      className={isBlock
        ? clsx('font-mono text-sm', className)
        : clsx('rounded bg-surface-secondary px-1 py-0.5 font-mono text-[0.92em] text-text-primary', className)}
      {...props}
    >
      {children}
    </code>
  );
}

function CopyableCodeBlock({ children }: { children: React.ReactNode }) {
  const text = extractNodeText(children).replace(/\n$/, '');

  return (
    <div className="group relative my-3 w-full max-w-full overflow-hidden rounded-lg border border-border/70 bg-surface-secondary/45">
      <div className="absolute right-2 top-2 z-10 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
        <InlineCopyButton text={text} label="复制" />
      </div>
      <MarkdownCodeBlockContext.Provider value>
        <pre className="w-full max-w-full overflow-x-auto px-3 py-2.5 pr-14">
          {children}
        </pre>
      </MarkdownCodeBlockContext.Provider>
    </div>
  );
}

function CopyableBlockquote({ children }: { children: React.ReactNode }) {
  const text = extractNodeText(children).trim();

  return (
    <div className="group my-3 rounded-xl border border-border/80 bg-surface-secondary/40 p-3">
      <div className="mb-2 flex items-center justify-end">
        <InlineCopyButton text={text} label="复制引用" />
      </div>
      <blockquote className="border-l-2 border-accent-primary/45 pl-4 text-text-secondary">
        {children}
      </blockquote>
    </div>
  );
}

function UnresolvedArtifactCard({ source, label }: { source: string; label?: string }) {
  const title = artifactDisplayLabel(label, source) || '产物文件';
  return (
    <div
      data-testid="artifact-unresolved"
      className="my-3 flex w-full max-w-full items-start gap-3 rounded-lg border border-border bg-surface-secondary/55 px-3 py-3"
    >
      <FileWarning className="mt-0.5 h-4 w-4 shrink-0 text-text-tertiary" />
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium text-text-primary">{title}</div>
        <div className="mt-1 break-all font-mono text-[11px] text-text-tertiary">{source}</div>
        <div className="mt-1 text-xs text-text-tertiary">产物尚未登记到素材库，无法安全预览。</div>
      </div>
      <InlineCopyButton text={source} label="复制引用" />
    </div>
  );
}

const isVerifiedArtifactFrameUrl = (
  source: string,
  resolvedUrl: string,
  kind: Extract<RenderableArtifactKind, 'pdf' | 'html'>,
): boolean => {
  const rawSource = String(source || '').trim();
  const url = String(resolvedUrl || '').trim();
  if (!rawSource || !url) return false;

  if (/^blob:/i.test(url)) return true;
  if (kind === 'html' && /^data:text\/html(?:[;,]|$)/i.test(url)) return true;
  if (kind === 'pdf' && /^data:application\/pdf(?:[;,]|$)/i.test(url)) return true;

  if (/^https?:/i.test(url)) {
    try {
      const parsed = new URL(url);
      if (typeof window !== 'undefined') {
        const current = window.location;
        if (parsed.protocol === current.protocol && parsed.host === current.host) {
          return false;
        }
      }
      return true;
    } catch {
      return false;
    }
  }

  if (!isLocalAssetUrl(rawSource)) return false;
  return /^(?:asset:|redbox-asset:|local-file:|file:)/i.test(url);
};

type ManagedArtifactResolution =
  | { status: 'loading' }
  | { status: 'resolved'; source: string }
  | { status: 'unresolved' };

const requiresManagedArtifactResolution = (source: string): boolean => {
  const kind = getRenderableArtifactKind(source);
  return isTemporaryMediaSource(source)
    || MEDIA_LIBRARY_RELATIVE_SOURCE_RE.test(source)
    || ((kind === 'html' || kind === 'pdf') && isLocalAssetUrl(source));
};

function ManagedArtifactSource({
  source,
  label,
  children,
}: {
  source: string;
  label?: string;
  children: (resolvedSource: string, retry: () => void) => React.ReactNode;
}) {
  const requiresMediaLibraryResolution = requiresManagedArtifactResolution(source);
  const requestSequence = useRef(0);
  const [resolution, setResolution] = useState<ManagedArtifactResolution>(() => (
    requiresMediaLibraryResolution
      ? { status: 'loading' }
      : { status: 'resolved', source }
  ));

  const resolveFromMediaLibrary = useCallback(async () => {
    const requestId = ++requestSequence.current;
    if (!requiresMediaLibraryResolution) {
      setResolution({ status: 'resolved', source });
      return;
    }
    setResolution({ status: 'loading' });
    const matched = await findMediaAssetMatch(source, 1000);
    if (requestId !== requestSequence.current) return;
    setResolution(matched?.source
      ? { status: 'resolved', source: matched.source }
      : { status: 'unresolved' });
  }, [requiresMediaLibraryResolution, source]);

  useEffect(() => {
    void resolveFromMediaLibrary();
    return () => {
      requestSequence.current += 1;
    };
  }, [resolveFromMediaLibrary]);

  if (resolution.status === 'loading') {
    return <div className="my-3 h-32 w-full max-w-full animate-pulse rounded-xl border border-border bg-surface-secondary/60" />;
  }
  if (resolution.status === 'unresolved') {
    return <UnresolvedArtifactCard source={source} label={label} />;
  }

  return <>{children(resolution.source, () => void resolveFromMediaLibrary())}</>;
}

// Legacy types for compatibility (will be migrated)
export interface ToolEvent {
  id: string;
  callId: string;
  name: string;
  input: unknown;
  output?: McpAppToolOutput;
  description?: string;
  status: 'running' | 'done' | 'failed';
}

export interface SkillEvent {
  name: string;
  description: string;
}

export interface UploadedFileMessageAttachment {
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
  extractedText?: string;
  extractionFormat?: string;
  extractionTruncated?: boolean;
  extractionWarning?: string;
  extractionError?: string;
  requiresMultimodal?: boolean;
}

export interface Message {
  id: string;
  role: 'user' | 'ai';
  messageType?: 'reply' | 'thinking';
  content: string;
  displayContent?: string;
  attachment?: {
    type: 'youtube-video';
    title: string;
    thumbnailUrl?: string;
    videoId?: string;
  } | {
    type: 'wander-references';
    title?: string;
    items: Array<{
      title: string;
      itemType: 'note' | 'video';
      tag?: string;
      folderPath?: string;
      summary?: string;
      cover?: string;
    }>;
  } | UploadedFileMessageAttachment;
  attachments?: UploadedFileMessageAttachment[];
  // New unified timeline
  timeline: ProcessItem[];
  // Plan steps
  plan?: PlanStep[];

  // Legacy fields (kept for compatibility during migration, but UI will prefer timeline)
  thinking?: string;
  tools: ToolEvent[];
  activatedSkill?: SkillEvent;

  isStreaming?: boolean;
  processingStartedAt?: number;
  processingFinishedAt?: number;
  suppressPendingIndicator?: boolean;
}

interface MessageItemProps {
  msg: Message;
  copiedMessageId: string | null;
  onCopyMessage: (id: string, content: string) => void;
  onForkMessage?: (message: Message, content: string) => Promise<boolean | void> | boolean | void;
  onMcpAppMessage?: (content: string) => Promise<unknown> | unknown;
  workflowPlacement?: 'top' | 'bottom';
  workflowVariant?: 'default' | 'compact';
  workflowEmphasis?: 'default' | 'thoughts-first';
  workflowDisplayMode?: 'all' | 'thoughts-only';
  showAttachments?: boolean;
}

interface ImageContextMenuState {
  visible: boolean;
  x: number;
  y: number;
  src: string;
  actionSource: string;
}

function formatProcessingElapsed(totalMs: number): string {
  const safeMs = Number.isFinite(totalMs) ? Math.max(0, totalMs) : 0;
  const totalSeconds = Math.floor(safeMs / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;

  if (hours > 0) {
    return `${hours}h ${minutes}m ${seconds}s`;
  }
  if (minutes > 0) {
    return `${minutes}m ${seconds}s`;
  }
  return `${seconds}s`;
}

function ProcessingTimerBadge({
  startedAt,
  finishedAt,
  isStreaming,
}: {
  startedAt: number;
  finishedAt?: number;
  isStreaming?: boolean;
}) {
  const [liveNow, setLiveNow] = useState(() => Date.now());

  useEffect(() => {
    if (!isStreaming) return;
    setLiveNow(Date.now());
    const timer = window.setInterval(() => {
      setLiveNow(Date.now());
    }, 1000);
    return () => window.clearInterval(timer);
  }, [isStreaming, startedAt]);

  const endAt = isStreaming ? liveNow : (finishedAt ?? liveNow);
  const elapsedLabel = formatProcessingElapsed(endAt - startedAt);

  return (
    <div className="chat-processing-timer" aria-live="off">
      <span className="chat-processing-timer__label">{isStreaming ? '已运行' : '耗时'}</span>
      <span className="chat-processing-timer__value">{elapsedLabel}</span>
    </div>
  );
}

function pendingStatusLabelFromTimeline(items: ProcessItem[]): string {
  const latestRunning = [...(items || [])].reverse().find((item) => item.status === 'running');
  if (!latestRunning) return '正在思考';
  if (latestRunning.type === 'tool-call') {
    const name = latestRunning.toolData?.name || latestRunning.title || '工具';
    return `正在调用 ${name}`;
  }
  if (latestRunning.type === 'cli-install') return `正在安装 ${latestRunning.cliData?.toolName || latestRunning.title || '工具'}`;
  if (latestRunning.type === 'cli-exec') return `正在执行 ${latestRunning.cliData?.toolName || latestRunning.title || '命令'}`;
  if (latestRunning.type === 'cli-escalation') return latestRunning.title || '等待权限确认';
  if (latestRunning.type === 'cli-verify') return latestRunning.title || '正在校验结果';
  if (latestRunning.type === 'thought') return '正在思考';
  const title = String(latestRunning.title || latestRunning.content || '').trim();
  if (!title || title === 'thinking') return '正在思考';
  return title.startsWith('正在') ? title : `正在${title}`;
}

const transformMarkdownUrl: UrlTransform = (url) => {
  const value = String(url || '').trim();
  if (!value) return '';

  if (isLocalAssetUrl(value)) {
    return resolveAssetUrl(value);
  }

  // Keep relative URLs and common safe protocols.
  if (/^\.{0,2}\//.test(value) || /^[a-zA-Z0-9._-]+(?:\/[a-zA-Z0-9._-]+)*$/.test(value)) {
    return value;
  }
  if (/^(https?:|mailto:|tel:|data:)/i.test(value)) {
    return value;
  }

  return '';
};

const MARKDOWN_COMPONENTS: Components = {
  code({ node, ...props }: any) {
    return <MarkdownCode {...props} />;
  },
  pre({ children }: any) {
    return <CopyableCodeBlock>{children}</CopyableCodeBlock>;
  },
  blockquote({ children }: any) {
    return <CopyableBlockquote>{children}</CopyableBlockquote>;
  },
  table({ children }: any) {
    return (
      <div className="overflow-x-auto my-3">
        <table className="min-w-full border-collapse border border-border text-sm">
          {children}
        </table>
      </div>
    );
  },
  th({ children }: any) {
    return <th className="border border-border bg-surface-secondary px-4 py-2 text-left font-medium">{children}</th>;
  },
  td({ children }: any) {
    return <td className="border border-border px-4 py-2">{children}</td>;
  },
  a({ children, href }: any) {
    return <a href={href} className="text-accent-primary hover:underline" target="_blank" rel="noopener noreferrer">{children}</a>;
  },
  ul({ children }: any) {
    return <ul className="list-disc list-outside ml-5 my-2 space-y-1">{children}</ul>;
  },
  ol({ children }: any) {
    return <ol className="list-decimal list-outside ml-5 my-2 space-y-1">{children}</ol>;
  },
  p({ children }: any) {
    return <p className="my-2 break-words whitespace-pre-wrap">{children}</p>;
  },
};

const stringifyValue = (value: unknown): string => {
  if (value === undefined) return '';
  if (typeof value === 'string') return value;
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
};

const timelineRenderSignature = (items?: ProcessItem[]): string => {
  const list = Array.isArray(items) ? items : [];
  return list.map((item) => [
    item.id,
    item.type,
    item.title || '',
    item.content || '',
    item.status,
    item.toolData?.name || '',
    stringifyValue(item.toolData?.input),
    item.toolData?.output || '',
    item.skillData?.name || '',
    item.skillData?.description || '',
    item.cliData?.commandPreview || '',
    item.cliData?.logPreview || '',
    item.cliData?.verificationSummary || '',
    item.duration ?? '',
  ].join('\u001f')).join('\u001e');
};

export const MessageItem = memo(({
  msg,
  copiedMessageId,
  onCopyMessage,
  onForkMessage,
  onMcpAppMessage,
  workflowPlacement = 'top',
  workflowVariant = 'default',
  workflowEmphasis = 'default',
  workflowDisplayMode = 'all',
  showAttachments = true,
}: MessageItemProps) => {
  const isUser = msg.role === 'user';
  const uploadedAttachments = msg.attachments?.length
    ? msg.attachments
    : msg.attachment?.type === 'uploaded-file'
      ? [msg.attachment]
      : [];
  const isThinkingMessage = !isUser && msg.messageType === 'thinking';
  const sanitizedAssistantContent = !isUser
    ? stripInternalProtocolMarkup(String(msg.content || ''))
    : String(msg.content || '');
  const displayAssistantContent = useMemo(
    () => (!isUser ? normalizeGenerationParameterBlocks(sanitizedAssistantContent) : sanitizedAssistantContent),
    [isUser, sanitizedAssistantContent],
  );
  const aiContentRef = useRef<HTMLDivElement | null>(null);
  const forkDraftRef = useRef<HTMLTextAreaElement | null>(null);
  const [previewImage, setPreviewImage] = useState<{ src: string; alt: string } | null>(null);
  const [isForkEditing, setIsForkEditing] = useState(false);
  const [forkDraftContent, setForkDraftContent] = useState('');
  const [isForkSubmitting, setIsForkSubmitting] = useState(false);
  const [imageMenu, setImageMenu] = useState<ImageContextMenuState>({
    visible: false,
    x: 0,
    y: 0,
    src: '',
    actionSource: '',
  });
  const filteredTimeline = useMemo(
    () => workflowDisplayMode === 'thoughts-only'
      ? (msg.timeline || []).filter((item) => item.type === 'thought')
      : (msg.timeline || []),
    [msg.timeline, workflowDisplayMode],
  );
  const assistantContentParts = useMemo(
    () => (!isUser ? splitRenderableArtifactContent(displayAssistantContent) : []),
    [isUser, displayAssistantContent],
  );
  const assistantArtifactKeys = useMemo(
    () => artifactKeysFromContentParts(assistantContentParts),
    [assistantContentParts],
  );
  const toolArtifactPreviews = useMemo(
    () => (!isUser
      && !msg.isStreaming
      && workflowDisplayMode !== 'thoughts-only'
      ? collectMessageToolArtifacts(filteredTimeline, msg.tools || [], assistantArtifactKeys)
      : []),
    [isUser, msg.isStreaming, workflowDisplayMode, filteredTimeline, msg.tools, assistantArtifactKeys],
  );
  const mcpApps = useMemo(() => (isUser ? [] : (msg.tools || []).flatMap((tool) => {
    const descriptor = getMcpAppDescriptor(tool.name, tool.output);
    return descriptor && tool.output
      ? [{ tool, descriptor }]
      : [];
  })), [isUser, msg.tools]);
  const showWorkflowDetails = workflowDisplayMode !== 'thoughts-only';
  const hasAssistantResponseContent = !isUser && Boolean(sanitizedAssistantContent);
  const showPendingThinkingIndicator = !isUser
    && !isThinkingMessage
    && !msg.suppressPendingIndicator
    && Boolean(msg.isStreaming && !hasAssistantResponseContent);
  const showProcessingTimer = !isUser && !isThinkingMessage && typeof msg.processingStartedAt === 'number' && Number.isFinite(msg.processingStartedAt);
  const hasRenderableMessageContent = isUser
    ? Boolean(msg.displayContent || msg.content || (msg.isStreaming && !msg.thinking))
    : hasAssistantResponseContent || showPendingThinkingIndicator || toolArtifactPreviews.length > 0;
  const showTimeline = !isUser && !isThinkingMessage && filteredTimeline.length > 0;
  const showLegacyWorkflow = !isUser
    && !isThinkingMessage
    && filteredTimeline.length === 0
    && (msg.thinking || (showWorkflowDetails && (msg.tools.length > 0 || msg.activatedSkill)));
  const showWorkflowOnTop = workflowPlacement === 'top';
  const latestTimelineThought = !isUser
    ? [...(msg.timeline || [])]
        .reverse()
        .find((item) => item.type === 'thought' && String(item.content || '').trim())
    : undefined;
  const activeThoughtContent = !isUser
    ? stripInternalProtocolMarkup(String(latestTimelineThought?.content || msg.thinking || ''))
    : '';
  const showStreamingThought = !isUser && !isThinkingMessage && Boolean(msg.isStreaming && activeThoughtContent);
  const pendingStatusLabel = useMemo(
    () => pendingStatusLabelFromTimeline(filteredTimeline),
    [filteredTimeline],
  );

  useEffect(() => {
    if (!imageMenu.visible) return;
    const closeMenu = () => setImageMenu((prev) => ({ ...prev, visible: false }));
    window.addEventListener('click', closeMenu);
    return () => {
      window.removeEventListener('click', closeMenu);
    };
  }, [imageMenu.visible]);

  const syncForkDraftHeight = useCallback(() => {
    const textarea = forkDraftRef.current;
    if (!textarea) return;
    textarea.style.height = 'auto';
    textarea.style.height = `${Math.min(Math.max(textarea.scrollHeight, 72), 260)}px`;
  }, []);

  useEffect(() => {
    if (!isForkEditing) return;
    const frame = window.requestAnimationFrame(() => {
      const textarea = forkDraftRef.current;
      if (!textarea) return;
      textarea.focus();
      textarea.setSelectionRange(textarea.value.length, textarea.value.length);
      syncForkDraftHeight();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [isForkEditing, syncForkDraftHeight]);

  const openImageMenu = useCallback((x: number, y: number, source: string, actionSource?: string) => {
    const normalized = resolveAssetUrl(String(source || '').trim());
    const rawActionSource = String(actionSource || source || '').trim();
    if (!normalized || !rawActionSource) return;
    setImageMenu({
      visible: true,
      x,
      y,
      src: normalized,
      actionSource: rawActionSource,
    });
  }, []);

  const handleImageContextMenu = useCallback((
    event: React.MouseEvent<HTMLImageElement>,
    source: string,
    actionSource?: string,
  ) => {
    event.preventDefault();
    openImageMenu(event.clientX, event.clientY, source, actionSource);
  }, [openImageMenu]);

  const handleMediaContextMenu = useCallback((
    event: React.MouseEvent<HTMLElement>,
    source: string,
    actionSource?: string,
  ) => {
    event.preventDefault();
    openImageMenu(event.clientX, event.clientY, source, actionSource);
  }, [openImageMenu]);

  const handleCopyArtifact = useCallback(async (source: string, kind: RenderableArtifactKind) => {
    const actionSource = String(source || '').trim();
    if (!actionSource) return;
    if (kind === 'image') {
      try {
        const result = await window.ipcRenderer.invoke('file:copy-image', { source: actionSource }) as { success?: boolean };
        if (result?.success) return;
      } catch {
        // Fall back to copying the source reference for remote or unsupported images.
      }
    }
    await copyTextWithClipboard(actionSource);
  }, []);

  const findMediaAssetForSource = useCallback(async (source: string): Promise<{ id: string; source: string } | null> => {
    return findMediaAssetMatch(source, 5000);
  }, []);

  const handleOpenArtifactInMediaLibrary = useCallback(async (source: string) => {
    const actionSource = String(source || '').trim();
    if (!actionSource) return;
    const matched = await findMediaAssetForSource(actionSource);
    if (matched?.id) {
      const focusPayload = { assetId: matched.id, source: actionSource, at: Date.now() };
      try {
        window.localStorage.setItem(MEDIA_LIBRARY_FOCUS_STORAGE_KEY, JSON.stringify(focusPayload));
      } catch {
        // Ignore storage failures; the live event below still works when the page is mounted.
      }
      window.dispatchEvent(new CustomEvent('redbox:navigate', { detail: { view: 'media-library' } }));
      window.dispatchEvent(new CustomEvent(MEDIA_LIBRARY_FOCUS_EVENT, { detail: focusPayload }));
      return;
    }

    window.dispatchEvent(new CustomEvent('redbox:navigate', { detail: { view: 'media-library' } }));
    try {
      await window.ipcRenderer.invoke('file:show-in-folder', { source: actionSource });
    } catch {
      // The media library navigation above is still useful even if Finder cannot reveal the file.
    }
  }, [findMediaAssetForSource]);

  const renderArtifactPreview = useCallback((source: string, label?: string) => {
    const rawSource = String(source || '').trim();
    if (!rawSource || !getRenderableArtifactKind(rawSource)) return null;

    const renderResolvedArtifact = (managedSource: string, retry: () => void) => {
      const mediaUrl = resolveAssetUrl(managedSource);
      const kind = getRenderableArtifactKind(managedSource || mediaUrl);
      if (!mediaUrl || !kind) return null;
      const title = artifactDisplayLabel(label, managedSource) || '生成产物';
      if ((kind === 'pdf' || kind === 'html') && !isVerifiedArtifactFrameUrl(managedSource, mediaUrl, kind)) {
        return <UnresolvedArtifactCard source={managedSource} label={title} />;
      }
      const frameClass = 'group relative my-3 block w-full max-w-full overflow-hidden rounded-xl border border-border bg-surface-secondary shadow-sm';
      const actionSource = managedSource || mediaUrl;
      const copyTitle = kind === 'image' ? '复制图片' : '复制产物引用';
      const actions = (
        <div className="absolute right-2 top-2 z-10 flex items-center gap-1 rounded-lg border border-white/30 bg-black/45 p-1 text-white shadow-lg backdrop-blur-md">
          <button
            type="button"
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void handleCopyArtifact(actionSource, kind);
            }}
            className="inline-flex h-7 w-7 items-center justify-center rounded-md transition hover:bg-white/[0.18]"
            title={copyTitle}
            aria-label={copyTitle}
          >
            <Copy className="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              void handleOpenArtifactInMediaLibrary(actionSource);
            }}
            className="inline-flex h-7 w-7 items-center justify-center rounded-md transition hover:bg-white/[0.18]"
            title="在素材库打开"
            aria-label="在素材库打开"
          >
            <Image className="h-3.5 w-3.5" />
          </button>
        </div>
      );

      if (kind === 'image') {
        return (
          <div className="group relative my-3 inline-block max-w-full overflow-hidden rounded-xl border border-border bg-surface-secondary shadow-sm">
            {actions}
            <img
              src={mediaUrl}
              alt={title}
              className="max-h-[28rem] w-auto max-w-full cursor-zoom-in object-contain"
              onError={retry}
              onClick={() => setPreviewImage({ src: mediaUrl, alt: title })}
              onContextMenu={(event) => handleImageContextMenu(event, mediaUrl, managedSource)}
              title="点击预览"
            />
          </div>
        );
      }

      if (kind === 'video') {
        return (
          <div className={frameClass}>
            {actions}
            <video
              src={mediaUrl}
              controls
              preload="metadata"
              className="block max-h-[32rem] w-full max-w-full bg-black"
              onError={retry}
              onContextMenu={(event) => handleMediaContextMenu(event, mediaUrl, managedSource)}
            />
          </div>
        );
      }

      if (kind === 'audio') {
        return (
          <div className={clsx(frameClass, 'p-3 pt-10')}>
            {actions}
            <span className="mb-2 block truncate text-xs text-text-tertiary">{title}</span>
            <audio src={mediaUrl} controls className="block w-full" onError={retry} />
          </div>
        );
      }

      if (kind === 'pdf' || kind === 'html') {
        return (
          <div className={frameClass}>
            {actions}
            <span className="flex items-center justify-between gap-3 border-b border-border px-3 py-2 pr-20 text-xs text-text-tertiary">
              <span className="truncate">{title}</span>
              <a href={mediaUrl} target="_blank" rel="noopener noreferrer" className="shrink-0 text-accent-primary hover:underline">
                打开
              </a>
            </span>
            <iframe
              src={mediaUrl}
              title={title}
              className="block h-[30rem] w-full bg-white"
              sandbox={kind === 'html' ? 'allow-scripts' : undefined}
              referrerPolicy="no-referrer"
            />
          </div>
        );
      }

      return null;
    };

    return (
      <ManagedArtifactSource source={rawSource} label={label}>
        {renderResolvedArtifact}
      </ManagedArtifactSource>
    );
  }, [handleCopyArtifact, handleImageContextMenu, handleMediaContextMenu, handleOpenArtifactInMediaLibrary]);

  const handleCopyImage = async () => {
    if (!imageMenu.actionSource) return;
    try {
      const result = await window.ipcRenderer.invoke('file:copy-image', { source: imageMenu.actionSource }) as { success?: boolean };
      if (!result?.success && /^https?:\/\//i.test(imageMenu.actionSource) && navigator.clipboard?.writeText) {
        await navigator.clipboard.writeText(imageMenu.actionSource);
      }
    } catch (error) {
      console.error('Failed to copy image:', error);
    } finally {
      setImageMenu((prev) => ({ ...prev, visible: false }));
    }
  };

  const handleShowInFolder = async () => {
    if (!imageMenu.actionSource) return;
    if (!isLocalAssetUrl(imageMenu.actionSource)) {
      setImageMenu((prev) => ({ ...prev, visible: false }));
      return;
    }
    try {
      await window.ipcRenderer.invoke('file:show-in-folder', { source: imageMenu.actionSource });
    } catch (error) {
      console.error('Failed to show image in folder:', error);
    } finally {
      setImageMenu((prev) => ({ ...prev, visible: false }));
    }
  };

  const menuSupportsReveal = isLocalAssetUrl(imageMenu.actionSource);
  const markdownComponents = useMemo<Components>(() => ({
    ...MARKDOWN_COMPONENTS,
    a({ children, href }: any) {
      const rawHref = String(href || '').trim();
      const rawLabel = extractNodeText(children).trim();
      const label = artifactDisplayLabel(rawLabel || undefined, rawHref);
      if (isRenderableArtifactSource(rawHref)) {
        const preview = renderArtifactPreview(rawHref, label);
        if (preview) {
          return <span className="block">{preview}</span>;
        }
      }
      return <a href={href} className="text-accent-primary hover:underline" target="_blank" rel="noopener noreferrer">{children}</a>;
    },
    img({ src, alt }: any) {
      const rawSource = String(src || '').trim();
      const preview = renderArtifactPreview(rawSource, artifactDisplayLabel(alt, rawSource));
      if (!preview) return <span className="text-xs text-text-tertiary">资源地址无效</span>;
      return preview;
    },
  }), [renderArtifactPreview]);

  const isUploadedImageAttachment = useCallback((attachment: Extract<NonNullable<Message['attachment']>, { type: 'uploaded-file' }>) => {
    const kind = String(attachment.kind || '').trim().toLowerCase();
    const mimeType = String(attachment.mimeType || '').trim().toLowerCase();
    const source = String(
      attachment.localUrl
        || attachment.absolutePath
        || attachment.originalAbsolutePath
        || attachment.name
        || '',
    ).trim().toLowerCase();

    return kind === 'image' || mimeType.startsWith('image/') || IMAGE_ATTACHMENT_EXT_RE.test(source);
  }, []);

  const resolveUploadedAttachmentSource = useCallback((attachment: Extract<NonNullable<Message['attachment']>, { type: 'uploaded-file' }>) => {
    const preferred = String(
      attachment.thumbnailDataUrl
        || attachment.localUrl
        || attachment.absolutePath
        || attachment.originalAbsolutePath
        || '',
    ).trim();
    if (!preferred) return '';
    return preferred.startsWith('data:') ? preferred : resolveAssetUrl(preferred);
  }, []);

  const resolveUploadedAttachmentActionSource = useCallback((attachment: Extract<NonNullable<Message['attachment']>, { type: 'uploaded-file' }>) => (
    String(
      attachment.localUrl
        || attachment.absolutePath
        || attachment.originalAbsolutePath
        || '',
    ).trim()
  ), []);

  const renderYoutubeCard = (card: { title: string; thumbnailUrl?: string }) => (
    <div className="bg-white/10 rounded-lg overflow-hidden">
      <div className="flex items-center gap-3 p-2.5">
        {card.thumbnailUrl ? (
          <img
            src={resolveAssetUrl(card.thumbnailUrl)}
            alt={card.title}
            className="w-20 h-12 object-cover rounded"
          />
        ) : (
          <div className="w-20 h-12 bg-red-600 rounded flex items-center justify-center">
            <span className="text-white text-xl">▶</span>
          </div>
        )}
        <div className="flex-1 min-w-0">
          <div className="text-xs opacity-70">YouTube 视频</div>
          <div className="text-sm font-medium truncate" title={card.title}>
            {card.title.length > 18 ? `${card.title.substring(0, 18)}...` : card.title}
          </div>
        </div>
      </div>
    </div>
  );

  const renderWanderReferenceCards = (attachment: Extract<NonNullable<Message['attachment']>, { type: 'wander-references' }>) => (
    <div className="mt-2 w-full max-w-[540px] rounded-2xl border border-border bg-surface-primary/95 p-2 shadow-sm">
      <div className="px-1 pb-2 text-[11px] font-medium text-text-tertiary">
        {attachment.title || '参考素材'}
      </div>
      <div className="space-y-2">
        {attachment.items.slice(0, 3).map((item, index) => (
          <div
            key={`${item.folderPath || item.title}-${index}`}
            className="flex items-start gap-3 rounded-xl border border-border bg-surface-secondary/60 p-2.5"
          >
            {item.cover ? (
              <img
                src={resolveAssetUrl(item.cover)}
                alt={item.title}
                className="h-14 w-14 rounded-lg object-cover shrink-0"
              />
            ) : (
              <div className="h-14 w-14 rounded-lg bg-surface-secondary border border-border flex items-center justify-center text-lg shrink-0">
                {item.itemType === 'video' ? '▶' : '📝'}
              </div>
            )}
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 text-[11px] text-text-tertiary">
                <span>{item.itemType === 'video' ? '视频笔记' : '图文笔记'}</span>
                {item.tag && <span className="rounded-full bg-accent-primary/10 px-1.5 py-0.5 text-accent-primary">{item.tag}</span>}
              </div>
              <div className="mt-1 truncate text-sm font-medium text-text-primary" title={item.title}>
                {item.title}
              </div>
              {item.summary && (
                <div className="mt-1 line-clamp-2 text-xs text-text-secondary">
                  {item.summary}
                </div>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );

  const renderUploadedFileCard = (attachment: Extract<NonNullable<Message['attachment']>, { type: 'uploaded-file' }>) => {
    const imageSrc = isUploadedImageAttachment(attachment) ? resolveUploadedAttachmentSource(attachment) : '';
    const actionSource = resolveUploadedAttachmentActionSource(attachment);
    if (imageSrc) {
      return (
        <div className="mt-2">
          <img
            src={imageSrc}
            alt={attachment.name}
            className="h-24 w-24 cursor-zoom-in rounded-2xl border border-border bg-surface-secondary object-cover shadow-sm"
            onClick={() => setPreviewImage({ src: imageSrc, alt: attachment.name })}
            onContextMenu={(event) => handleImageContextMenu(event, imageSrc, actionSource)}
            title={attachment.name}
          />
        </div>
      );
    }

    return (
      <div className="mt-2 w-full max-w-[520px] rounded-xl border border-border bg-surface-primary/90 p-3">
        <div className="flex items-start gap-3">
          <div className="flex h-10 w-10 items-center justify-center rounded-lg border border-border bg-surface-secondary text-text-secondary">
            <FileText className="h-5 w-5" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="text-xs text-text-tertiary">上传文件</div>
            <div className="mt-0.5 truncate text-sm font-medium text-text-primary" title={attachment.name}>
              {attachment.name}
            </div>
            <div className="mt-1 text-[11px] text-text-tertiary flex flex-wrap gap-x-2 gap-y-1">
              {attachment.kind && <span>类型: {attachment.kind}</span>}
              {typeof attachment.size === 'number' && <span>大小: {Math.max(0, Math.round(attachment.size / 1024))} KB</span>}
              {attachment.ext && <span>.{String(attachment.ext).replace(/^\./, '')}</span>}
              {attachment.storageMode === 'staged' && <span>已暂存</span>}
              {attachment.directUploadEligible && <span>可直传</span>}
              {attachment.extractionFormat && <span>Rust 已解析</span>}
              {attachment.extractionTruncated && <span>正文已截断</span>}
            </div>
            {attachment.summary && (
              <div className="mt-1.5 line-clamp-2 text-xs text-text-secondary">
                {attachment.summary}
              </div>
            )}
            {attachment.extractionWarning && (
              <div className="mt-1.5 text-xs text-amber-600 dark:text-amber-300">
                {attachment.extractionWarning}
              </div>
            )}
            {attachment.extractionError && (
              <div className="mt-1.5 text-xs text-red-600 dark:text-red-300">
                文档解析失败：{attachment.extractionError}
              </div>
            )}
          </div>
        </div>
      </div>
    );
  };

  const renderThoughtText = (content: string) => (
    <div className="chat-ai-shell">
      <div className="chat-ai-content">
        <StreamingMarkdown
          content={content}
          isStreaming={msg.isStreaming}
          components={markdownComponents}
          urlTransform={transformMarkdownUrl}
          className="chat-markdown-body text-text-secondary"
        />
      </div>
    </div>
  );

  return (
    <div className={clsx('chat-message-row', isUser ? 'chat-message-row-user' : 'chat-message-row-ai')}>

      {/* Plan Visualization (TodoList) */}
      {!isUser && msg.plan && msg.plan.length > 0 && (
        <TodoList steps={msg.plan} />
      )}

      {showWorkflowOnTop && showTimeline && (
        <ProcessTimeline items={filteredTimeline} isStreaming={!!msg.isStreaming} variant={workflowVariant} />
      )}

      {/* AI 工作流可视化 (兼容旧版：思考、工具、技能) - 仅当 timeline 为空时显示 */}
      {showWorkflowOnTop && showLegacyWorkflow && (
        <div className="mb-4 w-full max-w-3xl space-y-3">
          {/* Thinking Bubble */}
          {msg.thinking && (
            renderThoughtText(stripInternalProtocolMarkup(msg.thinking))
          )}

          {/* Activated Skill */}
          {showWorkflowDetails && msg.activatedSkill && (
            <SkillActivatedBadge
              name={msg.activatedSkill.name}
              description={msg.activatedSkill.description}
            />
          )}

          {/* Tool Calls */}
          {showWorkflowDetails && msg.tools.length > 0 && (
            <div className="rounded-lg border border-border/70 bg-surface-primary/60 px-3 py-2 text-xs text-text-tertiary">
              查看工具调用 ({msg.tools.length})
            </div>
          )}
        </div>
      )}

      {showStreamingThought && (
        <div className={clsx(showWorkflowOnTop ? 'mb-2' : 'mt-2', 'w-full max-w-[740px]')}>
          {renderThoughtText(activeThoughtContent)}
        </div>
      )}

      {!isUser && toolArtifactPreviews.length > 0 && (
        <div className="w-full max-w-[740px]">
          {toolArtifactPreviews.map((artifact, index) => (
            <React.Fragment key={`tool-artifact-${index}-${artifact.source}`}>
              {renderArtifactPreview(artifact.source, artifact.label)}
            </React.Fragment>
          ))}
        </div>
      )}

      {!isUser && mcpApps.length > 0 && (
        <div className="w-full max-w-[900px]">
          {mcpApps.map(({ tool, descriptor }) => (
            <McpAppFrame
              key={`${tool.callId}-${descriptor.resourceUri}`}
              extensionName={descriptor.extensionName}
              resourceUri={descriptor.resourceUri}
              toolName={tool.name}
              toolInput={tool.input}
              toolOutput={tool.output!}
              onSendMessage={onMcpAppMessage}
            />
          ))}
        </div>
      )}

      {/* 消息内容 */}
      {hasRenderableMessageContent && (
        isUser ? (
          /* 用户消息 */
          (() => {
            const videoCardMatch = msg.content.match(/<!--VIDEO_CARD:(.*?)-->/);
            let videoCard: { title: string; thumbnailUrl?: string; videoId?: string } | null = null;
            let displayText = msg.displayContent || msg.content;

            if (videoCardMatch) {
              try {
                videoCard = JSON.parse(videoCardMatch[1]);
                displayText = msg.displayContent || `总结视频「${videoCard?.title}」的内容`;
              } catch (e) {
                console.error('Failed to parse video card:', e);
              }
            }

            const beginForkEdit = () => {
              setForkDraftContent(String(displayText || msg.content || ''));
              setIsForkEditing(true);
            };
            const cancelForkEdit = () => {
              if (isForkSubmitting) return;
              setIsForkEditing(false);
              setForkDraftContent('');
            };
            const submitForkEdit = async () => {
              if (!onForkMessage || isForkSubmitting) return;
              const nextContent = forkDraftContent.trim();
              if (!nextContent) return;
              setIsForkSubmitting(true);
              try {
                const result = await onForkMessage(msg, nextContent);
                if (result !== false) {
                  setIsForkEditing(false);
                  setForkDraftContent('');
                }
              } finally {
                setIsForkSubmitting(false);
              }
            };

            return (
              <div className="group flex w-full flex-col items-end">
                {isForkEditing ? (
                  <div className="w-full max-w-[min(42rem,100%)] rounded-2xl border border-accent-primary/35 bg-surface-primary p-2.5 shadow-lg">
                    {videoCard && (
                      <div className="mb-2">
                        {renderYoutubeCard(videoCard)}
                      </div>
                    )}
                    <textarea
                      ref={forkDraftRef}
                      value={forkDraftContent}
                      onChange={(event) => {
                        setForkDraftContent(event.target.value);
                        window.requestAnimationFrame(syncForkDraftHeight);
                      }}
                      onKeyDown={(event) => {
                        if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
                          event.preventDefault();
                          void submitForkEdit();
                        }
                        if (event.key === 'Escape') {
                          event.preventDefault();
                          cancelForkEdit();
                        }
                      }}
                      className="block min-h-[72px] w-full resize-none rounded-xl border border-border bg-surface-secondary px-3 py-2.5 text-[15px] leading-relaxed text-text-primary outline-none transition focus:border-accent-primary/60 focus:bg-surface-primary"
                    />
                    <div className="mt-2 flex items-center justify-between gap-3">
                      <div className="min-w-0 truncate text-xs text-text-tertiary">
                        发送后从这里创建新分支
                      </div>
                      <div className="flex shrink-0 items-center gap-1.5">
                        <button
                          type="button"
                          onClick={cancelForkEdit}
                          disabled={isForkSubmitting}
                          className="inline-flex h-8 items-center gap-1.5 rounded-md px-2.5 text-xs text-text-secondary transition-colors hover:bg-surface-secondary hover:text-text-primary disabled:opacity-50"
                        >
                          <X className="h-3.5 w-3.5" />
                          <span>取消</span>
                        </button>
                        <button
                          type="button"
                          onClick={() => void submitForkEdit()}
                          disabled={isForkSubmitting || forkDraftContent.trim().length === 0}
                          className="inline-flex h-8 items-center gap-1.5 rounded-md bg-accent-primary px-3 text-xs font-medium text-white transition-colors hover:bg-accent-primary/90 disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          <Send className="h-3.5 w-3.5" />
                          <span>{isForkSubmitting ? '发送中' : '发送'}</span>
                        </button>
                      </div>
                    </div>
                  </div>
                ) : (
                  <div className="chat-user-bubble max-w-full px-4 py-2.5 text-[15px] leading-relaxed text-white shadow-sm">
                    {videoCard && (
                      <div className="mb-3">
                        {renderYoutubeCard(videoCard)}
                      </div>
                    )}
                    <div className="whitespace-pre-wrap">{displayText}</div>
                  </div>
                )}
                <div className="mt-1.5 flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100">
                  <button
                    type="button"
                    onClick={() => onCopyMessage(msg.id, String(displayText || msg.content || ''))}
                    className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-text-tertiary transition-colors hover:bg-surface-secondary hover:text-text-primary"
                    title="复制提示词"
                  >
                    {copiedMessageId === msg.id ? (
                      <>
                        <Check className="h-3.5 w-3.5 text-green-500" />
                        <span className="text-green-500">已复制</span>
                      </>
                    ) : (
                      <>
                        <Copy className="h-3.5 w-3.5" />
                        <span>复制</span>
                      </>
                    )}
                  </button>
                  {onForkMessage && !isForkEditing && (
                    <button
                      type="button"
                      onClick={beginForkEdit}
                      className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-text-tertiary transition-colors hover:bg-surface-secondary hover:text-text-primary"
                      title="编辑这条提示词，发送后从这里继续"
                    >
                      <Edit className="h-3.5 w-3.5" />
                      <span>编辑</span>
                    </button>
                  )}
                </div>
                {showAttachments && msg.attachment?.type === 'youtube-video' && !videoCard && (
                  <div className="mt-2 w-full max-w-[420px]">
                    {renderYoutubeCard(msg.attachment)}
                  </div>
                )}
                {showAttachments && msg.attachment?.type === 'wander-references' && renderWanderReferenceCards(msg.attachment)}
                {showAttachments && uploadedAttachments.map((attachment, index) => (
                  <React.Fragment key={`${attachment.absolutePath || attachment.originalAbsolutePath || attachment.localUrl || attachment.name}-${index}`}>
                    {renderUploadedFileCard(attachment)}
                  </React.Fragment>
                ))}
              </div>
            );
          })()
        ) : (
          /* AI 回复 */
          <div className={clsx('chat-ai-shell group', msg.isStreaming && 'chat-ai-shell-streaming')}>
            {showProcessingTimer && (
              <ProcessingTimerBadge
                startedAt={msg.processingStartedAt as number}
                finishedAt={msg.processingFinishedAt}
                isStreaming={msg.isStreaming}
              />
            )}
            <div ref={aiContentRef} className={clsx('chat-ai-content', msg.isStreaming && 'chat-ai-content-streaming')}>
              <div className={clsx(
                'chat-markdown-body',
                isThinkingMessage ? 'text-text-secondary' : 'text-text-primary',
                showPendingThinkingIndicator && 'chat-markdown-body-pending',
              )}>
                {showPendingThinkingIndicator ? (
                  <ThinkingIndicator label={pendingStatusLabel} />
                ) : (
                  <>
                    {assistantContentParts.map((part, index) => (
                      part.type === 'text' ? (
                        <StreamingMarkdown
                          key={`text-${index}`}
                          content={part.content}
                          isStreaming={msg.isStreaming}
                          components={markdownComponents}
                          urlTransform={transformMarkdownUrl}
                        />
                      ) : (
                        <React.Fragment key={`artifact-${index}-${part.artifact.source}`}>
                          {!part.artifact.hidden && renderArtifactPreview(part.artifact.source, part.artifact.label)}
                        </React.Fragment>
                      )
                    ))}
                  </>
                )}
                {msg.isStreaming && !showPendingThinkingIndicator && (
                  <span className="chat-streaming-caret" />
                )}
              </div>
            </div>
            {/* 复制按钮 */}
            {!msg.isStreaming && sanitizedAssistantContent && (
              <div className="chat-ai-actions opacity-0 transition-opacity group-hover:opacity-100">
                <button
                  onClick={() => onCopyMessage(msg.id, sanitizedAssistantContent)}
                  className="flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-text-tertiary transition-colors hover:bg-surface-secondary hover:text-text-primary"
                  title="复制内容"
                >
                  {copiedMessageId === msg.id ? (
                    <>
                      <Check className="w-3.5 h-3.5 text-green-500" />
                      <span className="text-green-500">已复制</span>
                    </>
                  ) : (
                    <>
                      <Copy className="w-3.5 h-3.5" />
                      <span>复制</span>
                    </>
                  )}
                </button>
              </div>
            )}
          </div>
        )
      )}

      {/* AI 工作流可视化 (底部渲染) */}
      {!showWorkflowOnTop && showTimeline && (
        <ProcessTimeline items={filteredTimeline} isStreaming={!!msg.isStreaming} variant={workflowVariant} />
      )}

      {!showWorkflowOnTop && showLegacyWorkflow && (
        <div className="mt-3 w-full max-w-3xl space-y-3">
          {msg.thinking && (
            renderThoughtText(stripInternalProtocolMarkup(msg.thinking))
          )}
          {showWorkflowDetails && msg.activatedSkill && (
            <SkillActivatedBadge
              name={msg.activatedSkill.name}
              description={msg.activatedSkill.description}
            />
          )}
          {showWorkflowDetails && msg.tools.length > 0 && (
            <div className="rounded-lg border border-border/70 bg-surface-primary/60 px-3 py-2 text-xs text-text-tertiary">
              查看工具调用 ({msg.tools.length})
            </div>
          )}
        </div>
      )}

      {imageMenu.visible && (
        <LiquidGlassMenuPanel
          className="fixed z-[9999] min-w-[170px]"
          style={{ left: imageMenu.x, top: imageMenu.y }}
          onClick={(event) => event.stopPropagation()}
        >
          <button
            type="button"
            className={getLiquidGlassMenuItemClassName()}
            onClick={() => void handleCopyImage()}
          >
            复制图片
          </button>
          {menuSupportsReveal && (
            <>
              <LiquidGlassMenuSeparator />
              <button
                type="button"
                className={getLiquidGlassMenuItemClassName()}
                onClick={() => void handleShowInFolder()}
              >
                在文件夹中打开
              </button>
            </>
          )}
        </LiquidGlassMenuPanel>
      )}

      {previewImage && (
        <div
          className="fixed inset-0 z-[9998] flex items-center justify-center bg-black/70 p-6"
          onClick={() => setPreviewImage(null)}
        >
          <img
            src={previewImage.src}
            alt={previewImage.alt}
            className="max-h-[90vh] max-w-[90vw] rounded-xl border border-white/15 bg-black/10 object-contain shadow-2xl"
            onClick={(event) => event.stopPropagation()}
            onContextMenu={(event) => handleImageContextMenu(event, previewImage.src)}
          />
        </div>
      )}
    </div>
  );
}, (prevProps, nextProps) => {
  // 自定义比对函数：只有内容、状态、思考过程真正变化时才渲染
  // 忽略父组件其他无关 State 变化导致的重绘
  const msgChanged = 
    prevProps.msg.content !== nextProps.msg.content ||
    prevProps.msg.messageType !== nextProps.msg.messageType ||
    prevProps.msg.isStreaming !== nextProps.msg.isStreaming ||
    prevProps.msg.processingStartedAt !== nextProps.msg.processingStartedAt ||
    prevProps.msg.processingFinishedAt !== nextProps.msg.processingFinishedAt ||
    prevProps.msg.suppressPendingIndicator !== nextProps.msg.suppressPendingIndicator ||
    prevProps.msg.thinking !== nextProps.msg.thinking ||
    prevProps.msg.tools !== nextProps.msg.tools ||
    prevProps.msg.plan !== nextProps.msg.plan || // Check plan changes
    prevProps.msg.activatedSkill !== nextProps.msg.activatedSkill ||
    timelineRenderSignature(prevProps.msg.timeline) !== timelineRenderSignature(nextProps.msg.timeline);

  const copyStatusChanged = 
    (prevProps.copiedMessageId === prevProps.msg.id) !== (nextProps.copiedMessageId === nextProps.msg.id);
  const workflowStyleChanged =
    prevProps.workflowPlacement !== nextProps.workflowPlacement ||
    prevProps.workflowVariant !== nextProps.workflowVariant ||
    prevProps.workflowEmphasis !== nextProps.workflowEmphasis ||
    prevProps.workflowDisplayMode !== nextProps.workflowDisplayMode ||
    prevProps.showAttachments !== nextProps.showAttachments ||
    prevProps.onForkMessage !== nextProps.onForkMessage;

  return !msgChanged && !copyStatusChanged && !workflowStyleChanged;
});
