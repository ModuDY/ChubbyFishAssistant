import { describe, expect, it } from 'vitest';

import {
  FLAVOR_BUCKETS,
  RECENT_MEMORY,
  SLOT_ACTIONS,
  pickFlavorAction,
  pickSlotAction,
  pushRecent,
  timeSlot,
} from './flavor';

/** 固定随机源：始终取第一个（让断言可预期） */
const first = () => 0;

describe('timeSlot —— 时段边界', () => {
  it('早 5–10 / 午 11–13 / 晚 17–20，其余为 null', () => {
    expect(timeSlot(4)).toBeNull();
    expect(timeSlot(5)).toBe('morning');
    expect(timeSlot(10)).toBe('morning');
    expect(timeSlot(11)).toBe('noon');
    expect(timeSlot(13)).toBe('noon');
    expect(timeSlot(14)).toBeNull();
    expect(timeSlot(16)).toBeNull();
    expect(timeSlot(17)).toBe('evening');
    expect(timeSlot(20)).toBe('evening');
    expect(timeSlot(21)).toBeNull();
    expect(timeSlot(0)).toBeNull();
  });
});

describe('pickSlotAction', () => {
  it('返回该时段的候选动作', () => {
    expect(SLOT_ACTIONS.morning).toContain(pickSlotAction('morning', first));
    expect(SLOT_ACTIONS.noon).toContain(pickSlotAction('noon', first));
    expect(SLOT_ACTIONS.evening).toContain(pickSlotAction('evening', first));
  });

  it('时段动作齐全（晨间/早/午/晚各就位）', () => {
    expect(SLOT_ACTIONS.morning).toEqual(['life-brushing', 'life-breakfast']);
    expect(SLOT_ACTIONS.noon).toEqual(['life-lunch']);
    expect(SLOT_ACTIONS.evening).toEqual(['life-dinner']);
  });
});

describe('pickFlavorAction —— 分桶轮转', () => {
  it('连续 N 轮会依次轮到 N 个不同的桶', () => {
    const picks = FLAVOR_BUCKETS.map((_, tick) => pickFlavorAction(tick, [], first));
    // 每轮取的是不同桶的第一个动作 → 结果互不相同
    expect(new Set(picks).size).toBe(FLAVOR_BUCKETS.length);
    FLAVOR_BUCKETS.forEach((bucket, i) => expect(picks[i]).toBe(bucket[0]));
  });

  it('轮次回绕（第 N+1 轮回到第 1 个桶）', () => {
    const wrap = FLAVOR_BUCKETS.length;
    expect(pickFlavorAction(wrap, [], first)).toBe(pickFlavorAction(0, [], first));
  });

  it('负数轮次也能安全取模', () => {
    expect(FLAVOR_BUCKETS.flat()).toContain(pickFlavorAction(-1, [], first));
  });

  it('优先避开最近播过的动作', () => {
    const bucket = FLAVOR_BUCKETS[0];
    // 把桶内除最后一个之外全部标记为「最近播过」
    const recent = bucket.slice(0, -1);
    expect(pickFlavorAction(0, recent, first)).toBe(bucket[bucket.length - 1]);
  });

  it('桶内全部播过时退化为全桶随机且不抛错', () => {
    const bucket = FLAVOR_BUCKETS[0];
    expect(bucket).toContain(pickFlavorAction(0, [...bucket], first));
  });

  it('返回的 id 一定在池内', () => {
    const all = FLAVOR_BUCKETS.flat();
    for (let tick = 0; tick < 12; tick += 1) {
      expect(all).toContain(pickFlavorAction(tick, [], () => Math.random()));
    }
  });
});

describe('pushRecent', () => {
  it('新动作进队首、去重、长度封顶', () => {
    expect(pushRecent([], 'a')).toEqual(['a']);
    expect(pushRecent(['a', 'b'], 'a')).toEqual(['a', 'b']);
    let list: string[] = [];
    for (let i = 0; i < RECENT_MEMORY + 5; i += 1) list = pushRecent(list, `x${i}`);
    expect(list.length).toBe(RECENT_MEMORY);
    expect(list[0]).toBe(`x${RECENT_MEMORY + 4}`);
  });
});

describe('随机池卫生（防回归）', () => {
  const all = FLAVOR_BUCKETS.flat();

  it('池内 id 不重复', () => {
    expect(new Set(all).size).toBe(all.length);
  });

  it('不含节日类动作（节日由日期触发，避免「七月放烟花」）', () => {
    expect(all.filter((id) => id.startsWith('festival-'))).toEqual([]);
  });

  it('不含时段类动作（避免与时段触发语义重复）', () => {
    const slotIds = Object.values(SLOT_ACTIONS).flat();
    expect(all.filter((id) => slotIds.includes(id))).toEqual([]);
  });

  it('不含展示类循环动作（会被瞬态兜底时长硬切）', () => {
    expect(all).not.toContain('sfx-spin-showcase');
    expect(all).not.toContain('sfx-outfit-showcase');
  });

  it('每个桶都非空（保证轮转有意义）', () => {
    for (const bucket of FLAVOR_BUCKETS) expect(bucket.length).toBeGreaterThan(0);
  });
});
