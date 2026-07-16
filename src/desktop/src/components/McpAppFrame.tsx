import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';

export interface McpAppToolOutput {
  success?: boolean;
  content?: string;
  blocks?: unknown[];
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
  _meta?: Record<string, unknown>;
  [key: string]: unknown;
}

type McpAppDescriptor = {
  extensionName: string;
  resourceUri: string;
};

type McpAppFrameProps = McpAppDescriptor & {
  toolName: string;
  toolInput: unknown;
  toolOutput: McpAppToolOutput;
  onSendMessage?: (text: string) => Promise<unknown> | unknown;
};

const APP_PROTOCOL_VERSION = '2026-01-26';
const ALLOWED_APP_TOOLS = new Set([
  'drama_selection_commit',
  'drama_stage_decide',
  'drama_ui_refresh',
]);

const toRecord = (value: unknown): Record<string, unknown> => (
  value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
);

const nestedString = (value: unknown, path: string[]): string => {
  let current: unknown = value;
  for (const segment of path) {
    current = toRecord(current)[segment];
  }
  return typeof current === 'string' ? current : '';
};

export function getMcpAppDescriptor(
  toolName: string,
  output?: McpAppToolOutput,
): McpAppDescriptor | null {
  const meta = toRecord(output?._meta);
  const resourceUri = nestedString(meta, ['ui', 'resourceUri'])
    || nestedString(meta, ['goose', 'mcpApp', 'resourceUri'])
    || nestedString(meta, ['__goose_tool_update_meta', 'mcpApp', 'resourceUri']);
  if (!resourceUri.startsWith('ui://')) return null;
  const delimiter = toolName.lastIndexOf('__');
  const extensionName = delimiter > 0 ? toolName.slice(0, delimiter) : 'openmontage';
  return { extensionName, resourceUri };
}

const resourceHtmlFromOutput = (output: McpAppToolOutput): string => {
  const meta = toRecord(output._meta);
  const candidates = [
    toRecord(toRecord(toRecord(meta.goose).mcpApp).resourceResult),
    toRecord(toRecord(toRecord(meta.__goose_tool_update_meta).mcpApp).resourceResult),
  ];
  for (const candidate of candidates) {
    const contents = Array.isArray(candidate.contents) ? candidate.contents : [];
    for (const content of contents) {
      const text = toRecord(content).text;
      if (typeof text === 'string' && text.trim()) return text;
    }
  }
  return '';
};

const currentTheme = (): 'light' | 'dark' => {
  const root = document.documentElement;
  if (root.classList.contains('dark')) return 'dark';
  if (root.classList.contains('light')) return 'light';
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
};

export function McpAppFrame({
  extensionName,
  resourceUri,
  toolName,
  toolInput,
  toolOutput,
  onSendMessage,
}: McpAppFrameProps) {
  const iframeRef = useRef<HTMLIFrameElement | null>(null);
  const [html, setHtml] = useState(() => resourceHtmlFromOutput(toolOutput));
  const [error, setError] = useState('');
  const [height, setHeight] = useState(420);
  const [initialized, setInitialized] = useState(false);
  const requestSource = useMemo(
    () => ({ extensionName, uri: resourceUri }),
    [extensionName, resourceUri],
  );

  const post = useCallback((message: Record<string, unknown>) => {
    iframeRef.current?.contentWindow?.postMessage(message, '*');
  }, []);

  const sendToolResult = useCallback(() => {
    post({
      jsonrpc: '2.0',
      method: 'ui/notifications/tool-result',
      params: {
        content: Array.isArray(toolOutput.blocks)
          ? toolOutput.blocks
          : [{ type: 'text', text: String(toolOutput.content || '') }],
        structuredContent: toolOutput.structuredContent,
        isError: toolOutput.isError ?? toolOutput.success === false,
        _meta: toolOutput._meta,
      },
    });
  }, [post, toolOutput]);

  useEffect(() => {
    let cancelled = false;
    const embedded = resourceHtmlFromOutput(toolOutput);
    if (embedded) {
      setHtml(embedded);
      setError('');
      return () => { cancelled = true; };
    }
    void window.ipcRenderer.invoke('goose:mcp-read-resource', requestSource)
      .then((result) => {
        if (cancelled) return;
        const record = toRecord(result);
        const contents = Array.isArray(record.contents) ? record.contents : [];
        const resource = contents.map(toRecord).find((item) => typeof item.text === 'string');
        const nextHtml = typeof resource?.text === 'string' ? resource.text : '';
        if (!nextHtml) throw new Error('插件没有返回 MCP App HTML');
        setHtml(nextHtml);
        setError('');
      })
      .catch((reason) => {
        if (cancelled) return;
        setError(reason instanceof Error ? reason.message : String(reason));
      });
    return () => { cancelled = true; };
  }, [requestSource, toolOutput]);

  useEffect(() => {
    if (!initialized) return;
    post({
      jsonrpc: '2.0',
      method: 'ui/notifications/tool-input',
      params: { arguments: toRecord(toolInput) },
    });
    sendToolResult();
  }, [initialized, post, sendToolResult, toolInput]);

  useEffect(() => {
    const observer = new MutationObserver(() => {
      post({
        jsonrpc: '2.0',
        method: 'ui/notifications/host-context-changed',
        params: { theme: currentTheme(), displayMode: 'inline' },
      });
    });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['class', 'style'] });
    return () => observer.disconnect();
  }, [post]);

  useEffect(() => {
    const handleMessage = (event: MessageEvent) => {
      if (event.source !== iframeRef.current?.contentWindow) return;
      const message = toRecord(event.data);
      if (message.jsonrpc !== '2.0') return;
      const method = typeof message.method === 'string' ? message.method : '';
      const id = message.id;
      const params = toRecord(message.params);

      const respond = (result?: unknown, responseError?: Error) => {
        if (id === undefined || id === null) return;
        post(responseError
          ? { jsonrpc: '2.0', id, error: { code: -32000, message: responseError.message } }
          : { jsonrpc: '2.0', id, result: result ?? {} });
      };

      if (method === 'ui/initialize') {
        respond({
          protocolVersion: APP_PROTOCOL_VERSION,
          hostContext: {
            theme: currentTheme(),
            displayMode: 'inline',
            locale: navigator.language || 'zh-CN',
            containerDimensions: {
              width: iframeRef.current?.clientWidth || undefined,
            },
          },
          hostCapabilities: {
            availableDisplayModes: ['inline'],
          },
        });
        return;
      }

      if (method === 'ui/notifications/initialized') {
        setInitialized(true);
        return;
      }

      if (method === 'ui/notifications/size-changed') {
        const requested = Number(params.height || 0);
        if (Number.isFinite(requested) && requested > 0) {
          setHeight(Math.max(220, Math.min(Math.ceil(requested), 1000)));
        }
        return;
      }

      if (method === 'tools/call') {
        const requestedName = String(params.name || '').trim();
        const localName = requestedName.split('__').pop() || requestedName;
        if (!ALLOWED_APP_TOOLS.has(localName)) {
          respond(undefined, new Error(`MCP App 无权调用工具：${localName}`));
          return;
        }
        void window.ipcRenderer.invoke('goose:mcp-call-tool', {
          extensionName,
          name: localName,
          arguments: toRecord(params.arguments),
        }).then((result) => respond(result)).catch((reason) => {
          respond(undefined, reason instanceof Error ? reason : new Error(String(reason)));
        });
        return;
      }

      if (method === 'ui/message') {
        const content = Array.isArray(params.content) ? params.content : [];
        const text = content
          .map(toRecord)
          .filter((item) => item.type === 'text')
          .map((item) => String(item.text || ''))
          .join('\n')
          .trim();
        if (!text || !onSendMessage) {
          respond(undefined, new Error('当前聊天无法接收 MCP App 消息'));
          return;
        }
        Promise.resolve(onSendMessage(text))
          .then((result) => respond(result))
          .catch((reason) => respond(undefined, reason instanceof Error ? reason : new Error(String(reason))));
        return;
      }

      if (method === 'ui/resource-teardown') {
        respond({});
        return;
      }

      if (id !== undefined && id !== null) {
        respond(undefined, new Error(`未支持的 MCP App 方法：${method || '(empty)'}`));
      }
    };

    window.addEventListener('message', handleMessage);
    return () => window.removeEventListener('message', handleMessage);
  }, [extensionName, onSendMessage, post]);

  if (error) {
    return (
      <div className="my-3 w-full max-w-[900px] rounded-xl border border-red-500/35 bg-red-500/10 px-4 py-3 text-sm text-red-700 dark:text-red-300">
        <div className="font-medium">AI 短剧工作台加载失败</div>
        <div className="mt-1 text-xs opacity-85">{error}</div>
      </div>
    );
  }

  if (!html) {
    return (
      <div className="my-3 w-full max-w-[900px] rounded-xl border border-border bg-surface-secondary/50 px-4 py-8 text-center text-sm text-text-tertiary">
        正在加载 AI 短剧工作台…
      </div>
    );
  }

  return (
    <div className="my-3 w-full max-w-[900px] overflow-hidden rounded-2xl border border-border bg-surface-primary shadow-sm">
      <div className="flex items-center justify-between border-b border-border/70 px-3 py-2 text-xs text-text-tertiary">
        <span>OpenMontage · AI 短剧工作台</span>
        <span>{toolName.split('__').pop()}</span>
      </div>
      <iframe
        ref={iframeRef}
        title="OpenMontage AI 短剧工作台"
        srcDoc={html}
        sandbox="allow-scripts"
        referrerPolicy="no-referrer"
        className="block w-full border-0 bg-transparent"
        style={{ height }}
        onLoad={() => {
          setInitialized(false);
        }}
      />
    </div>
  );
}

export default McpAppFrame;
