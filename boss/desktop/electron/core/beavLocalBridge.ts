import { BrowserWindow } from 'electron';
import { AppCliTool } from './tools/appCliTool';
import { createBuiltinTools } from './tools';
import type { ToolResult } from './toolRegistry';
import {
  normalizeBridgePayload,
  requireBeavBridgeAction,
  requireBeavBridgeView,
} from './beavLocalBridgeContract';

export interface BeavBridgeRequest {
  action?: string;
  payload?: Record<string, unknown>;
  source?: string;
}

export interface BeavBridgeResponse {
  result?: unknown;
  error?: {
    code: string;
    message: string;
    details?: unknown;
  };
}

function normalizePayload(value: unknown): Record<string, unknown> {
  return normalizeBridgePayload(value);
}

function normalizeToolResult(result: ToolResult): unknown {
  return {
    success: result.success,
    display: result.display,
    llmContent: result.llmContent,
    data: result.data,
    error: result.error,
  };
}

function firstWindow(): BrowserWindow | null {
  const windows = BrowserWindow.getAllWindows();
  return windows.length > 0 ? windows[0] : null;
}

function focusWindow(target: BrowserWindow): void {
  if (target.isMinimized()) {
    target.restore();
  }
  target.show();
  target.focus();
}

async function handleAppCli(payload: Record<string, unknown>): Promise<unknown> {
  const command = String(payload.command || '').trim();
  if (!command) {
    throw new Error('app_cli command is required');
  }
  const tool = new AppCliTool();
  const params = {
    command,
    payload: normalizePayload(payload.payload),
  };
  const validationError = tool.validate(params);
  if (validationError) {
    throw new Error(validationError);
  }
  const result = await tool.execute(params);
  return normalizeToolResult(result);
}

async function handleNavigate(payload: Record<string, unknown>): Promise<unknown> {
  const view = requireBeavBridgeView(payload.view);
  const target = firstWindow();
  if (!target) {
    throw new Error('No Beav desktop window is available');
  }
  if (payload.focusWindow !== false) {
    focusWindow(target);
  }
  target.webContents.send('app:navigate', {
    view,
    params: normalizePayload(payload.params),
    source: 'beav-local-bridge',
  });
  return { success: true, view };
}

async function handleToolCall(payload: Record<string, unknown>): Promise<unknown> {
  const name = String(payload.name || '').trim();
  if (!name) {
    throw new Error('tool_call name is required');
  }
  const args = normalizePayload(payload.arguments);
  const tool = createBuiltinTools({ pack: 'full' }).find((item) => item.name === name);
  if (!tool) {
    throw new Error(`Unsupported Beav tool: ${name}`);
  }
  const validationError = tool.validate(args);
  if (validationError) {
    throw new Error(validationError);
  }
  const result = await tool.execute(args, new AbortController().signal);
  return normalizeToolResult(result);
}

export async function handleBeavLocalBridgeRequest(input: BeavBridgeRequest): Promise<BeavBridgeResponse> {
  try {
    const action = requireBeavBridgeAction(input?.action);
    const payload = normalizePayload(input?.payload);
    if (action === 'app_cli') {
      return { result: await handleAppCli(payload) };
    }
    if (action === 'navigate') {
      return { result: await handleNavigate(payload) };
    }
    if (action === 'tool_call') {
      return { result: await handleToolCall(payload) };
    }
    throw new Error(`Unsupported Beav bridge action: ${action || '(empty)'}`);
  } catch (error) {
    return {
      error: {
        code: 'BEAV_BRIDGE_ACTION_FAILED',
        message: error instanceof Error ? error.message : String(error),
      },
    };
  }
}
