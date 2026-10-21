/**
 * 动作语义池（**单一事实来源**）
 *
 * 为什么单独一层：106 个动作原来被分散在 4 个文件里引用（`petState.CORE_ACTIONS` /
 * `petState.TRANSIENT_ACTIONS` / `flavor.FLAVOR_BUCKETS` / `festivals.*.actions`），
 * 结果「谁在播什么、哪个动作永远不会被播到」无从核对，2026-10-20 前有 50+ 个动作
 * 因为没有接线而一直吃灰（详见 `coverage.ts`）。现在所有池子在这里集中声明：
 *
 * - 新增动作 → 只是往某个池子里加一个 id；
 * - 想核对覆盖 → `coverage.ts` 汇总全部池子与 manifest 比对（单测强制）。
 *
 * 本模块**零副作用**（不 import actions/PetSprite/Tauri），因此可以被单测直接读取，
 * 也可以被 `petState` / `flavor` / `festivals` / `PetRoot` 共同引用而不担心循环依赖。
 */
import type { PetLinePool } from './lines';

/* ------------------------------- 分组素材 ------------------------------- */

/** 吃的（饮食起居里"动嘴"的那批） */
export const FOOD_ACTIONS = [
  'life-rice',
  'life-snack-big',
  'life-snack-caught',
  'life-icecream-melt',
  'life-tanghulu',
  'life-watermelon',
  'life-crab',
  'life-longevity-noodles',
  'life-hotpot',
];

/** 玩的（有道具/有输赢的那批） */
export const PLAY_ACTIONS = [
  'life-swing',
  'life-hobbyhorse',
  'life-jianzi',
  'life-spinning-top',
  'life-gomoku',
  'life-rubiks',
  'life-toy-car',
  'life-watergun',
  'life-balloon',
];

/** 生活杂事（照镜子 / 摇扇 / 堆雪人 / 拆礼物 / 气急败坏 / 被吓一跳） */
export const LIFE_MISC_ACTIONS = [
  'life-mirror',
  'life-fan',
  'life-snowman',
  'life-gift',
  'life-game-rage',
  'life-scared',
];

/** 才艺 / 表演 */
export const TALENT_ACTIONS = [
  'talent-juggling',
  'talent-maid-dance',
  'talent-otaku-dance',
  'talent-maid-curtsy',
  'talent-violin',
  'talent-flute',
  'talent-humming',
  'talent-card-magic',
  'talent-dove-magic',
  'talent-flower-magic',
  'talent-coding',
  'talent-desk-tap',
  'talent-swing-dance',
  'talent-quick-note',
  'talent-crab-walk',
  'talent-eat-token',
];

/** 特效 / 奇观（不含两个 `loop:true` 展示类，见 SHOWCASE_ACTIONS） */
export const SFX_ACTIONS = [
  'sfx-animals-ring',
  'sfx-tail-slap',
  'sfx-cute-ghost',
  'sfx-whale-rise',
  'sfx-butterfly-bee-bloom',
  'sfx-autumn-leaves',
  'sfx-bubbles',
];

/**
 * 展示类（`manifest.loop = true`）：循环动作放进瞬态会被兜底时长硬切，
 * 因此**永不进任何随机池**，只接受右键菜单手动点播。
 */
export const SHOWCASE_ACTIONS = ['sfx-spin-showcase', 'sfx-outfit-showcase'];

/** 「被撸」的可爱反应（悬停抚摸 / 主动互动） */
export const CUTE_ACTIONS = ['life-cat', 'click-tickle'];

/** 拖拽松手时「被甩出去吓一跳」的候选（低频彩蛋，见 `interaction.ts`） */
export const STARTLE_ACTIONS = ['life-scared'];

/* ------------------------------- 核心态 ------------------------------- */

/**
 * 核心态 → 循环动作池（必须都是 `loop:true` 素材，否则会定格在最后一帧）。
 * `idle` 用 tick 交替，其余池子长度 1~2 时同样按 tick 交替——长时间定格一个
 * 循环动作会显得死板（这是 `IDLE_SWAP_MS` 存在的原始理由）。
 */
export const CORE_POOLS = {
  idle: ['idle-breathe', 'idle-float'],
  sleepy: ['idle-nap'],
  thinking: ['work-think'],
  /**
   * working 加 `work-pace`（原地踱步）：签到/网关联动动辄跑几十秒，
   * 一直「忙碌点按」同一段循环最容易看出是假动画。
   */
  working: ['work-busy', 'work-pace'],
  sad: ['work-sad'],
} as const;

/* ------------------------------- 瞬态池 ------------------------------- */

/**
 * 瞬态语义 → 动作候选。
 *
 * 新增 5 个语义（2026-10-20）把「用户主动互动」和「时间彩蛋」分开：
 * `feed` / `play` / `perform` / `pet` / `tease` 只在用户交互时出现，
 * 不会混进空闲随机池（否则彩蛋里全是吃喝玩乐，反而不像"助手"）。
 */
export const TRANSIENT_POOLS = {
  happy: ['work-celebrate', 'click-happy'],
  rich: ['balance-jingle', 'balance-full', 'balance-count'],
  broke: ['balance-broke', 'balance-empty'],
  click: ['click-wave', 'click-happy', 'click-tickle', 'click-tsundere'],
  idle: ['idle-look', 'mumble-daze', 'mumble-deep'],
  pace: ['work-pace'],
  archive: ['work-archive'],
  neutral: ['balance-normal'],
  turn: ['turn-run'],
  jump: ['jump-grab'],
  /**
   * working 长时间无进度：三个碎碎念轮换（原来只播 `mumble-screen`，
   * 另两个 `mumble-table-wipe` / `mumble-what-to-eat` 只挂在彩蛋桶里）。
   */
  mumble: ['mumble-screen', 'mumble-table-wipe', 'mumble-what-to-eat'],
  /** 用户主动「喂点吃的」 */
  feed: FOOD_ACTIONS,
  /** 用户主动「陪它玩会儿」 */
  play: PLAY_ACTIONS,
  /** 用户主动「露一手」 */
  perform: TALENT_ACTIONS,
  /** 悬停抚摸 / 主动互动 */
  pet: CUTE_ACTIONS,
  /** 连点过头（傲娇生气） */
  tease: ['click-tsundere'],
  /**
   * 节日：调用方**总是**传 override（当天轮播的具体动作），
   * 这里的池只是兜底（万一有人忘了传，至少不会播成"打招呼"）。
   */
  festival: ['festival-spring-fireworks'],
} as const;

/* --------------------------- 动作 → 台词池 --------------------------- */

/**
 * 让动作说「对得上」的话：吃西瓜时冒「水面很平静」是明显的穿帮。
 * 先按前缀归类，再对少数语义特殊者做精确覆盖。
 */
const EXACT_LINE_POOL: Record<string, PetLinePool> = {
  'work-celebrate': 'happy',
  'work-archive': 'working',
  'work-sad': 'sad',
  'balance-broke': 'broke',
  'balance-empty': 'broke',
  'balance-count': 'rich',
  'idle-nap': 'sleepy',
  'idle-yawn': 'sleepy',
  'idle-wake': 'click',
  'idle-squat': 'click',
  'click-tsundere': 'tease',
  // 害羞是被放下时的反应，不是"被戳烦了"，用默认点击台词更贴（tease 池是"别戳了"）
  'click-shy': 'click',
  'drag-dangle': 'click',
  'turn-run': 'working',
  'jump-grab': 'happy',
};

const PREFIX_LINE_POOL: Array<[string, PetLinePool]> = [
  ['festival-', 'festival'],
  ['life-', 'life'],
  ['talent-', 'talent'],
  ['sfx-', 'sfx'],
  ['mumble-', 'mumble'],
  ['balance-', 'rich'],
  ['click-', 'click'],
  ['work-', 'working'],
  ['idle-', 'idle'],
];

/** 动作 id → 台词池（未知动作回落 idle，保证任何素材都能说句话） */
export function linePoolForAction(id: string): PetLinePool {
  return EXACT_LINE_POOL[id] ?? PREFIX_LINE_POOL.find(([p]) => id.startsWith(p))?.[1] ?? 'idle';
}

/** 从池子里随机取一个（空池返回 undefined，让调用方回落到默认动作） */
export function pickFromPool(
  pool: readonly string[],
  rand: () => number = Math.random,
): string | undefined {
  if (pool.length === 0) return undefined;
  return pool[Math.min(pool.length - 1, Math.floor(rand() * pool.length))];
}
