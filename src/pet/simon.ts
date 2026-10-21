/**
 * 桌宠互动小游戏：记忆亮灯 Simon（P10，纯函数层）
 *
 * 与 `games.ts` 同一分层：序列生成、按键判定、亮灯节奏、动作映射都是可单测的
 * 纯逻辑，组件只负责画灯阵与跑定时器。判定全本地、零 token——AI 只在每轮结束后
 * 由 `PetRoot` 事后点评，绝不参与"灯亮了哪个"。
 *
 * 为什么第二个游戏选它：右侧留白只有 216px，2×2 四个色块是能放下的最大可点面积；
 * 不需要秒级计时器（打地鼠/接球要），错一次就结束，回合边界天然清晰，
 * 正好给「每轮一句解说」提供触发点。
 */

/** 四个灯位（0 左上 / 1 右上 / 2 左下 / 3 右下） */
export type SimonPad = 0 | 1 | 2 | 3;

export const SIMON_PADS: readonly SimonPad[] = [0, 1, 2, 3];

/**
 * 序列追加一位。
 *
 * 连续三个相同会让玩家以为"刚才是不是没亮"，只重掷一次（不做循环重试，
 * 保持可预测的调用次数——单测用固定 rand 就能锁定行为）。
 */
export function nextPad(
  prev: readonly SimonPad[],
  rand: () => number = Math.random,
): SimonPad {
  const idx = Math.min(SIMON_PADS.length - 1, Math.floor(rand() * SIMON_PADS.length));
  const last = prev[prev.length - 1];
  const lastTwo = prev[prev.length - 2];
  if (last !== undefined && last === lastTwo && SIMON_PADS[idx] === last) {
    return SIMON_PADS[(idx + 1) % SIMON_PADS.length];
  }
  return SIMON_PADS[idx];
}

/** 成功一轮后：在原序列尾部再亮一位（返回新数组，不改原值） */
export function extendSequence(
  seq: readonly SimonPad[],
  rand: () => number = Math.random,
): SimonPad[] {
  return [...seq, nextPad(seq, rand)];
}

/** 玩家按键与当前该按的那位是否一致 */
export function judgePress(
  seq: readonly SimonPad[],
  index: number,
  pad: SimonPad,
): 'ok' | 'wrong' {
  return seq[index] === pad ? 'ok' : 'wrong';
}

/**
 * 亮灯节奏：序列越长越快（否则第 10 位要亮 6 秒，玩家等到走神）。
 * 下限保证"看得清"，不是竞速游戏。
 */
export function showTiming(len: number): { on: number; gap: number } {
  const n = Math.max(1, len);
  return {
    on: Math.max(300, 640 - (n - 1) * 40),
    gap: Math.max(120, 240 - (n - 1) * 15),
  };
}

/** 一轮结束的事件：clear = 复现成功、record = 刷新最高、miss = 按错结束 */
export type SimonEvent = 'clear' | 'record' | 'miss';

export type SimonLinePool = 'happy' | 'sad' | 'click' | 'talent';

export type SimonReaction = { action: string; linePool: SimonLinePool };

/** 刷新纪录到这一长度时改用得意表演（与猜拳连胜 3 的阈值感一致） */
export const SHOWOFF_AT = 6;

/**
 * 事件 → 动作 + 台词池。动作 id 全部来自现有 manifest（`coverage.test` 会校验）。
 * 注意 `miss` 不追加炸毛：记忆游戏输在自己手滑，给 `work-sad` 比给 `click-tsundere` 贴。
 */
export function reactionFor(event: SimonEvent, best: number): SimonReaction {
  if (event === 'miss') return { action: 'work-sad', linePool: 'sad' };
  if (event === 'record' && best >= SHOWOFF_AT) {
    return { action: 'talent-violin', linePool: 'talent' };
  }
  if (event === 'record') return { action: 'work-celebrate', linePool: 'happy' };
  return { action: 'click-happy', linePool: 'happy' };
}

/** 一轮结果回传 PetRoot 的载荷（纯数据，不含 React 状态） */
export type SimonRoundPayload = {
  event: SimonEvent;
  /** 本次挑战的序列长度（clear/record = 已复现长度；miss = 挑战到第几个） */
  round: number;
  /** 更新后的最高纪录 */
  best: number;
  /** 按错的位置（1-based，仅 miss 有值） */
  wrongAt?: number;
};
