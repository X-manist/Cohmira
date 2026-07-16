import React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';

import {
  MessageItem,
  extractRenderableArtifactsFromText,
  extractRenderableArtifactsFromUnknown,
} from './MessageItem';

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
