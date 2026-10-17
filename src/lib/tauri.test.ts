import { beforeEach, expect, it, vi } from 'vitest';
import { listen } from '@tauri-apps/api/event';
import { setupListeners } from './tauri';

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

beforeEach(() => vi.resetAllMocks());

it('rolls back partially registered listeners and allows a clean retry', async () => {
  const unlisten = vi.fn();
  vi.mocked(listen).mockResolvedValueOnce(unlisten).mockRejectedValueOnce(new Error('registration failed'));
  const handlers = { onProxyLog: vi.fn(), onAccountCaptured: vi.fn() };
  await expect(setupListeners(handlers)).rejects.toThrow('registration failed');
  expect(unlisten).toHaveBeenCalledTimes(1);

  vi.mocked(listen).mockResolvedValue(vi.fn());
  const cleanup = await setupListeners(handlers);
  expect(cleanup).toHaveLength(2);
});

it('continues cleanup if a cancellation callback throws', async () => {
  const first = vi.fn();
  const second = vi.fn(() => { throw new Error('cancel failed'); });
  vi.mocked(listen).mockResolvedValueOnce(first).mockResolvedValueOnce(second)
    .mockRejectedValueOnce(new Error('registration failed'));
  await expect(setupListeners({ onProxyLog: vi.fn(), onAccountCaptured: vi.fn(), onCheckinProgress: vi.fn() }))
    .rejects.toThrow('registration failed');
  expect(first).toHaveBeenCalledTimes(1);
  expect(second).toHaveBeenCalledTimes(1);
});
