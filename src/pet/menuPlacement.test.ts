import { describe, expect, it } from 'vitest';

import { MENU_EDGE, placeMenu } from './menuPlacement';

const VIEWPORT = { width: 800, height: 600 };
const SIZE = { width: 216, height: 320 };

describe('placeMenu —— 光标锚点与翻转', () => {
  it('空间充足时落在光标右下', () => {
    const p = placeMenu({ x: 100, y: 100 }, SIZE, VIEWPORT);
    expect(p).toEqual({ left: 106, top: 106, flippedX: false, flippedY: false });
  });

  it('贴近右边缘时向左翻转，且仍与光标保持间距', () => {
    const p = placeMenu({ x: 780, y: 100 }, SIZE, VIEWPORT);
    expect(p.flippedX).toBe(true);
    expect(p.left).toBe(780 - 6 - SIZE.width);
  });

  it('贴近下边缘时向上翻转', () => {
    const p = placeMenu({ x: 100, y: 580 }, { width: 216, height: 200 }, VIEWPORT);
    expect(p.flippedY).toBe(true);
    expect(p.top).toBe(580 - 6 - 200);
  });

  it('卡片比窗口还高时，夹紧到上边缘而不是负坐标', () => {
    // 宠物缩到 0.6 时窗口只有 340×216
    const p = placeMenu({ x: 200, y: 100 }, { width: 216, height: 320 }, { width: 340, height: 216 });
    expect(p.top).toBe(MENU_EDGE);
    expect(p.left).toBeGreaterThanOrEqual(MENU_EDGE);
    expect(p.top + Math.min(320, 216 - MENU_EDGE * 2)).toBeLessThanOrEqual(216);
  });
});

describe('placeMenu —— 角色避让', () => {
  it('与角色重叠超过 30% 时换到角色另一侧', () => {
    const pet = { left: 0, top: 0, width: 300, height: 600 };
    const p = placeMenu({ x: 100, y: 300 }, SIZE, VIEWPORT, pet);
    expect(p.left).toBe(pet.left + pet.width + 6);
  });

  it('角色在右侧时改为向左让位', () => {
    const pet = { left: 500, top: 0, width: 300, height: 600 };
    const p = placeMenu({ x: 560, y: 300 }, SIZE, VIEWPORT, pet);
    expect(p.flippedX).toBe(true);
    expect(p.left).toBe(pet.left - 6 - SIZE.width);
  });

  it('任何位置下卡片都完整落在窗口内', () => {
    const anchors = [
      { x: 0, y: 0 },
      { x: 800, y: 600 },
      { x: 400, y: 300 },
    ];
    for (const anchor of anchors) {
      const p = placeMenu(anchor, SIZE, VIEWPORT, { left: 0, top: 0, width: 300, height: 600 });
      expect(p.left).toBeGreaterThanOrEqual(MENU_EDGE);
      expect(p.top).toBeGreaterThanOrEqual(MENU_EDGE);
      expect(p.left + SIZE.width).toBeLessThanOrEqual(VIEWPORT.width - MENU_EDGE);
      expect(p.top + SIZE.height).toBeLessThanOrEqual(VIEWPORT.height - MENU_EDGE);
    }
  });
});
