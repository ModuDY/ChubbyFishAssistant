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
 */
import { ACTIONS } from './actions';

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

/** 瞬态类型 → 动作候选（同一语义有多个素材时随机取一个，避免每次都一样） */
const TRANSIENT_ACTIONS: Record<PetTransientKind, string[]> = {
  happy: ['work-celebrate', 'click-happy'],
  rich: ['balance-jingle', 'balance-full', 'balance-count'],
  broke: ['balance-broke', 'balance-empty'],
  click: ['click-wave', 'click-happy', 'click-tickle', 'click-tsundere'],
  // 空闲彩蛋（低频）：东张西望 / 发呆碎碎念 / 深度思考碎碎念。
  // 上游把「深度思考碎碎念」放在 animations.categories 的「文字」随机池（weight 10）里，
  // 不是 thinking 态专用动作，所以归 idle 随机池，而不是"working 无进度"的 mumble 池
  idle: ['idle-look', 'mumble-daze', 'mumble-deep'],
  // 重试等待（签到 retry）
  pace: ['work-pace'],
  // 批量收尾（done 混合结果时清点归档）
  archive: ['work-archive'],
  // 余额查询无变化
  neutral: ['balance-normal'],
  // 切换账号 / 切换页面
  turn: ['turn-run'],
  // 大额到账 / 里程碑
  jump: ['jump-grab'],
  // 长时间 working 但无进度（对屏碎碎念）
  mumble: ['mumble-screen'],
};

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
  | 'mumble';

export type PetTransient = {
  /** 每次触发递增：同一动作连播时也能重启视频 */
  seq: number;
  kind: PetTransientKind;
  action: string;
  text?: string;
  /** 兜底时长（素材 onEnded 丢失时也要回落，例如解码卡住） */
  ms: number;
};

export function makeTransient(
  seq: number,
  kind: PetTransientKind,
  text?: string,
  /** 指定动作（如拖拽落下固定用 click-shy），省略则从候选池随机 */
  actionOverride?: string,
): PetTransient {
  const pool = TRANSIENT_ACTIONS[kind];
  const action = actionOverride ?? pool[Math.floor(Math.random() * pool.length)];
  // 素材时长普遍 2~4s；给 1.5 倍余量，onEnded 正常触发时不会走到这里
  const ms = kind === 'click' ? 2600 : 4200;
  return { seq, kind, action, text, ms };
}

/** 核心态 → 动作候选；idle 用 tick 交替，避免长时间定格同一个循环动作 */
const CORE_ACTIONS: Record<PetCoreState, string[]> = {
  idle: ['idle-breathe', 'idle-float'],
  sleepy: ['idle-nap'],
  thinking: ['work-think'],
  working: ['work-busy'],
  sad: ['work-sad'],
};

export function coreAction(state: PetCoreState, tick: number): string {
  const pool = CORE_ACTIONS[state].filter((id) => ACTIONS[id]);
  if (pool.length === 0) return 'idle-breathe';
  return pool[tick % pool.length];
}

/** 动作是否循环播放（来自 manifest，唯一事实来源） */
export function isLoopAction(id: string): boolean {
  return ACTIONS[id]?.loop ?? true;
}

/** 舞台文案用中文名（诊断/调试时能直接看出播的是哪个动作） */
export function actionLabel(id: string): string {
  return ACTIONS[id]?.label ?? id;
}
