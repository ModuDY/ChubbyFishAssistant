import { useMemo } from 'react';
import { Coins, PlayCircle, ScrollText, ArrowRight } from 'lucide-react';
import { useAppStore } from '../../../store';

/**
 * 区块④：最近 & 常用。
 *
 * P0 阶段后端还没有 workflow_runs / 知识库（那是 P1/P2），
 * 因此这里先接**已有数据**里"最近发生过什么"最有用的两类：
 * - 最近签到记录（来自 store.creditsDaily / accounts 的今日状态）
 * - 常用页面入口（按使用频率手排，不做统计）
 *
 * 待 P1 落地后在此追加"最近索引的文档"，P2 追加"最近工作流运行"——
 * 本组件的结构已按"条目列表 + 跳转"抽象，加数据源不用改布局。
 */
export default function TaskRecentWidget() {
  const setView = useAppStore((s) => s.setView);
  const accounts = useAppStore((s) => s.accounts);
  const creditsDaily = useAppStore((s) => s.creditsDaily);
  const apiStatus = useAppStore((s) => s.apiStatus);

  const recent = useMemo(() => {
    const items: {
      key: string;
      icon: typeof Coins;
      title: string;
      hint: string;
      onClick: () => void;
    }[] = [];

    // 最近一天有一条积分快照 → 显示"最近同步"
    const lastSnap = creditsDaily.length > 0 ? creditsDaily[creditsDaily.length - 1] : null;
    if (lastSnap) {
      items.push({
        key: 'credits-snap',
        icon: Coins,
        title: `积分快照 ${lastSnap.date ?? ''}`.trim(),
        hint: '查看积分变化明细',
        onClick: () => setView('credits'),
      });
    }

    // 今日已签账号数 → 指向签到记录
    const checked = accounts.filter((a) => a.checked_today).length;
    if (checked > 0) {
      items.push({
        key: 'checkin-today',
        icon: PlayCircle,
        title: `今日已签 ${checked} 个账号`,
        hint: '查看签到结果与趋势',
        onClick: () => setView('checkin'),
      });
    }

    // 网关在跑 → 直接去用量统计
    if (apiStatus?.running) {
      items.push({
        key: 'api-usage',
        icon: ScrollText,
        title: '网关运行中',
        hint: `已处理 ${apiStatus.total_requests} 次请求 · 查看用量`,
        onClick: () => setView('api-service'),
      });
    }

    return items;
  }, [accounts, creditsDaily, apiStatus, setView]);

  if (recent.length === 0) {
    return (
      <div className="py-3 text-center text-xs text-slate-400 dark:text-zinc-500">
        还没有可展示的最近记录，完成一次签到或开启网关后会出现在这里
      </div>
    );
  }

  return (
    <ul className="space-y-0.5">
      {recent.map((it) => {
        const Icon = it.icon;
        return (
          <li key={it.key}>
            <button
              onClick={it.onClick}
              className="group flex w-full items-center gap-2.5 rounded-lg px-2 py-2 text-left transition hover:bg-slate-50 dark:hover:bg-zinc-800/60"
            >
              <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-slate-100 text-slate-600 dark:bg-zinc-800 dark:text-zinc-300">
                <Icon size={15} />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-medium leading-tight text-slate-800 dark:text-zinc-100">
                  {it.title}
                </span>
                <span className="mt-0.5 block truncate text-[11px] text-slate-400 dark:text-zinc-500">
                  {it.hint}
                </span>
              </span>
              <ArrowRight
                size={13}
                className="shrink-0 text-slate-300 opacity-0 transition group-hover:opacity-100 dark:text-zinc-600"
              />
            </button>
          </li>
        );
      })}
    </ul>
  );
}
