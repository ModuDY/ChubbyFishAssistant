import { useMemo } from 'react';
import { useAppStore } from '../../../store';
import { cn } from '../../../lib/cn';

/** 单个状态格：label + 主值 + 副说明；点击可跳到对应页面 */
function StatusCell({
  label,
  value,
  hint,
  tone = 'slate',
  onClick,
}: {
  label: string;
  value: string | number;
  hint?: string;
  tone?: 'slate' | 'green' | 'amber' | 'red';
  onClick?: () => void;
}) {
  const toneClass = {
    slate: 'text-slate-800 dark:text-zinc-100',
    green: 'text-emerald-600 dark:text-emerald-400',
    amber: 'text-amber-600 dark:text-amber-400',
    red: 'text-rose-600 dark:text-rose-400',
  }[tone];

  const Comp = onClick ? 'button' : 'div';
  return (
    <Comp
      onClick={onClick}
      className={cn(
        'flex flex-col items-start rounded-lg px-3 py-2 text-left',
        onClick && 'transition hover:bg-slate-50 active:scale-[0.98] dark:hover:bg-zinc-800/60',
      )}
    >
      <span className="text-[11px] font-medium text-slate-500 dark:text-zinc-400">{label}</span>
      <span className={cn('mt-0.5 text-lg font-semibold tabular-nums leading-tight', toneClass)}>
        {value}
      </span>
      {hint && <span className="mt-0.5 text-[11px] text-slate-400 dark:text-zinc-500">{hint}</span>}
    </Comp>
  );
}

/**
 * 区块①：今日状态条。
 *
 * 只放"今天这一刻的实际状态"，不放趋势、不放排行（那是 Dashboard 的活）。
 * 数据全部来自 store，零额外 IPC。
 */
export default function TaskStatusWidget() {
  const accounts = useAppStore((s) => s.accounts);
  const apiStatus = useAppStore((s) => s.apiStatus);
  const certInstalled = useAppStore((s) => s.certInstalled);
  const setView = useAppStore((s) => s.setView);

  const stats = useMemo(() => {
    const nowSec = Math.floor(Date.now() / 1000);
    const total = accounts.length;
    const checked = accounts.filter((a) => a.checked_today).length;
    // 告警口径与 Dashboard 一致：JWT 24h 内过期 + 积分 7 日内过期
    const jwtWarn = accounts.filter(
      (a) => a.jwt_exp_hours !== null && a.jwt_exp_hours <= 24,
    ).length;
    const creditWarn = accounts.filter(
      (a) =>
        a.credits_expire_at != null &&
        a.credits_expire_at > nowSec &&
        a.credits_expire_at <= nowSec + 7 * 86400,
    ).length;
    const totalCredits = accounts.reduce((s, a) => s + (a.remaining_credits ?? 0), 0);
    return { total, checked, alerts: jwtWarn + creditWarn, totalCredits };
  }, [accounts]);

  const fmtCredits = (v: number) =>
    v.toLocaleString('zh-CN', { maximumFractionDigits: 0 });

  return (
    <div className="grid grid-cols-2 gap-1 sm:grid-cols-3 lg:grid-cols-6">
      <StatusCell
        label="今日签到"
        value={`${stats.checked}/${stats.total}`}
        hint={stats.total > 0 && stats.checked === stats.total ? '已全部完成' : '未完成'}
        tone={stats.total > 0 && stats.checked === stats.total ? 'green' : 'amber'}
        onClick={() => setView('checkin')}
      />
      <StatusCell
        label="可用积分"
        value={fmtCredits(stats.totalCredits)}
        hint="全部 Trae 账号合计"
        onClick={() => setView('credits')}
      />
      <StatusCell
        label="告警"
        value={stats.alerts}
        hint={stats.alerts > 0 ? 'JWT / 积分临期' : '无临期项'}
        tone={stats.alerts > 0 ? 'red' : 'green'}
        onClick={() => setView('dashboard')}
      />
      <StatusCell
        label="网关"
        value={apiStatus?.running ? '运行中' : '已停止'}
        hint={apiStatus?.running ? `端口 ${apiStatus.port}` : 'IDE 无法调用'}
        tone={apiStatus?.running ? 'green' : 'slate'}
        onClick={() => setView('api-service')}
      />
      <StatusCell
        label="证书"
        value={certInstalled ? '已安装' : '未安装'}
        hint={certInstalled ? 'HTTPS 可解密' : '抓包会失败'}
        tone={certInstalled ? 'green' : 'red'}
        onClick={() => setView('settings')}
      />
      <StatusCell
        label="账号"
        value={stats.total}
        hint="Trae 账号池"
        onClick={() => setView('accounts')}
      />
    </div>
  );
}
