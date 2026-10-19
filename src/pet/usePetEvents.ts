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
  | 'broke'
  // 以下为「纯瞬态」信号：只叠加播一次动作，不改核心态
  | 'pace'
  | 'archive'
  | 'neutral'
  | 'turn'
  | 'jump';

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
  /** 失败分类（与 Rust classify_error 返回值同名） */
  error_type?: string;
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

/** 积分增量达到该阈值即视为「里程碑」，改用更夸张的 jump-grab 动作 */
const MILESTONE_DELTA = 500;

/** 失败分类 → 中文短标签（与 Rust `classify_error` 的返回值对齐） */
const ERROR_LABELS: Record<string, string> = {
  PlanLimit: '套餐上限',
  SoftRate: '触发限流',
  SessionDead: '登录失效',
  NotFound: '接口异常',
  Server: '服务端错误',
  Client: '请求被拒',
  BusinessError: '业务失败',
  Unknown: '未知原因',
};

/** 没有失败分类时用（Buddy 的 account 事件不带 error_type） */
const NO_REASONS = new Map<string, number>();

/** 取出现最多的 1~2 类失败原因，拼成「登录失效 2、触发限流」 */
function topReasons(reasons: Map<string, number>, max = 2): string {
  return [...reasons.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, max)
    .map(([key, n]) => {
      const label = ERROR_LABELS[key] ?? key;
      return n > 1 ? `${label} ${n}` : label;
    })
    .join('、');
}

/**
 * 一轮结束的总结：签了几个 / 几个已签 / 几个失败 + 主要原因。
 *
 * 之前的文案只有「成功 X / 失败 Y」或一句「签到完成」，用户看不出
 * 「今天到底为什么没签上」——失败原因只有逐账号事件里有。
 */
function describeDone(
  ok: number,
  already: number,
  failed: number,
  reasons: Map<string, number>,
): { type: PetSignalType; text: string } {
  const counts: string[] = [];
  if (ok > 0) counts.push(`新签 ${ok}`);
  if (already > 0) counts.push(`已签 ${already}`);
  if (failed > 0) counts.push(`失败 ${failed}`);

  if (failed > 0) {
    const why = topReasons(reasons);
    if (ok + already === 0) {
      return { type: 'sad', text: `${failed} 个都没签上${why ? `：${why}` : ''}` };
    }
    return { type: 'archive', text: `${counts.join(' · ')}${why ? `（${why}）` : ''}` };
  }
  if (ok > 0) return { type: 'happy', text: counts.join(' · ') };
  if (already > 0) return { type: 'idle', text: `${already} 个今天都签过了` };
  return { type: 'idle', text: '今天没有要签的账号' };
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
    // 余额上一次已知值：用于判断「查询完成但无变化」→ neutral（balance-normal）
    let lastCredits: number | null = null;
    // 切换流程是否已跨过「回合开始」阶段（首行 thinking → 后续 working）
    let switchStarted = false;
    // Trae 本轮失败分类计数：done 时汇总成「服务端错误 2、登录失效 1」
    const traeReasons = new Map<string, number>();

    const emit = (signal: PetSignal, force = false) => {
      if (disposed) return;
      const now = Date.now();
      // force：核心态收尾（done/exit 降回 idle）必须生效，不能被同类节流吞掉，
      // 否则桌宠会一直停留在 thinking/working 的循环动作里
      if (!force && signal.type !== 'working') {
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
          // 回合刚开始、还没进实际处理 → thinking（work-think）。
          // 对齐上游 workStatus[0]=thinking（turn/start）、[1]=working（tool/call）
          traeReasons.clear();
          emit({
            type: 'thinking',
            text: p.total ? `开工啦，本轮 ${p.total} 个账号` : '没有要签的账号',
          });
          break;
        case 'account':
          // 进入实际处理：核心态升到 working，再叠加积分瞬态（瞬态播完回落 working）
          emit({ type: 'working' });
          if (p.status === 'success') {
            if (p.delta && p.delta > 0) {
              // 大额增量 → jump-grab（里程碑），普通增量 → rich
              emit({
                type: p.delta >= MILESTONE_DELTA ? 'jump' : 'rich',
                text: `+${formatAmount(p.delta)}`,
              });
            } else if (typeof p.credits === 'number') {
              // Trae 的 claim 响应经常不带奖励字段（delta=0），此时退化为播报该账号当前积分，
              // 否则桌宠签 Trae 时全程静默——只有 Buddy 有 wb-credits-updated，Trae 没有余额事件
              emit({ type: 'rich', text: `Trae 积分 ${formatAmount(p.credits)}` });
            } else {
              emit({ type: 'neutral', text: '签上了，积分没变化' });
            }
          } else if (p.status === 'already') {
            // 今天已签：给个反馈，别让桌宠像没收到消息
            emit({ type: 'neutral', text: '今天签过啦' });
          } else if (p.status === 'fail') {
            // 记下失败分类：done 时要汇总成「服务端错误 2、登录失效 1」
            if (p.error_type) {
              traeReasons.set(p.error_type, (traeReasons.get(p.error_type) ?? 0) + 1);
            }
            emit({ type: 'sad', text: short(p.message) ?? '这个账号没签上' });
          }
          break;
        case 'retry':
          // 核心态保持 working，叠加 work-pace 的「等待踱步」动作
          emit({ type: 'working' });
          emit({ type: 'pace', text: p.round ? `第 ${p.round} 轮重试…` : '稍等，重试中' });
          break;
        case 'done': {
          // 收尾先把核心态降回 idle：walking/thinking 的循环动作不降下来会一直转
          emit({ type: 'idle' }, true);
          // 总结：新签几个 / 几个已签 / 失败几个 + 失败原因（force，别被同类节流吞掉）
          const summary = describeDone(p.ok ?? 0, p.already ?? 0, p.failed ?? 0, traeReasons);
          traeReasons.clear();
          emit(summary, true);
          break;
        }
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
          // 同 Trae：回合开始先 thinking（work-think），进入账号处理再升 working
          emit({
            type: 'thinking',
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
          // 先升核心态，失败再叠加 sad（sad 优先级最高，不会被 working 盖掉）
          emit({ type: 'working' });
          if (p.status === 'fail') {
            emit({ type: 'sad', text: short(p.message) ?? '有个账号没签上' });
          }
          break;
        case 'done': {
          // 同 Trae：收尾先降核心态，再叠加总结（Buddy 事件里没有 error_type，只报计数）
          emit({ type: 'idle' }, true);
          emit(
            describeDone(p.ok ?? 0, p.already ?? 0, p.failed ?? 0, NO_REASONS),
            true,
          );
          break;
        }
        case 'exit':
          // 轮次退出：无论成败都该回落，否则宠物会一直「忙碌」
          emit({ type: 'idle' }, true);
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
      const prev = lastCredits;
      lastCredits = total;
      if (total <= 0) {
        emit({ type: 'broke', text: 'Buddy 余额见底了' });
      } else if (prev !== null && total === prev) {
        // 查询完成但余额没变 → balance-normal
        emit({ type: 'neutral', text: '余额没有变化' });
      } else {
        emit({ type: 'rich', text: `Buddy 余额 ${formatAmount(total)}` });
      }
    });

    // ── 账号切换/保存登录态（NDJSON 字符串 + 终态对象） ──
    on<unknown>('switch-progress', (raw) => {
      const p = parseJson<{ stage?: string; status?: string; message?: string }>(raw);
      if (!p) return;
      if (p.status === 'error' || p.status === 'fail') {
        switchStarted = false;
        emit({ type: 'sad', text: short(p.message) ?? '切换没成功' });
        return;
      }
      // 首行 = 回合开始（thinking / work-think），随后进入实际处理（working + turn-run 手势）
      if (!switchStarted) {
        switchStarted = true;
        emit({ type: 'thinking', text: short(p.message) ?? '正在切换账号…' });
        return;
      }
      emit({ type: 'working' });
      emit({ type: 'turn', text: short(p.message) ?? '正在切换账号…' });
    });

    on<DonePayload>('switch-done', (p) => {
      switchStarted = false;
      // 收尾降核心态，再叠加成败瞬态（否则会停在 thinking/working 循环动作）
      emit({ type: 'idle' }, true);
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
