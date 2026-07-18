import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import {
  MessageItem,
  extractRenderableArtifactsFromText,
  extractRenderableArtifactsFromUnknown,
} from './MessageItem';

afterEach(() => {
  vi.unstubAllGlobals();
});

const renderToolArtifactMessage = (output: Record<string, unknown>) => render(React.createElement(MessageItem, {
  msg: {
    id: 'tool-artifact-message',
    role: 'ai' as const,
    content: '',
    tools: [{
      id: 'tool-event',
      callId: 'tool-call',
      name: 'openmontage__drama_stage_decide',
      input: {},
      output,
      status: 'done' as const,
    }],
    timeline: [],
    isStreaming: false,
  },
  copiedMessageId: null,
  onCopyMessage: vi.fn(),
}));

describe('MessageItem artifact extraction', () => {
  it('recognizes generated relative image and video paths in assistant text', () => {
    const artifacts = extractRenderableArtifactsFromText([
      '参考图：generated/demo/images/frame_001.png',
      '成片：generated/demo/output/final.mp4',
    ].join('\n'));

    expect(artifacts.map((artifact) => artifact.kind)).toEqual(['image', 'video']);
    expect(artifacts.map((artifact) => artifact.source)).toEqual([
      'generated/demo/images/frame_001.png',
      'generated/demo/output/final.mp4',
    ]);
  });

  it('keeps media paths nested inside structured tool output', () => {
    const artifacts = extractRenderableArtifactsFromUnknown({
      success: true,
      structuredContent: {
        mediaAssets: [
          {
            name: '最终成片',
            absolutePath: '/Users/test/.redconvert/spaces/default/media/generated/demo/final.mp4',
            relativePath: 'generated/demo/final.mp4',
          },
        ],
      },
    });

    expect(artifacts).toHaveLength(1);
    expect(artifacts[0].kind).toBe('video');
    expect(artifacts[0].source).toContain('generated/demo/final.mp4');
  });

  it('recognizes Windows drive and UNC artifact paths in assistant text', () => {
    const drivePath = String.raw`C:\Users\tester\Desktop\drama-preview.html`;
    const uncPath = String.raw`\\fileserver\team-share\weekly-report.pdf`;
    const artifacts = extractRenderableArtifactsFromText([
      `预览：${drivePath}`,
      `报告：${uncPath}`,
    ].join('\n'));

    expect(artifacts.map((artifact) => artifact.source)).toEqual([drivePath, uncPath]);
    expect(artifacts.map((artifact) => artifact.kind)).toEqual(['html', 'pdf']);
  });

  it('does not iframe an unregistered AI Drama relative HTML artifact', async () => {
    const invoke = vi.fn().mockResolvedValue({ success: true, assets: [] });
    vi.stubGlobal('ipcRenderer', { invoke });
    const { container } = renderToolArtifactMessage({
      success: true,
      structuredContent: {
        htmlPath: 'generated/ai-drama/intermediate/index.html',
      },
    });

    await waitFor(() => {
      expect(screen.getByTestId('artifact-unresolved')).toBeInTheDocument();
    });
    expect(invoke).toHaveBeenCalledWith('media:list', { limit: 1000 });
    expect(container.querySelector('iframe')).toBeNull();
  });

  it('does not iframe an unregistered Windows absolute HTML artifact', async () => {
    const invoke = vi.fn().mockResolvedValue({ success: true, assets: [] });
    vi.stubGlobal('ipcRenderer', { invoke });
    const { container } = renderToolArtifactMessage({
      success: true,
      structuredContent: {
        htmlPath: String.raw`C:\Users\tester\.redconvert\spaces\default\media\generated\drama\missing.html`,
      },
    });

    await waitFor(() => {
      expect(screen.getByTestId('artifact-unresolved')).toBeInTheDocument();
    });
    expect(container.querySelector('iframe')).toBeNull();
  });

  it('sandboxes a registered local HTML artifact without same-origin access', async () => {
    const htmlPath = String.raw`C:\Users\tester\.redconvert\spaces\default\media\generated\drama\preview.html`;
    const invoke = vi.fn().mockResolvedValue({
      success: true,
      assets: [{ id: 'drama-preview', absolutePath: htmlPath, exists: true }],
    });
    vi.stubGlobal('ipcRenderer', { invoke });
    const { container } = renderToolArtifactMessage({
      success: true,
      structuredContent: {
        htmlPath,
      },
    });

    await waitFor(() => {
      expect(container.querySelector('iframe')).not.toBeNull();
    });
    const frame = container.querySelector('iframe');
    expect(frame).toHaveAttribute('sandbox', 'allow-scripts');
    expect(frame?.getAttribute('sandbox')).not.toContain('allow-same-origin');
  });

  it('does not iframe a stale registered HTML artifact whose file is missing', async () => {
    const htmlPath = String.raw`C:\Users\tester\.redconvert\spaces\default\media\generated\drama\stale.html`;
    const invoke = vi.fn().mockResolvedValue({
      success: true,
      assets: [{ id: 'stale-preview', absolutePath: htmlPath, exists: false }],
    });
    vi.stubGlobal('ipcRenderer', { invoke });
    const { container } = renderToolArtifactMessage({
      success: true,
      structuredContent: { htmlPath },
    });

    await waitFor(() => {
      expect(screen.getByTestId('artifact-unresolved')).toBeInTheDocument();
    });
    expect(container.querySelector('iframe')).toBeNull();
  });

  it('renders inline parameters as inline code and only fenced groups get a copy button', () => {
    const renderMessage = (content: string) => renderToStaticMarkup(React.createElement(MessageItem, {
      msg: {
        id: 'render-test',
        role: 'ai' as const,
        content,
        tools: [],
        timeline: [],
        isStreaming: false,
      },
      copiedMessageId: null,
      onCopyMessage: vi.fn(),
    }));

    const inlineMarkup = renderMessage('比例：`9:16`；可选：`16:9`、`1:1`、`3:4`。');
    expect(inlineMarkup).toContain('>9:16</code>');
    expect(inlineMarkup).not.toContain('title="复制"');

    const groupedMarkup = renderMessage('生成参数：\n\n```text\n比例：9:16\n时长：30 秒\n```');
    expect(groupedMarkup.match(/title="复制"/g)).toHaveLength(1);
  });
});
