import { useAppStore } from '../store';

export default function StorageHealthBanner() {
  const health = useAppStore((s) => s.storageHealth);
  const refresh = useAppStore((s) => s.refreshStorageHealth);
  if (!health || health.mode === 'persistent') return null;

  return (
    <div role="alert" className="flex items-center gap-3 border-b border-amber-300 bg-amber-50 px-5 py-3 text-sm text-amber-950 dark:border-amber-800 dark:bg-amber-950 dark:text-amber-100">
      <p className="min-w-0 flex-1 break-words">{health.message}</p>
      {health.mode === 'unavailable' && (
        <button className="shrink-0 underline" onClick={() => void refresh()}>重新检查</button>
      )}
    </div>
  );
}
