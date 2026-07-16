import { describe, expect, it } from 'vitest';

import { normalizeGenerationParameterBlocks } from './generationParameterFormatting';

describe('normalizeGenerationParameterBlocks', () => {
  it('groups confirmed generation parameters into one copyable text block', () => {
    expect(normalizeGenerationParameterBlocks([
      '已确认并使用的参数：',
      '',
      '- 比例：`9:16`',
      '- 数量：`3`',
      '- 时长：`30 秒`',
      '',
      '现在开始生成。',
    ].join('\n'))).toBe([
      '已确认并使用的参数：',
      '',
      '```text',
      '比例：9:16',
      '数量：3',
      '时长：30 秒',
      '```',
      '',
      '现在开始生成。',
    ].join('\n'));
  });

  it('does not split or rewrite an existing fenced parameter block', () => {
    const source = [
      '生成参数：',
      '',
      '```text',
      '比例：16:9',
      '数量：1',
      '```',
    ].join('\n');
    expect(normalizeGenerationParameterBlocks(source)).toBe(source);
  });
});
