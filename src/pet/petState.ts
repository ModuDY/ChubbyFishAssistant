/**
 * 桌宠状态机（优先级聚合）
 *
 * 为什么不是"简单抢占"：签到出错后紧跟一条进度事件，直接抢占会把「出错」表情
 * 一秒冲掉，用户根本看不到失败提示。上游 `dsh-pet-indesktop` 的做法是给状态定优先级、
 * 取最高者——这里照搬同一思路：
 *
 *     出错(sad) > 进行中(working) > 思考(thinking) > 打盹(sleepy) > 空闲(idle)
 *
 * 规则：
 * - 高优先级可覆盖低优先级；**低优先级不得打断高优先级**；
 * - `happy` / `rich` / `broke` 是**瞬态**（一次性播完就回落），不参与优先级竞争，
 *   因为它们表达"刚刚发生了什么"，而不是"现在处于什么状态"；
 * - 同一优先级内新事件覆盖旧事件。
 *
 * 动作池集中在 `pools.ts`（单一事实来源），本模块只负责"该用哪个池"。
 */
import { ACTIONS } from './actions';
import { CORE_POOLS, TRANSIENT_POOLS, pickFromPool } from './pools';

/** 参与优先级竞争的"核心态" */
export type PetCoreState = 'idle' | 'sleepy' | 'thinking' | 'working' | 'sad';

const PRIORITY: Record<PetCoreState, number> = {
  idle: 0,
  sleepy: 1,
  thinking: 2,
  working: 3,
  sad: 4,
};

/** 请求切换到 next：只有不低于当前优先级时才生效 */
export function raise(current: PetCoreState, next: PetCoreState): PetCoreState {
  return PRIORITY[next] >= PRIORITY[current] ? next : current;
}

/** 当前态是否高于目标态（用于判断"低优先级事件不该打断"） */
export function isHigher(current: PetCoreState, target: PetCoreState): boolean {
  return PRIORITY[current] > PRIORITY[target];
}

/**
 * 瞬态语义。前 11 个是业务/时间驱动的原语义；
 * `feed` / `play` / `perform` / `pet` / `tease` 是 2026-10-20 新增的**用户主动互动**语义，
 * 让 106 个动作里的吃喝玩乐才艺真正被"人"点到，而不只是随机漂出来。
 */
export type PetTransientKind =
  | 'happy'
  | 'rich'
  | 'broke'
  | 'click'
  | 'idle'
  | 'pace'
  | 'archive'
  | 'neutral'
  | 'turn'
  | 'jump'
  | 'mumble'
  | 'feed'
  | 'play'
  | 'perform'
  | 'pet'
  | 'tease'
  | 'festival';

export type PetTransient = {
  /** 每次触发递增：同一动作连播时也能重启视频 */
  seq: number;
  kind: PetTransientKind;
  action: string;
  text?: string;
  /** 兜底时长（素材 onEnded 丢失时也要回落，例如解码卡住） */
  ms: number;
};

/** 按语义随机挑一个动作（供调用方先拿到 action、再查它的实测时长） */
export function pickTransientAction(kind: PetTransientKind, override?: string): string {
  return override ?? pickFromPool(TRANSIENT_POOLS[kind]) ?? 'click-wave';
}

export function makeTransient(
  seq: number,
  kind: PetTransientKind,
  action: string,
  text?: string,
  /** 素材实测时长（ms），由 PetSprite 的 onDuration 上报 */
  durationMs?: number,
): PetTransient {
  // 兜底时长口径（2026-10-19 修正）：
  // 素材普遍 10s，原来固定 2.6s/4.2s 会在第 4 秒把动作切走——一次性动作只播了不到一半。
  // 现在优先用实测时长（+400ms 余量让 onEnded 先到），没测到时给 12s 上限。
  const ms = durationMs && durationMs > 0 ? durationMs + 400 : 12_000;
  return { seq, kind, action, text, ms };
}

/**
 * 核心态 → 动作（池内按 tick 交替）。
 *
 * `working` 现在是 `work-busy` / `work-pace` 两个循环动作交替：签到一轮可能跑几十秒，
 * 一直播同一段循环最容易看出是"假动画"。非 idle 态也一并走 tick，
 * 池子长度 >1 时自动获得交替能力（改池子即可，不用改这里）。
 */
export function coreAction(state: PetCoreState, tick: number): string {
  const pool = CORE_POOLS[state].filter((id) => ACTIONS[id]);
  if (pool.length === 0) return 'idle-breathe';
  return pool[((tick % pool.length) + pool.length) % pool.length];
}

/** 该核心态的动作池是否有多个候选（有则值得按 tick 交替） */
export function corePoolSize(state: PetCoreState): number {
  return CORE_POOLS[state].filter((id) => ACTIONS[id]).length;
}

/** 动作是否循环播放（来自 manifest，唯一事实来源） */
export function isLoopAction(id: string): boolean {
  return ACTIONS[id]?.loop ?? true;
}

/** 舞台文案用中文名（诊断/调试时能直接看出播的是哪个动作） */
export function actionLabel(id: string): string {
  return ACTIONS[id]?.label ?? id;
}
