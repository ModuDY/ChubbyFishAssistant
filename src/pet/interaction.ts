/**
 * 交互语义（用户主动操作 → 该播什么）
 *
 * 原来点击只有一条路：`playClick()` → 从 5 个 click 动作里随机一个，连点也不计数。
 * 结果是 106 个动作里 50+ 个只能靠"时间彩蛋"偶尔漂出来，用户**主动**点不到它们，
 * 互动场景很单薄（`click-tsundere` 的设计意图「高频连点（防刷）」也从未实现）。
 *
 * 这里把**用户手势**翻译成明确的互动语义，纯函数、可单测：
 *
 * | 手势                    | 语义           | 动作来源              |
 * |------------------------|---------------|----------------------|
 * | 单击（1 次）             | 打个招呼       | `click` 池           |
 * | 快速连点（≥3 次 / 3s）   | 被戳烦了       | `click-tsundere`     |
 * | 连点硬上限（≥6 次 / 5s） | 有点炸毛       | `click-tsundere` + 台词变凶 |
 * | 悬停不点（≥1.2s）        | 被抚摸         | `pet` 池（撸猫 / 挠痒）|
 * | 拖拽松手（正常）         | 落地反馈       | `click-shy` / `idle-squat` |
 * | 拖拽松手（甩得快）       | 被甩出去吓一跳 | `STARTLE_ACTIONS`     |
 * | 右键菜单「喂点吃的」     | 投喂           | `feed` 池（9 种吃的） |
 * | 右键菜单「陪我玩」       | 陪玩           | `play` 池（9 种玩法） |
 * | 右键菜单「露一手」       | 才艺表演       | `perform` 池（16 个） |
 *
 * 关键点：**同一类动作不会被两条路径重复占用**（喂食只会从 FOOD 池取，
 * 连点只会出傲娇），所以设备端看到的动作与用户做的事始终对得上。
 */
import { STARTLE_ACTIONS } from './pools';
import type { PetTransientKind } from './petState';

/* ------------------------------- 连点判定 ------------------------------- */

/** 连点计数的滚动窗口（ms）：窗口内的点击次数决定"烦不烦" */
export const CLICK_WINDOW_MS = 3000;
/** 窗口内达到该次数即视为「戳烦了」（第 3 次点击开始变脸） */
export const CLICK_TEASE_AT = 3;
/** 硬上限：窗口内达到该次数则强制傲娇（避免无限连点把动作池摇干） */
export const CLICK_LIMIT_AT = 6;

export type ClickMood = 'normal' | 'tease' | 'annoyed';

/**
 * 记录一次点击并返回当前"心情档"。
 *
 * 用滚动窗口而不是累计计数：累计计数在用户停手后不会消退，
 * 两小时后随便点一下还是"炸毛"，手感很怪。
 *
 * @param history 历史点击时间戳（就地裁掉窗口外的旧记录并返回新数组）
 * @param now     当前时间戳（测试可注入）
 */
export function trackClick(
  history: number[],
  now: number = Date.now(),
): { history: number[]; mood: ClickMood; count: number } {
  const kept = history.filter((t) => now - t < CLICK_WINDOW_MS);
  kept.push(now);
  const count = kept.length;
  const mood: ClickMood = count >= CLICK_LIMIT_AT ? 'annoyed' : count >= CLICK_TEASE_AT ? 'tease' : 'normal';
  return { history: kept, mood, count };
}

/** 该心情档对应的瞬态语义（normal 走 click 池，其余固定傲娇动作） */
export function clickKindFor(mood: ClickMood): PetTransientKind {
  return mood === 'normal' ? 'click' : 'tease';
}

/* ------------------------------- 抚摸判定 ------------------------------- */

/** 悬停多久算「摸了一下」（ms）：太短会误触，太长用户以为没反应 */
export const HOVER_PET_MS = 1200;
/** 两次抚摸的最小间隔（ms）：连续悬停不要刷屏 */
export const HOVER_COOLDOWN_MS = 20_000;

/**
 * 悬停是否应该触发「被抚摸」。
 *
 * 护栏：正在拖拽、正在播瞬态、冷却未过都不触发——否则用户移动鼠标经过角色
 * 会把正在播的签到反馈冲掉（这是"动作互相打断"最烦人的一种）。
 *
 * @param hovered  鼠标当前是否停在角色热区内
 * @param hoverMs  已悬停时长（ms）
 * @param lastPetAt 上次抚摸的时间戳（0 = 从未）
 * @param busy      是否正忙（拖拽中 / 正在播瞬态 / 窗口不可见）
 */
export function shouldPetOnHover(
  hovered: boolean,
  hoverMs: number,
  lastPetAt: number,
  busy: boolean,
  now: number = Date.now(),
): boolean {
  if (!hovered || busy) return false;
  if (hoverMs < HOVER_PET_MS) return false;
  return now - lastPetAt >= HOVER_COOLDOWN_MS;
}

/* ------------------------------- 拖拽落地 ------------------------------- */

/** 快速甩动的速度阈值（px/单次 pointermove 累计的 px/s）：超过即"被甩出去" */
export const STARTLE_SPEED = 2600;
/** 惊吓彩蛋的出现概率（其余按原落地反馈走） */
export const STARTLE_CHANCE = 0.25;

/** 拖拽落地反馈候选（正常松手） */
export const LANDING_ACTIONS = ['click-shy', 'idle-squat'];

/**
 * 松手时该播什么。
 *
 * @param speedPxPerSec 松手前一刻的速度（父组件按位移/时间算）
 * @param rand          随机源
 */
export function pickLandingAction(
  speedPxPerSec: number,
  rand: () => number = Math.random,
): string {
  if (speedPxPerSec >= STARTLE_SPEED && STARTLE_ACTIONS.length > 0 && rand() < STARTLE_CHANCE) {
    return STARTLE_ACTIONS[0];
  }
  const i = Math.min(LANDING_ACTIONS.length - 1, Math.floor(rand() * LANDING_ACTIONS.length));
  return LANDING_ACTIONS[i];
}

/* ------------------------------ 右键互动菜单 ------------------------------ */

/** 主动互动的入口定义（右键菜单渲染 + 快捷键共用同一份，避免文案漂移） */
export const INTERACTIONS: Array<{
  id: 'feed' | 'play' | 'perform';
  label: string;
  kind: PetTransientKind;
}> = [
  { id: 'feed', label: '喂点吃的', kind: 'feed' },
  { id: 'play', label: '陪我玩会儿', kind: 'play' },
  { id: 'perform', label: '露一手', kind: 'perform' },
];
