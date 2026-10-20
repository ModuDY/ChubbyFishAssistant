import { describe, expect, it } from 'vitest';

import {
  SOLAR_RULES,
  getTodaysFestivalPlan,
  localDateKey,
  type FestivalRule,
} from './festivals';

/** 构造本地时区时间（避免 UTC 偏移把节日算到前后一天） */
const at = (y: number, m: number, d: number) => new Date(y, m - 1, d, 12, 0, 0);

describe('localDateKey', () => {
  it('按本地时区输出 YYYY-MM-DD', () => {
    expect(localDateKey(at(2026, 2, 17))).toBe('2026-02-17');
    expect(localDateKey(at(2026, 10, 5))).toBe('2026-10-05');
  });
});

describe('getTodaysFestivalPlan —— 农历节日（按年份表）', () => {
  it('2026 春节（正月初一）命中 02-17，并带春节动作', () => {
    const plan = getTodaysFestivalPlan(at(2026, 2, 17));
    expect(plan?.label).toBe('春节');
    expect(plan?.actions).toContain('festival-spring-fireworks');
    expect(plan?.key).toBe('2026-02-17#spring');
  });

  it('2026 元宵 03-03 / 中秋 09-25 / 端午 06-19 / 腊八 01-26 均命中', () => {
    expect(getTodaysFestivalPlan(at(2026, 3, 3))?.label).toBe('元宵节');
    expect(getTodaysFestivalPlan(at(2026, 9, 25))?.label).toBe('中秋节');
    expect(getTodaysFestivalPlan(at(2026, 6, 19))?.label).toBe('端午节');
    expect(getTodaysFestivalPlan(at(2026, 1, 26))?.label).toBe('腊八节');
  });

  it('节气类同样命中（清明 04-05 / 冬至 12-22）', () => {
    expect(getTodaysFestivalPlan(at(2026, 4, 5))?.label).toBe('清明节');
    expect(getTodaysFestivalPlan(at(2026, 12, 22))?.label).toBe('冬至');
  });

  it('2026–2030 每年都有 10 条农历/节气命中日（8 农历 + 2 节气）', () => {
    const LUNAR_LABELS = new Set([
      '春节',
      '元宵节',
      '清明节',
      '端午节',
      '七夕',
      '中元节',
      '中秋节',
      '重阳节',
      '腊八节',
      '冬至',
    ]);
    const counts: Record<number, number> = {};
    for (let y = 2026; y <= 2030; y += 1) {
      for (let m = 1; m <= 12; m += 1) {
        for (let d = 1; d <= 31; d += 1) {
          const day = at(y, m, d);
          // 跳过滚动产生的越界日（如 2 月 30 日）
          if (day.getMonth() + 1 !== m || day.getDate() !== d) continue;
          const plan = getTodaysFestivalPlan(day);
          if (plan && LUNAR_LABELS.has(plan.label)) counts[y] = (counts[y] ?? 0) + 1;
        }
      }
    }
    for (let y = 2026; y <= 2030; y += 1) {
      expect(counts[y]).toBe(10);
    }
  });
});

describe('getTodaysFestivalPlan —— 公历固定节日', () => {
  it('圣诞 / 万圣 命中，且动作可用', () => {
    expect(getTodaysFestivalPlan(at(2026, 12, 25))?.label).toBe('圣诞节');
    expect(getTodaysFestivalPlan(at(2026, 12, 25))?.actions).toEqual(['festival-christmas']);
    expect(getTodaysFestivalPlan(at(2026, 10, 31))?.label).toBe('万圣节');
    expect(getTodaysFestivalPlan(at(2026, 10, 31))?.actions).toEqual(['festival-halloween']);
  });

  it('表未覆盖年份仍能命中公历节日（2035 元旦）', () => {
    expect(getTodaysFestivalPlan(at(2035, 1, 1))?.label).toBe('元旦');
  });

  it('平常日子返回 null', () => {
    expect(getTodaysFestivalPlan(at(2026, 7, 3))).toBeNull();
    expect(getTodaysFestivalPlan(at(2035, 7, 3))).toBeNull();
  });
});

describe('节日规则表自检', () => {
  it('公历规则的 id 唯一、月日合法、动作非空', () => {
    const ids = new Set<string>();
    for (const r of SOLAR_RULES as FestivalRule[]) {
      expect(ids.has(r.id)).toBe(false);
      ids.add(r.id);
      expect(r.month).toBeGreaterThanOrEqual(1);
      expect(r.month).toBeLessThanOrEqual(12);
      expect(r.day).toBeGreaterThanOrEqual(1);
      expect(r.day).toBeLessThanOrEqual(31);
      expect(r.actions.length).toBeGreaterThan(0);
    }
  });

  it('同一天重复查询返回同一个去重键', () => {
    const a = getTodaysFestivalPlan(at(2026, 9, 25));
    const b = getTodaysFestivalPlan(at(2026, 9, 25));
    expect(a?.key).toBe(b?.key);
  });
});
