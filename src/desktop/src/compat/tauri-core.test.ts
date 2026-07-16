import { afterEach, describe, expect, it, vi } from 'vitest';

import { invoke } from './tauri-core';

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('tauri core compatibility invoke', () => {
  it('sends native file picker commands directly to Tauri', async () => {
    const nativeInvoke = vi.fn(async () => ['/tmp/reference.png']);
    const transportInvoke = vi.fn(async () => ({ success: false }));
    vi.stubGlobal('window', {
      __TAURI_INTERNALS__: { invoke: nativeInvoke },
      __RED_ELECTRON_IPC__: {
        invoke: transportInvoke,
        send: vi.fn(),
        on: vi.fn(),
        off: vi.fn(),
        removeAllListeners: vi.fn(),
      },
    });

    await expect(invoke('pick_files', { title: '选择文件' })).resolves.toEqual(['/tmp/reference.png']);
    expect(nativeInvoke).toHaveBeenCalledWith('pick_files', { title: '选择文件' });
    expect(transportInvoke).not.toHaveBeenCalled();
  });

  it('keeps business commands on the compatibility IPC transport', async () => {
    const nativeInvoke = vi.fn();
    const transportInvoke = vi.fn(async () => [{ id: 'default' }]);
    vi.stubGlobal('window', {
      __TAURI_INTERNALS__: { invoke: nativeInvoke },
      __RED_ELECTRON_IPC__: {
        invoke: transportInvoke,
        send: vi.fn(),
        on: vi.fn(),
        off: vi.fn(),
        removeAllListeners: vi.fn(),
      },
    });

    await invoke('spaces_list');
    expect(transportInvoke).toHaveBeenCalledWith('spaces:list', undefined);
    expect(nativeInvoke).not.toHaveBeenCalled();
  });
});
