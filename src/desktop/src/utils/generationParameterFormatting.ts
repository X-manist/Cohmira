const GENERATION_PARAMETER_HEADING_RE = /^(?:我使用的生成参数(?:是)?|已确认并使用的参数|生成参数)\s*[:：]\s*$/;
const GENERATION_PARAMETER_LINE_RE = /^\s*(?:[-*]\s*)?([^：:\n]{1,24})[：:]\s*(.*?)\s*$/;
const GENERATION_PARAMETER_LABEL_RE = /^(?:用途|平台|比例|数量|风格|参考图|prompt|提示词|模型|尺寸|画幅|分辨率|时长|模式|质量)$/i;

export const normalizeGenerationParameterBlocks = (value: string): string => {
  const lines = String(value || '').split('\n');
  const output: string[] = [];

  for (let index = 0; index < lines.length; index += 1) {
    const heading = lines[index];
    if (!GENERATION_PARAMETER_HEADING_RE.test(heading.trim())) {
      output.push(heading);
      continue;
    }

    let cursor = index + 1;
    while (cursor < lines.length && !lines[cursor].trim()) cursor += 1;
    if (lines[cursor]?.trim().startsWith('```')) {
      output.push(heading);
      continue;
    }

    const parameters: string[] = [];
    while (cursor < lines.length) {
      const line = lines[cursor];
      if (!line.trim()) break;
      const match = line.match(GENERATION_PARAMETER_LINE_RE);
      if (!match || !GENERATION_PARAMETER_LABEL_RE.test(match[1].trim())) break;
      const label = match[1].trim();
      const parameterValue = match[2].replace(/`+/g, '').trim();
      parameters.push(`${label}：${parameterValue}`);
      cursor += 1;
    }

    if (parameters.length < 2) {
      output.push(heading);
      continue;
    }

    output.push(heading, '', '```text', ...parameters, '```');
    index = cursor - 1;
  }

  return output.join('\n').replace(/\n{3,}/g, '\n\n').trim();
};
