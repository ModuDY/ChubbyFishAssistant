/** 记忆亮灯纯函数单测：序列生成 / 按键判定 / 节奏 / 动作映射 */
import { describe, expect, it } from 'vitest';
import {
  SHOWOFF_AT,
  SIMON_PADS,
  extendSequence,
  judgePress,
  nextPad,
  reactionFor,
  showTiming,
  type SimonPad,
} from './simon';

describe('nextPad —— 序列生成', () => {
  it('只返回四个合法灯位', () => {
    for (let i = 0; i < 60; i += 1) {
      expect(SIMON_PADS).toContain(nextPad([]));
    }
  });

  it('允许相邻重复（两个相同是正常序列）', () => {
    expect(nextPad([0], () => 0)).toBe(0);
  });

  it('避免连续三个相同：第三个会被顺延', () => {
    expect(nextPad([0, 0], () => 0)).toBe(1);
  });

  it('rand 返回 1 时不越界', () => {
    expect(SIMON_PADS).toContain(nextPad([], () => 1));
  });
});

describe('extendSequence —— 成功一轮后加长', () => {
  it('返回新数组、原数组不变', () => {
    const seq: SimonPad[] = [1, 2];
    const next = extendSequence(seq, () => 0.9);
    expect(seq).toEqual([1, 2]);
    expect(next).toEqual([1, 2, 3]);
  });
});

describe('judgePress —— 按键判定', () => {
  it('按对返回 ok，按错返回 wrong', () => {
    expect(judgePress([1, 2, 3], 0, 1)).toBe('ok');
    expect(judgePress([1, 2, 3], 1, 1)).toBe('wrong');
    expect(judgePress([1, 2, 3], 2, 3)).toBe('ok');
  });
});

describe('showTiming —— 节奏随长度收紧', () => {
  it('越长越快', () => {
    const short = showTiming(2);
    const long = showTiming(8);
    expect(long.on).toBeLessThan(short.on);
    expect(long.gap).toBeLessThan(short.gap);
  });

  it('有下限，不会快到看不清', () => {
    const t = showTiming(50);
    expect(t.on).toBe(300);
    expect(t.gap).toBe(120);
  });
});

describe('reactionFor —— 事件到动作的联动', () => {
  it('按错 → 难过', () => {
    expect(reactionFor('miss', 3)).toEqual({ action: 'work-sad', linePool: 'sad' });
  });

  it('普通通过 → 开心', () => {
    expect(reactionFor('clear', 3)).toEqual({ action: 'click-happy', linePool: 'happy' });
  });

  it('刷新记录 → 庆祝', () => {
    expect(reactionFor('record', SHOWOFF_AT - 1)).toEqual({
      action: 'work-celebrate',
      linePool: 'happy',
    });
  });

  it(`纪录达到 ${SHOWOFF_AT} → 得意表演`, () => {
    expect(reactionFor('record', SHOWOFF_AT)).toEqual({
      action: 'talent-violin',
      linePool: 'talent',
    });
  });
});
