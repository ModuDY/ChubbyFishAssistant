/** 石头剪刀布纯函数的单测：判定 / 记分 / 连击 / 反应动作 / 台词池 */
import { describe, expect, it } from 'vitest';
import {
  RPS_MOVES,
  applyOutcome,
  isValidMove,
  judgeRound,
  linePoolForOutcome,
  petMove,
  reactionAction,
  updateStreak,
  STREAK_LEVEL_AT,
  type RpsMove,
  type RpsStreak,
} from './games';

describe('judgeRound —— 单轮判定（pet 视角）', () => {
  it('克制关系正确', () => {
    expect(judgeRound('rock', 'scissors')).toBe('win');
    expect(judgeRound('paper', 'rock')).toBe('win');
    expect(judgeRound('scissors', 'paper')).toBe('win');
    expect(judgeRound('scissors', 'rock')).toBe('lose');
    expect(judgeRound('rock', 'paper')).toBe('lose');
    expect(judgeRound('paper', 'scissors')).toBe('lose');
  });

  it('同招平局', () => {
    for (const m of RPS_MOVES) {
      expect(judgeRound(m, m)).toBe('draw');
    }
  });
});

describe('applyOutcome —— 记分', () => {
  it('赢加分 pet 侧，输加分 player 侧，平局不动', () => {
    let s = { pet: 0, player: 0 };
    s = applyOutcome(s, 'win');
    s = applyOutcome(s, 'win');
    s = applyOutcome(s, 'lose');
    s = applyOutcome(s, 'draw');
    expect(s).toEqual({ pet: 2, player: 1 });
  });

  it('返回新对象不改旧值（React 状态更新约定）', () => {
    const prev = { pet: 1, player: 0 };
    const next = applyOutcome(prev, 'lose');
    expect(prev).toEqual({ pet: 1, player: 0 });
    expect(next).toEqual({ pet: 1, player: 1 });
  });
});

describe('updateStreak —— 连续计数', () => {
  it('同侧累计', () => {
    let st: RpsStreak = { kind: null, count: 0 };
    st = updateStreak(st, 'win');
    st = updateStreak(st, 'win');
    expect(st).toEqual({ kind: 'win', count: 2 });
  });

  it('异侧清零重计', () => {
    let st: RpsStreak = { kind: 'win', count: 2 };
    st = updateStreak(st, 'lose');
    expect(st).toEqual({ kind: 'lose', count: 1 });
  });

  it('平局清零（打断连胜）', () => {
    let st: RpsStreak = { kind: 'win', count: 2 };
    st = updateStreak(st, 'draw');
    expect(st).toEqual({ kind: null, count: 0 });
  });
});

describe('reactionAction —— 结果 → 动作联动', () => {
  it('单轮：赢庆祝 / 输难过 / 平害羞', () => {
    expect(reactionAction('win', { kind: null, count: 0 })).toBe('work-celebrate');
    expect(reactionAction('lose', { kind: null, count: 0 })).toBe('work-sad');
    expect(reactionAction('draw', { kind: null, count: 0 })).toBe('click-shy');
  });

  it(`pet 连败 ${STREAK_LEVEL_AT} 局 → 炸毛（玩家连胜）`, () => {
    let st: RpsStreak = { kind: null, count: 0 };
    for (let i = 0; i < STREAK_LEVEL_AT; i += 1) st = updateStreak(st, 'lose');
    expect(st.count).toBe(STREAK_LEVEL_AT);
    expect(reactionAction('lose', st)).toBe('click-tsundere');
  });

  it(`pet 连胜 ${STREAK_LEVEL_AT} 局 → 得意表演`, () => {
    let st: RpsStreak = { kind: null, count: 0 };
    for (let i = 0; i < STREAK_LEVEL_AT; i += 1) st = updateStreak(st, 'win');
    expect(reactionAction('win', st)).toBe('talent-violin');
  });

  it('连败 2 局还只是难过（未到阈值不提前升级）', () => {
    expect(reactionAction('lose', { kind: 'lose', count: STREAK_LEVEL_AT - 1 })).toBe('work-sad');
  });

  it('连败中断后重新计数', () => {
    expect(reactionAction('lose', { kind: 'lose', count: STREAK_LEVEL_AT, })).toBe('click-tsundere');
    // 中间平局清零后只剩 1 败 → 普通难过
    expect(reactionAction('lose', { kind: 'lose', count: 1 })).toBe('work-sad');
  });
});

describe('linePoolForOutcome —— 台词池映射', () => {
  it('连败炸毛走 tease，连胜得意走 talent', () => {
    expect(linePoolForOutcome('lose', { kind: 'lose', count: STREAK_LEVEL_AT })).toBe('tease');
    expect(linePoolForOutcome('win', { kind: 'win', count: STREAK_LEVEL_AT })).toBe('talent');
  });

  it('单轮结果走对应情绪池', () => {
    expect(linePoolForOutcome('win', { kind: 'win', count: 1 })).toBe('happy');
    expect(linePoolForOutcome('lose', { kind: 'lose', count: 1 })).toBe('sad');
    expect(linePoolForOutcome('draw', { kind: null, count: 0 })).toBe('click');
  });
});

describe('出招工具', () => {
  it('petMove 只返回合法招式', () => {
    for (let i = 0; i < 60; i += 1) {
      expect(RPS_MOVES).toContain(petMove());
    }
  });

  it('isValidMove 拦外部输入', () => {
    expect(isValidMove('rock')).toBe(true);
    expect(isValidMove('fist')).toBe(false);
    expect(isValidMove(42 as unknown as RpsMove)).toBe(false);
    expect(isValidMove(null)).toBe(false);
  });
});
