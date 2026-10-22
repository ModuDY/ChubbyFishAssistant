import { useMemo, useState } from 'react';
import { AlertCircle, AlertTriangle, Info, ArrowRight } from 'lucide-react';
import { useAppStore } from '../../../store';
import { api } from '../../../lib/tauri';
import { buildTodos, type TodoItem } from '../../../lib/workbenchTodos';
import { cn } from '../../../lib/cn';

const TONE_ICON = {
  danger: AlertCircle,
  warn: AlertTriangle,
  info: Info,
} as const;

const TONE_CLASS = {
  danger: 'text-rose-500 dark:text-rose-400',
  warn: 'text-amber-500 dark:text-amber-400',
  info: 'text-sky-500 dark:text-sky-400',
} as const;

/**
 * 区块②：待办 / 提醒（工作台核心）。
 *
 * 关键设计（方案 §4.2 / §10）：
 * - 每条都是**规则自动生成**，用户不需要维护待办清单；
 * - **没有可做的事就返回空** → 本组件直接不渲染（父级也不给卡片外壳），
 *   而不是显示一张"今日无待办"的空卡制造噪音；
 * - 每条必须能一键执行或明确跳到对应页面。
 */
export default function TaskTodosWidget() {
  const accounts = useAppStore((s) => s.accounts);
  const apiStatus = useAppStore((s) => s.apiStatus);
  const certInstalled = useAppStore((s) => s.certInstalled);
  const env = useAppStore((s) => s.env);
  const settings = useAppStore((s) => s.settings);
  const setView = useAppStore((s) => s.setView);
  const pushToast = useAppStore((s) => s.pushToast);

  const [busyId, setBusyId] = useState<string | null>(null);

  const todos = useMemo(
    () =>
      buildTodos({
        accounts,
        apiStatus,
        certInstalled,
        env,
        // Buddy / Qoder 账号数不在 store.accounts 里（各自独立命令），
        // 这里按 settings 是否有对应配置做弱判断：
        // 有配置说明该平台在用，才提示签到，避免给未启用的平台制造噪音。
        platformCounts: {
          trae: accounts.length,
          buddy: settings?.buddy_switch_migrate_chats !== undefined && accounts.length > 0 ? 1 : 0,
          qoder: settings?.qoder_checkin_hhmm && accounts.length > 0 ? 1 : 0,
        },
      }),
    [accounts, apiStatus, certInstalled, env, settings],
  );

  /** 执行一条待办的动作；失败必须明示（项目红线：失败不静默） */
  const run = async (t: TodoItem) => {
    if (!t.action) return;
    const act = t.action;

    if (act.kind === 'nav') {
      setView(act.view);
      return;
    }

    setBusyId(t.id);
    try {
      switch (act.kind) {
        case 'renew_jwt': {
          // 只续期"有 refresh_token"的账号：无 token 的必然失败，跳过并如实汇报
          const targets = accounts.filter(
            (a) => a.jwt_exp_hours !== null && a.jwt_exp_hours <= 24 && a.has_refresh_token,
          );
          if (targets.length === 0) {
            pushToast('warn', '没有可自动续期的账号，请到账号管理重新登录');
            return;
          }
          const results = await Promise.allSettled(
            targets.map((a) => api.accounts.refreshJwt(a.user_id, true)),
          );
          const ok = results.filter((r) => r.status === 'fulfilled').length;
          const fail = results.length - ok;
          if (fail === 0) {
            pushToast('success', `${ok} 个账号 JWT 已续期`);
          } else {
            // 部分失败要报出具体数量，不能笼统说"完成"
            pushToast('warn', `续期完成：成功 ${ok}，失败 ${fail}（失败账号需重新登录）`);
          }
          await useAppStore.getState().refreshAccounts();
          break;
        }
        case 'checkin': {
          // 各平台签到入口不同：Trae 走 checkin.start，其余跳到对应页面
          if (act.platform === 'trae') {
            // 跳过开关沿用用户设置（settings 未加载完成时按"全不跳过"= 最保守：
            // 宁可用户多点几秒，也不要在后台静默跳过本该签到的账号）
            await api.checkin.start({
              scope: 'all',
              skip_checked_in: settings?.checkin_skip_checked ?? false,
              skip_expired: settings?.checkin_skip_expired ?? false,
            });
            pushToast('info', '签到已开始，进度见「一键签到」页');
            setView('checkin');
          } else if (act.platform === 'buddy') {
            setView('buddy-checkin');
          } else {
            setView('qoder-checkin');
          }
          break;
        }
        case 'install_cert': {
          await api.cert.install();
          await useAppStore.getState().refreshCert();
          pushToast('success', '证书安装成功');
          break;
        }
        case 'start_api': {
          await api.apiServer.start();
          await useAppStore.getState().refreshApiStatus();
          pushToast('success', 'API 网关已启动');
          break;
        }
      }
    } catch (e) {
      pushToast('error', `${t.actionLabel ?? '操作'}失败：${String(e)}`);
    } finally {
      setBusyId(null);
    }
  };

  // 空态：整块不渲染（父级据此隐藏卡片外壳）
  if (todos.length === 0) return null;

  return (
    <ul className="space-y-1.5">
      {todos.map((t) => {
        const Icon = TONE_ICON[t.tone];
        const busy = busyId === t.id;
        return (
          <li
            key={t.id}
            className="flex items-start gap-2.5 rounded-lg px-1 py-1.5 transition hover:bg-slate-50/70 dark:hover:bg-zinc-800/40"
          >
            <Icon size={15} className={cn('mt-0.5 shrink-0', TONE_CLASS[t.tone])} />
            <div className="min-w-0 flex-1">
              <div className="text-sm font-medium leading-snug text-slate-800 dark:text-zinc-100">
                {t.title}
              </div>
              {t.detail && (
                <div className="mt-0.5 text-xs leading-snug text-slate-500 dark:text-zinc-400">
                  {t.detail}
                </div>
              )}
            </div>
            {t.actionLabel && (
              <button
                onClick={() => void run(t)}
                disabled={busy}
                className="btn-outline shrink-0 !px-2 !py-1 text-xs"
              >
                {busy ? '处理中…' : t.actionLabel}
                {!busy && <ArrowRight size={12} />}
              </button>
            )}
          </li>
        );
      })}
    </ul>
  );
}

/** 供父级判断"待办区块是否有内容"：空则整块隐藏，不显示空卡 */
export function useTodosEmpty(): boolean {
  const accounts = useAppStore((s) => s.accounts);
  const apiStatus = useAppStore((s) => s.apiStatus);
  const certInstalled = useAppStore((s) => s.certInstalled);
  const env = useAppStore((s) => s.env);
  const settings = useAppStore((s) => s.settings);
  return useMemo(
    () =>
      buildTodos({
        accounts,
        apiStatus,
        certInstalled,
        env,
        platformCounts: {
          trae: accounts.length,
          buddy: settings?.buddy_switch_migrate_chats !== undefined && accounts.length > 0 ? 1 : 0,
          qoder: settings?.qoder_checkin_hhmm && accounts.length > 0 ? 1 : 0,
        },
      }).length === 0,
    [accounts, apiStatus, certInstalled, env, settings],
  );
}
