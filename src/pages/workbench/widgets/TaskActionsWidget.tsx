import { useState } from 'react';
import {
  PlayCircle,
  Server,
  Copy,
  RefreshCw,
  FolderOpen,
} from 'lucide-react';
import { useAppStore } from '../../../store';
import { api } from '../../../lib/tauri';

/** 单个动作按钮：图标 + 文案 + 可选副说明 */
function ActionRow({
  icon: Icon,
  label,
  hint,
  busy,
  onClick,
}: {
  icon: typeof PlayCircle;
  label: string;
  hint?: string;
  busy?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      disabled={busy}
      className="flex w-full items-center gap-2.5 rounded-lg px-2 py-2 text-left transition hover:bg-slate-50 active:scale-[0.985] disabled:opacity-50 dark:hover:bg-zinc-800/60"
    >
      <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-slate-100 text-slate-600 dark:bg-zinc-800 dark:text-zinc-300">
        <Icon size={15} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block text-sm font-medium leading-tight text-slate-800 dark:text-zinc-100">
          {label}
        </span>
        {hint && (
          <span className="mt-0.5 block truncate text-[11px] text-slate-400 dark:text-zinc-500">
            {hint}
          </span>
        )}
      </span>
    </button>
  );
}

/**
 * 区块③：一键动作。
 *
 * 只放**高频且幂等**的操作。会改变用户当前环境的（如切换登录账号）不放这里——
 * 误点代价太高，应去账号管理页确认后再操作。
 */
export default function TaskActionsWidget() {
  const setView = useAppStore((s) => s.setView);
  const pushToast = useAppStore((s) => s.pushToast);
  const apiStatus = useAppStore((s) => s.apiStatus);
  const settings = useAppStore((s) => s.settings);
  const [busy, setBusy] = useState<string | null>(null);

  const run = async (key: string, fn: () => Promise<void>) => {
    if (busy) return;
    setBusy(key);
    try {
      await fn();
    } catch (e) {
      pushToast('error', `${key}失败：${String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="space-y-0.5">
      <ActionRow
        icon={PlayCircle}
        label="开始签到"
        hint="按当前设置跳过已签/过期"
        busy={busy === 'checkin'}
        onClick={() =>
          void run('checkin', async () => {
            await api.checkin.start({
              scope: 'all',
              skip_checked_in: settings?.checkin_skip_checked ?? false,
              skip_expired: settings?.checkin_skip_expired ?? false,
            });
            setView('checkin');
          })
        }
      />
      <ActionRow
        icon={Server}
        label={apiStatus?.running ? '重启网关' : '启动网关'}
        hint={apiStatus?.running ? `当前端口 ${apiStatus.port}` : 'IDE 与第三方工具调用入口'}
        busy={busy === 'gateway'}
        onClick={() =>
          void run('gateway', async () => {
            if (apiStatus?.running) await api.apiServer.stop();
            await api.apiServer.start();
            await useAppStore.getState().refreshApiStatus();
            pushToast('success', apiStatus?.running ? '网关已重启' : '网关已启动');
          })
        }
      />
      <ActionRow
        icon={RefreshCw}
        label="刷新账号数据"
        hint="重新读取本地账号池"
        busy={busy === 'refresh'}
        onClick={() =>
          void run('refresh', async () => {
            await useAppStore.getState().refreshAccounts();
            pushToast('success', '账号数据已刷新');
          })
        }
      />
      <ActionRow
        icon={Copy}
        label="复制 API Base"
        hint={`http://127.0.0.1:${apiStatus?.port || settings?.api_port || 7864}/v1`}
        onClick={() =>
          void run('copy', async () => {
            const base = `http://127.0.0.1:${apiStatus?.port || settings?.api_port || 7864}/v1`;
            await navigator.clipboard.writeText(base);
            pushToast('success', `已复制：${base}`);
          })
        }
      />
      <ActionRow
        icon={FolderOpen}
        label="打开数据目录"
        hint="备份 / 日志 / 导出"
        onClick={() => setView('settings')}
      />
    </div>
  );
}
