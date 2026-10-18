/**
 * 业务事件 → 桌宠信号（只读订阅，零新增 Rust 转发）
 *
 * 现有事件都是**全局广播**，宠物窗口直接 `listen` 即可，不需要主窗口中转。
 *
 * 两个容易踩的载荷差异（照抄现有前端契约）：
 * - `checkin-progress` 是**对象**；
 * - `wb-checkin-progress` / `switch-progress` 是 **NDJSON 字符串**（payload 为 String，
 *   必须 JSON.parse；按对象读会静默拿到 undefined）。
 */
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useEffect, useRef } from 'react';
import { formatAmount } from './lines';

export type PetSignalType =
  | 'idle'
  | 'working'
  | 'thinking'
  | 'happy'
  | 'sad'
  | 'rich'
  | 'broke';

export type PetSignal = {
  type: PetSignalType;
  /** 气泡文案；空则只播动作不弹气泡 */
  text?: string;
};

type CheckinPayload = {
  type?: string;
  total?: number;
  ok?: number;
  already?: number;
  failed?: number;
  empty?: boolean;
  status?: string;
  name?: string;
  message?: string;
  delta?: number | null;
  credits?: number | null;
  round?: number;
};

type BuddyPayload = {
  type?: string;
  total?: number;
  ok?: number;
  already?: number;
  failed?: number;
  empty?: boolean;
  status?: string;
  message?: string;
  travel?: string;
  lottery?: string;
  tasks?: string;
};

type CreditsPayload = {
  ok?: boolean;
  total_balance?: number | null;
  accounts?: Array<{ balance?: number | null }>;
};

type DonePayload = { success?: boolean; raw?: string };

/** NDJSON 行 / 对象统一解析：两种载荷形态都接受 */
function parseJson<T>(raw: unknown): T | null {
  if (typeof raw === 'string') {
    try {
      return JSON.parse(raw) as T;
    } catch {
      return null;
    }
  }
  if (raw && typeof raw === 'object') return raw as T;
  return null;
}

/** 事件文本可能很长（后端异常栈），气泡只留一行可读的部分 */
function short(text: string | undefined, limit = 42): string | undefined {
  if (!text) return undefined;
  const oneLine = text.replace(/\s+/g, ' ').trim();
  if (!oneLine) return undefined;
  return oneLine.length > limit ? `${oneLine.slice(0, limit)}…` : oneLine;
}

/**
 * 订阅全部业务事件。
 *
 * 节流：同一类信号 2.5s 内只处理一次（`working` 除外，进度必须逐条反映）。
 * 原因：签到/成长是一次性连发几十条 `account` 事件，逐条弹气泡会刷屏。
 */
export function usePetEvents(onSignal: (signal: PetSignal) => void): void {
  const handlerRef = useRef(onSignal);
  handlerRef.current = onSignal;

  useEffect(() => {
    let disposed = false;
    const subs: Array<Promise<UnlistenFn>> = [];
    const lastAt = new Map<PetSignalType, number>();

    const emit = (signal: PetSignal) => {
      if (disposed) return;
      const now = Date.now();
      if (signal.type !== 'working') {
        const prev = lastAt.get(signal.type) ?? 0;
        if (now - prev < 2500) return;
        lastAt.set(signal.type, now);
      }
      handlerRef.current(signal);
    };

    const on = <T>(name: string, fn: (payload: T) => void) => {
      subs.push(
        listen<T>(name, (event) => {
          if (!disposed) fn(event.payload);
        }),
      );
    };

    // ── Trae 签到（对象载荷） ──
    on<CheckinPayload>('checkin-progress', (p) => {
      switch (p?.type) {
        case 'start':
          emit({
            type: 'working',
            text: p.total ? `开工啦，本轮 ${p.total} 个账号` : '没有要签的账号',
          });
          break;
        case 'account':
          if (p.status === 'success') {
            if (p.delta && p.delta > 0) {
              emit({ type: 'rich', text: `+${formatAmount(p.delta)}` });
            } else {
              emit({ type: 'working' });
            }
          } else if (p.status === 'fail') {
            emit({ type: 'sad', text: short(p.message) ?? '这个账号没签上' });
          }
          break;
        case 'retry':
          emit({ type: 'working', text: p.round ? `第 ${p.round} 轮重试…` : '稍等，重试中' });
          break;
        case 'done':
          if ((p.failed ?? 0) > 0) {
            emit({ type: 'sad', text: `${p.failed} 个账号未成功` });
          } else if ((p.ok ?? 0) > 0) {
            emit({ type: 'happy', text: `签到完成，新签 ${p.ok} 个` });
          } else {
            emit({ type: 'idle', text: p.empty ? '今天都签过了' : '本轮结束' });
          }
          break;
        default:
          break;
      }
    });

    // ── WorkBuddy 签到/成长（NDJSON 字符串） ──
    on<unknown>('wb-checkin-progress', (raw) => {
      const p = parseJson<BuddyPayload>(raw);
      if (!p) return;
      switch (p.type) {
        case 'start':
          emit({
            type: 'working',
            text: p.total ? `Buddy 开工：${p.total} 个账号` : 'Buddy 开工',
          });
          break;
        case 'growth':
          if (p.status === 'fail') {
            emit({ type: 'sad', text: short(p.message) ?? '成长任务有失败项' });
          } else {
            emit({ type: 'happy', text: '成长任务跑完了' });
          }
          break;
        case 'account':
          if (p.status === 'fail') {
            emit({ type: 'sad', text: short(p.message) ?? '有个账号没签上' });
          } else {
            emit({ type: 'working' });
          }
          break;
        case 'done':
          if ((p.failed ?? 0) > 0) {
            emit({ type: 'sad', text: `${p.failed} 个账号未成功` });
          } else if ((p.ok ?? 0) > 0) {
            emit({ type: 'happy', text: `Buddy 签到完成，新签 ${p.ok} 个` });
          } else {
            emit({ type: 'idle' });
          }
          break;
        case 'exit':
          // 轮次退出：无论成败都该回落，否则宠物会一直「忙碌」
          emit({ type: 'idle' });
          break;
        default:
          break;
      }
    });

    // ── Buddy 积分刷新（对象载荷） ──
    on<CreditsPayload>('wb-credits-updated', (p) => {
      if (!p) return;
      const total =
        typeof p.total_balance === 'number'
          ? p.total_balance
          : (p.accounts ?? []).reduce((sum, a) => sum + (a.balance ?? 0), 0);
      if (!Number.isFinite(total)) return;
      if (total <= 0) {
        emit({ type: 'broke', text: 'Buddy 余额见底了' });
      } else {
        emit({ type: 'rich', text: `Buddy 余额 ${formatAmount(total)}` });
      }
    });

    // ── 账号切换/保存登录态（NDJSON 字符串 + 终态对象） ──
    on<unknown>('switch-progress', (raw) => {
      const p = parseJson<{ stage?: string; status?: string; message?: string }>(raw);
      if (!p) return;
      if (p.status === 'error' || p.status === 'fail') {
        emit({ type: 'sad', text: short(p.message) ?? '切换没成功' });
      } else {
        emit({ type: 'working', text: short(p.message) ?? '正在切换账号…' });
      }
    });

    on<DonePayload>('switch-done', (p) => {
      emit(
        p?.success
          ? { type: 'happy', text: '切换完成' }
          : { type: 'sad', text: short(p?.raw) ?? '切换失败' },
      );
    });

    on<DonePayload>('save-login-done', (p) => {
      emit(
        p?.success
          ? { type: 'happy', text: '登录态已保存' }
          : { type: 'sad', text: short(p?.raw) ?? '保存登录态失败' },
      );
    });

    return () => {
      disposed = true;
      for (const sub of subs) {
        void sub.then((un) => un()).catch(() => {});
      }
    };
  }, []);
}
