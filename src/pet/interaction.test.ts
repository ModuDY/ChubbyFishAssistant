import { describe, expect, it } from 'vitest';

import {
  CLICK_LIMIT_AT,
  CLICK_TEASE_AT,
  CLICK_WINDOW_MS,
  HOVER_COOLDOWN_MS,
  HOVER_PET_MS,
  clickKindFor,
  shouldPetOnHover,
  trackClick,
} from './interaction';

describe('trackClick —— 连点分流', () => {
  it('第 1~2 次点击是普通回应', () => {
    let h: number[] = [];
    let r = trackClick(h, 1000);
    h = r.history;
    expect(r.mood).toBe('normal');
    r = trackClick(h, 1200);
    expect(r.mood).toBe('normal');
  });

  it('第 3 次点击开始变脸（戳烦了）', () => {
    let h: number[] = [];
    for (let i = 0; i < CLICK_TEASE_AT; i += 1) {
      const r = trackClick(h, 1000 + i * 100);
      h = r.history;
      if (i === CLICK_TEASE_AT - 1) expect(r.mood).toBe('tease');
    }
  });

  it('达到硬上限后升级为 annoyed', () => {
    let h: number[] = [];
    let mood = 'normal';
    for (let i = 0; i < CLICK_LIMIT_AT; i += 1) {
      const r = trackClick(h, 1000 + i * 100);
      h = r.history;
      mood = r.mood;
    }
    expect(mood).toBe('annoyed');
  });

  it('窗口外的旧点击会被裁掉（停手后回落到 normal）', () => {
    let r = trackClick([], 0);
    for (let i = 1; i < CLICK_LIMIT_AT; i += 1) r = trackClick(r.history, i * 100);
    expect(r.mood).toBe('annoyed');
    // 隔一个窗口再点一次：旧记录全部过期
    const later = trackClick(r.history, CLICK_WINDOW_MS + 5000);
    expect(later.mood).toBe('normal');
    expect(later.count).toBe(1);
  });

  it('tease 与非 tease 映射到不同的瞬态语义', () => {
    expect(clickKindFor('normal')).toBe('click');
    expect(clickKindFor('tease')).toBe('tease');
    expect(clickKindFor('annoyed')).toBe('tease');
  });
});

describe('shouldPetOnHover —— 悬停抚摸', () => {
  it('悬停够久且冷却已过才触发', () => {
    expect(shouldPetOnHover(true, HOVER_PET_MS, 0, false, 100_000)).toBe(true);
  });

  it('悬停太短不触发（避免鼠标扫过就摸）', () => {
    expect(shouldPetOnHover(true, HOVER_PET_MS - 1, 0, false, 100_000)).toBe(false);
  });

  it('正忙（拖拽 / 播动作 / 窗口隐藏）时一律不触发', () => {
    expect(shouldPetOnHover(true, HOVER_PET_MS + 100, 0, true, 100_000)).toBe(false);
  });

  it('鼠标不在角色上不触发', () => {
    expect(shouldPetOnHover(false, HOVER_PET_MS + 100, 0, false, 100_000)).toBe(false);
  });

  it('冷却期内不重复触发', () => {
    const last = 100_000;
    expect(shouldPetOnHover(true, HOVER_PET_MS + 100, last, false, last + 1000)).toBe(false);
    expect(shouldPetOnHover(true, HOVER_PET_MS + 100, last, false, last + HOVER_COOLDOWN_MS)).toBe(
      true,
    );
  });
});
