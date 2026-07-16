type RawListener = (...args: unknown[]) => void;

type ElectronIpcTransport = {
  on: (channel: string, listener: RawListener) => void;
  off: (channel: string, listener: RawListener) => void;
  removeAllListeners: (channel: string) => void;
  send: (channel: string, payload?: unknown) => void;
  invoke: <T = unknown>(channel: string, payload?: unknown) => Promise<T>;
};

type TauriInternals = {
  invoke?: <T = unknown>(command: string, args?: unknown, options?: unknown) => Promise<T>;
  transformCallback?: <T = unknown>(callback?: (response: T) => void, once?: boolean) => number;
  unregisterCallback?: (callbackId: number) => void;
  convertFileSrc?: (path: string, protocol?: string) => string;
};

declare global {
  interface Window {
    __RED_ELECTRON_IPC__?: ElectronIpcTransport;
    __TAURI_INTERNALS__?: TauriInternals;
  }
}

type ListenerCleanup = () => void;

const tauriListenerCleanups = new Map<string, Map<RawListener, Promise<ListenerCleanup>>>();

function getTauriInternals(): TauriInternals | null {
  const internals = window.__TAURI_INTERNALS__;
  return internals && typeof internals.invoke === 'function' ? internals : null;
}

function getTauriTransport(): ElectronIpcTransport | null {
  const internals = getTauriInternals();
  if (!internals) return null;

  const invokeRaw = async <T = unknown>(command: string, args?: unknown): Promise<T> => {
    if (!internals.invoke) {
      throw new Error('Tauri invoke is unavailable.');
    }
    return internals.invoke<T>(command, args);
  };

  return {
    invoke<T = unknown>(channel: string, payload?: unknown): Promise<T> {
      return invokeRaw<T>('ipc_invoke', { channel, payload: payload ?? null });
    },

    send(channel: string, payload?: unknown): void {
      void invokeRaw('ipc_send', { channel, payload: payload ?? null });
    },

    on(channel: string, listener: RawListener): void {
      if (!internals.transformCallback) {
        throw new Error('Tauri event callback registration is unavailable.');
      }
      const callbacks = tauriListenerCleanups.get(channel) ?? new Map<RawListener, Promise<ListenerCleanup>>();
      tauriListenerCleanups.set(channel, callbacks);

      const handler = internals.transformCallback<{ payload?: unknown }>((event) => {
        listener({ channel, tauri: true }, event?.payload);
      });
      const cleanup = invokeRaw<number>('plugin:event|listen', {
        event: channel,
        target: { kind: 'Any' },
        handler,
      }).then((eventId) => {
        return () => {
          window.__TAURI_EVENT_PLUGIN_INTERNALS__?.unregisterListener?.(channel, eventId);
          void invokeRaw('plugin:event|unlisten', { event: channel, eventId });
        };
      });

      callbacks.set(listener, cleanup);
      cleanup.catch(() => {
        callbacks.delete(listener);
        internals.unregisterCallback?.(handler);
      });
    },

    off(channel: string, listener: RawListener): void {
      const callbacks = tauriListenerCleanups.get(channel);
      const cleanup = callbacks?.get(listener);
      if (!cleanup) return;
      callbacks?.delete(listener);
      void cleanup.then((dispose) => dispose());
    },

    removeAllListeners(channel: string): void {
      const callbacks = tauriListenerCleanups.get(channel);
      if (!callbacks) return;
      for (const cleanup of callbacks.values()) {
        void cleanup.then((dispose) => dispose());
      }
      callbacks.clear();
    },
  };
}

export function getElectronIpcTransport(): ElectronIpcTransport {
  const transport = window.__RED_ELECTRON_IPC__;
  if (transport) {
    return transport;
  }
  const tauriTransport = getTauriTransport();
  if (tauriTransport) {
    return tauriTransport;
  }
  throw new Error('IPC transport is unavailable.');
}

export function convertFileSrcWithTauri(path: string, protocol = 'asset'): string | null {
  return getTauriInternals()?.convertFileSrc?.(path, protocol) ?? null;
}

export type { ElectronIpcTransport, RawListener, TauriInternals };
