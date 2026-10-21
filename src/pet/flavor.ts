/**
 * 彩蛋调度（时段触发 + 空闲随机池）
 *
 * 上游 75 个彩蛋动作入库后，若只靠"随机挑一个"会让池子过大（50+），单个动作几乎不会被看到。
 * 这里做两件事把它变得可用：
 * 1. **时段触发**：早/午/晚各有对应动作（刷牙、早餐、午餐、晚餐），到点自然播一次；
 * 2. **分桶轮转**：把其余生活/才艺/特效动作按语义分成多个桶，每轮按轮次换桶再桶内随机，
 *    并避开最近播过的动作——保证每一类都能稳定轮到，而不是全靠运气。
 *
 * 分桶按**语义**而不是按 group 粗略切分（2026-10-20 重排）：
 * 原来 `life` 组的 25 个动作挤在同一个桶里，桶内纯随机 → 单个动作命中率 4%，
 * 玩家基本只会反复见到「吃白饭」。现在吃的 / 玩的 / 杂事分开轮，命中率翻倍且语义连贯。
 *
 * 本模块**只产出"该播哪个动作"**，何时播、怎么播由 `PetRoot` 的状态机决定（纯函数便于单测）。
 */
import {
  FOOD_ACTIONS,
  LIFE_MISC_ACTIONS,
  PLAY_ACTIONS,
  SFX_ACTIONS,
  TALENT_ACTIONS,
} from './pools';

/** 时段（无对应时段的动作走随机池） */
export type TimeSlot = 'morning' | 'noon' | 'evening';

/**
 * 深夜判定（23 点 ~ 次日 5 点）。
 *
 * 为什么要它：彩蛋池里"火锅 / 水枪 / 宅舞 / 放烟花"在凌晨三点漂出来非常出戏。
 * 深夜改走 `QUIET_ACTIONS`（照镜子 / 各档碎碎念 / 东张西望这类安静动作），
 * 既贴合作息、又让几个原本只挂在白天池里的动作多一条触发路径。
 */
export function isQuietHour(hour: number): boolean {
  return hour >= 23 || hour < 5;
}

/** 深夜安静池（均为低动态动作，不会在半夜突然放烟花） */
export const QUIET_ACTIONS: string[] = [
  'idle-look',
  'mumble-daze',
  'mumble-table-wipe',
  'mumble-what-to-eat',
  'life-mirror',
  'talent-humming',
];

/**
 * 时段判定：早 5–10 点 / 午 11–13 点 / 晚 17–20 点，其余返回 null。
 * 边界刻意错开（10 与 11、13 与 17 之间留空），避免刚过整点就连续触发两个时段。
 */
export function timeSlot(hour: number): TimeSlot | null {
  if (hour >= 5 && hour <= 10) return 'morning';
  if (hour >= 11 && hour <= 13) return 'noon';
  if (hour >= 17 && hour <= 20) return 'evening';
  return null;
}

/** 全部时段（覆盖率审计按它枚举；新增时段只需改这里一处） */
export const TIME_SLOT_ALL: TimeSlot[] = ['morning', 'noon', 'evening'];

/** 时段 → 动作候选（饮食起居类；这些动作**不进随机池**，避免语义重复） */
export const SLOT_ACTIONS: Record<TimeSlot, string[]> = {
  morning: ['life-brushing', 'life-breakfast'],
  noon: ['life-lunch'],
  evening: ['life-dinner'],
};

/**
 * 空闲随机池的分桶（轮转，长度不固定）。
 *
 * 刻意排除三类动作：
 * - **节日类**（`festival-*`）：由 `festivals.ts` 按日期触发，塞进随机池会让「万圣南瓜」在七月出现，
 *   破坏"应景"意图；它们仍可在右键菜单手动点播；
 * - **时段类**（刷牙/早/午/晚餐）：见 `SLOT_ACTIONS`；
 * - **展示类**（`sfx-spin-showcase` / `sfx-outfit-showcase`，loop=true）：循环动作进瞬态会被兜底时长硬切，
 *   同样只留手动点播。
 */
export const FLAVOR_BUCKETS: string[][] = [
  // 桶 0：经典空闲反应（东张西望 / 各档碎碎念）
  ['idle-look', 'mumble-daze', 'mumble-deep', 'mumble-table-wipe', 'mumble-what-to-eat'],
  // 桶 1：吃的
  FOOD_ACTIONS,
  // 桶 2：玩的
  PLAY_ACTIONS,
  // 桶 3：生活杂事
  LIFE_MISC_ACTIONS,
  // 桶 4：才艺 / 表演
  TALENT_ACTIONS,
  // 桶 5：特效 / 奇观
  SFX_ACTIONS,
];

/** 「最近播过」的默认记忆长度（避免短时间内重复） */
export const RECENT_MEMORY = 8;

/**
 * 按轮次选桶 + 桶内随机，并尽量避开最近播过的动作。
 *
 * @param bucketTick 轮次计数（每次触发 +1）；用它轮换桶，保证每类都会被轮到
 * @param recent     最近播过的动作 id（新→旧）
 * @param rand       随机源（默认 Math.random，测试可注入）
 */
export function pickFlavorAction(
  bucketTick: number,
  recent: string[] = [],
  rand: () => number = Math.random,
  /** 深夜时只从安静池取（见 isQuietHour），保证凌晨不出戏 */
  quiet = false,
): string {
  const buckets = quiet ? [QUIET_ACTIONS] : FLAVOR_BUCKETS;
  const n = buckets.length;
  const i = ((bucketTick % n) + n) % n;
  const pool = buckets[i];
  if (pool.length === 0) return 'idle-look';

  const pickFrom = (list: string[]) => list[Math.min(list.length - 1, Math.floor(rand() * list.length))];
  const fresh = pool.filter((id) => !recent.includes(id));

  // 桶内还有没播过的就用没播过的；否则退化为全桶随机（最多重摇 3 次试图避开）
  if (fresh.length > 0) return pickFrom(fresh);
  for (let n2 = 0; n2 < 3; n2 += 1) {
    const id = pickFrom(pool);
    if (!recent.includes(id)) return id;
  }
  return pickFrom(pool);
}

/** 时段动作随机取一个（morning 有两个候选） */
export function pickSlotAction(slot: TimeSlot, rand: () => number = Math.random): string {
  const pool = SLOT_ACTIONS[slot];
  return pool[Math.min(pool.length - 1, Math.floor(rand() * pool.length))];
}

/** 把新播的动作压入"最近记忆"（保留最新 RECENT_MEMORY 条） */
export function pushRecent(recent: string[], id: string): string[] {
  const next = [id, ...recent.filter((x) => x !== id)];
  return next.slice(0, RECENT_MEMORY);
}
