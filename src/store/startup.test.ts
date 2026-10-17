import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createStore } from 'zustand/vanilla';
import type { AppState } from '../store';
import { createStartupSlice } from './startup';

const mocks = vi.hoisted(() => ({
  setup: vi.fn(),
  health: vi.fn(),
  notice: vi.fn(),
  guide: vi.fn(),
}));

vi.mock('../lib/tauri', () => ({
  setupListeners: mocks.setup,
  api: { misc: { storageHealth: mocks.health, startupNoticeTake: mocks.notice, traeworkMigrationGuide: mocks.guide } },
}));

function makeStore(refreshEnv = vi.fn().mockResolvedValue(undefined)) {
  const noop = vi.fn().mockResolvedValue(undefined);
  return createStore<AppState>((set, get, store) => ({
    ...createStartupSlice(set, get, store),
    proxy: { running: false }, settings: null,
    refreshEnv, refreshCert: noop, refreshProxy: noop, refreshApiStatus: noop,
    refreshAccounts: noop, refreshGroups: noop, refreshSettings: noop,
    refreshCreditsHistory: noop, refreshCreditsDaily: noop, refreshProfiles: noop,
    refreshLocalEntitlement: noop,
  } as unknown as AppState));
}

beforeEach(() => {
  vi.resetAllMocks();
  mocks.setup.mockResolvedValue([]);
  mocks.health.mockResolvedValue({ mode: 'persistent', message: null });
  mocks.notice.mockResolvedValue(null);
  mocks.guide.mockResolvedValue(null);
});

describe('startup lifecycle', () => {
  it('shares one initialization between concurrent callers and skips completed startup', async () => {
    const store = makeStore();
    const first = store.getState().init();
    expect(store.getState().init()).toBe(first);
    await first;
    await store.getState().init();
    expect(mocks.setup).toHaveBeenCalledTimes(1);
    expect(store.getState().ready).toBe(true);
  });

  it('reports listener failure and permits a successful retry', async () => {
    mocks.setup.mockRejectedValueOnce(new Error('event bridge unavailable'));
    const store = makeStore();
    await expect(store.getState().init()).rejects.toThrow('event bridge unavailable');
    expect(store.getState().ready).toBe(false);
    expect(store.getState().initError).toContain('event bridge unavailable');
    await store.getState().init();
    expect(store.getState().ready).toBe(true);
    expect(store.getState().initError).toBeNull();
  });

  it('cleans up listeners when data loading fails before retrying', async () => {
    const unlisten = vi.fn();
    mocks.setup.mockResolvedValue([unlisten]);
    const refresh = vi.fn().mockRejectedValueOnce(new Error('load failed')).mockResolvedValue(undefined);
    const store = makeStore(refresh);
    await expect(store.getState().init()).rejects.toThrow('load failed');
    expect(unlisten).toHaveBeenCalledTimes(1);
    await store.getState().init();
    expect(mocks.setup).toHaveBeenCalledTimes(2);
  });

  it('keeps storage degradation visible even if notifications are disabled', async () => {
    mocks.health.mockResolvedValue({ mode: 'memory', message: 'Changes will not persist' });
    const store = makeStore();
    store.setState({ settings: { notify: 'none' } as AppState['settings'] });
    await store.getState().init();
    expect(store.getState().ready).toBe(true);
    expect(store.getState().storageHealth?.mode).toBe('memory');
  });

  it('waits for pending startup reads before releasing the retry lock', async () => {
    let finish!: () => void;
    const pending = new Promise<void>((resolve) => { finish = resolve; });
    const store = makeStore(vi.fn().mockReturnValue(pending));
    store.setState({ refreshCert: vi.fn().mockRejectedValue(new Error('cert failed')) });
    const first = store.getState().init();
    const failure = expect(first).rejects.toThrow('cert failed');
    await vi.waitFor(() => expect(mocks.health).toHaveBeenCalledTimes(1));
    expect(store.getState().init()).toBe(first);
    expect(store.getState().initError).toBeNull();
    finish();
    await failure;
    expect(store.getState().initError).toContain('cert failed');
  });

  it('exposes an unavailable health check and can check again', async () => {
    mocks.health.mockRejectedValueOnce(new Error('IPC failed'));
    const store = makeStore();
    await store.getState().init();
    expect(store.getState().storageHealth?.mode).toBe('unavailable');
    await store.getState().refreshStorageHealth();
    expect(store.getState().storageHealth?.mode).toBe('persistent');
  });
});
