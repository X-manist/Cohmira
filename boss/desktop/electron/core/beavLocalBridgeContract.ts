export const BEAV_BRIDGE_SUPPORTED_VIEWS = [
  'chat',
  'team',
  'skills',
  'knowledge',
  'settings',
  'manuscripts',
  'archives',
  'wander',
  'redclaw',
  'media-library',
  'cover-studio',
  'generation-studio',
  'subjects',
  'workboard',
] as const;

export const BEAV_BRIDGE_SUPPORTED_ACTIONS = [
  'app_cli',
  'navigate',
  'tool_call',
] as const;

export type BeavBridgeAction = typeof BEAV_BRIDGE_SUPPORTED_ACTIONS[number];

const SUPPORTED_VIEW_SET = new Set<string>(BEAV_BRIDGE_SUPPORTED_VIEWS);
const SUPPORTED_ACTION_SET = new Set<string>(BEAV_BRIDGE_SUPPORTED_ACTIONS);

export function normalizeBridgePayload(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

export function isSupportedBeavBridgeView(view: unknown): boolean {
  return SUPPORTED_VIEW_SET.has(String(view || '').trim());
}

export function requireBeavBridgeView(view: unknown): string {
  const normalized = String(view || '').trim();
  if (!isSupportedBeavBridgeView(normalized)) {
    throw new Error(`Unsupported Beav view: ${normalized}`);
  }
  return normalized;
}

export function isSupportedBeavBridgeAction(action: unknown): action is BeavBridgeAction {
  return SUPPORTED_ACTION_SET.has(String(action || '').trim());
}

export function requireBeavBridgeAction(action: unknown): BeavBridgeAction {
  const normalized = String(action || '').trim();
  if (!isSupportedBeavBridgeAction(normalized)) {
    throw new Error(`Unsupported Beav bridge action: ${normalized || '(empty)'}`);
  }
  return normalized;
}
