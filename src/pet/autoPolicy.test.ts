import { describe, expect, it } from 'vitest';
import {
  PetAutoPolicy,
  canAutoSpeak,
  dateKey,
  pickGreeting,
  rollover,
  slotEventKey,
  type AutoPolicyState,
} from './autoPolicy';

function memoryStorage() {
  const map = new Map<string, string>();
  return {
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
  };
}

const DAY = new Date(2026, 9, 22, 9, 0, 0);

describe('rollover', () => {
  it('同一天保持原状态，跨天重置计数 / 静音 / 去重集合', () => {
    const state: AutoPolicyState = {
      date: '2026-10-21',
      count: 3,
      silencedDate: '2026-10-21',
      seen: ['2026-10-21#greet-morning'],
    };
    expect(rollover(state, '2026-10-21')).toBe(state);
    expect(rollover(state, '2026-10-22')).toEqual({
      date: '2026-10-22',
      count: 0,
      silencedDate: null,
      seen: [],
    });
  });
});

describe('canAutoSpeak', () => {
  const state: AutoPolicyState = { date: '2026-10-22', count: 2, silencedDate: null, seen: [] };

  it('配额内允许，用尽后停止', () => {
    expect(canAutoSpeak(state, '2026-10-22', 4, 9)).toBe(true);
    expect(canAutoSpeak({ ...state, count: 4 }, '2026-10-22', 4, 9)).toBe(false);
    expect(canAutoSpeak(state, '2026-10-22', 0, 9)).toBe(false);
  });

  it('当日已静音时不允许', () => {
    expect(canAutoSpeak({ ...state, silencedDate: '2026-10-22' }, '2026-10-22', 4, 9)).toBe(false);
    // 昨天的静音不影响今天
    expect(canAutoSpeak({ ...state, silencedDate: '2026-10-21' }, '2026-10-22', 4, 9)).toBe(true);
  });

  it('深夜 23–5 点不允许', () => {
    expect(canAutoSpeak(state, '2026-10-22', 4, 23)).toBe(false);
    expect(canAutoSpeak(state, '2026-10-22', 4, 4)).toBe(false);
    expect(canAutoSpeak(state, '2026-10-22', 4, 5)).toBe(true);
  });
});

describe('PetAutoPolicy', () => {
  it('计数 / 静音 / 去重跨实例持久化（模拟重启）', () => {
    const storage = memoryStorage();
    const p1 = new PetAutoPolicy(storage, () => DAY);
    expect(p1.canSpeak(4)).toBe(true);
    p1.noteSpoken();
    p1.markSeen(slotEventKey('2026-10-22', 'morning'));
    expect(p1.count()).toBe(1);

    const p2 = new PetAutoPolicy(storage, () => DAY);
    expect(p2.count()).toBe(1);
    expect(p2.hasSeen(slotEventKey('2026-10-22', 'morning'))).toBe(true);
    expect(p2.silencedToday()).toBe(false);
  });

  it('「今天别主动说话」设置与恢复', () => {
    const storage = memoryStorage();
    const p = new PetAutoPolicy(storage, () => DAY);
    p.setSilenced(true);
    expect(p.silencedToday()).toBe(true);
    expect(p.canSpeak(4)).toBe(false);

    const restarted = new PetAutoPolicy(storage, () => DAY);
    expect(restarted.silencedToday()).toBe(true);
    restarted.setSilenced(false);
    expect(restarted.canSpeak(4)).toBe(true);
  });

  it('跨天后静音与计数自动解除', () => {
    const storage = memoryStorage();
    let now = DAY;
    const p = new PetAutoPolicy(storage, () => now);
    p.setSilenced(true);
    p.noteSpoken();
    now = new Date(2026, 9, 23, 9, 0, 0);
    expect(p.silencedToday()).toBe(false);
    expect(p.count()).toBe(0);
    expect(p.canSpeak(4)).toBe(true);
  });

  it('存储内容损坏时按全新一天处理，不会永久闭嘴', () => {
    const storage = memoryStorage();
    storage.setItem('pet-auto-policy-v1', '{not json');
    const p = new PetAutoPolicy(storage, () => DAY);
    expect(p.count()).toBe(0);
    expect(p.silencedToday()).toBe(false);
    expect(p.canSpeak(4)).toBe(true);
  });
});

describe('时段问候', () => {
  it('三个时段都有短句，且不出现具体钟点', () => {
    for (const slot of ['morning', 'noon', 'evening'] as const) {
      const line = pickGreeting(slot, () => 0);
      expect(line.length).toBeGreaterThan(0);
      expect(line).not.toMatch(/\d+\s*点/);
    }
  });

  it('去重键包含日期与时段', () => {
    expect(slotEventKey('2026-10-22', 'noon')).toBe('2026-10-22#greet-noon');
  });

  it('dateKey 使用本地日期', () => {
    expect(dateKey(new Date(2026, 0, 2, 23, 30))).toBe('2026-01-02');
  });
});
