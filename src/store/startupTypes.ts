import type { StorageHealth } from '../types';

export interface StartupSlice {
  ready: boolean;
  initError: string | null;
  storageHealth: StorageHealth | null;
  init: () => Promise<void>;
  refreshStorageHealth: () => Promise<void>;
}
