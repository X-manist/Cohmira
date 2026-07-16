import { REDBOX_NAVIGATE_EVENT, type NotificationAction } from './types';

function normalizeNavigationPayload(payload: { view?: string } | undefined): { view?: string } | undefined {
  if (!payload?.view) return payload;
  if (payload.view === 'chat' || payload.view === 'generation-studio') {
    return { ...payload, view: 'redclaw' };
  }
  return payload;
}

export async function runNotificationAction(action: NotificationAction): Promise<void> {
  if (action.action === 'navigate') {
    window.dispatchEvent(new CustomEvent(REDBOX_NAVIGATE_EVENT, { detail: normalizeNavigationPayload(action.payload) }));
    return;
  }

  if (action.action === 'open-path') {
    await window.ipcRenderer.openPath(action.payload.path);
    return;
  }

  if (action.action === 'retry-generation') {
    await window.ipcRenderer.generation.retryJob(action.payload.jobId);
    window.dispatchEvent(
      new CustomEvent(REDBOX_NAVIGATE_EVENT, { detail: { view: 'redclaw' } }),
    );
  }
}
