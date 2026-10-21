import { describe, expect, it } from 'vitest';

import { MENU_EDGE, MENU_OFFSET, PET_GAP, placeMenu } from './menuPlacement';

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
  it('与角色重叠时换到角色另一侧，并留出 PET_GAP', () => {
    const pet = { left: 0, top: 0, width: 300, height: 600 };
    const p = placeMenu({ x: 100, y: 300 }, SIZE, VIEWPORT, pet);
    expect(p.left).toBe(pet.left + pet.width + PET_GAP);
  });

  it('角色在右侧时改为向左让位', () => {
    const pet = { left: 500, top: 0, width: 300, height: 600 };
    const p = placeMenu({ x: 560, y: 300 }, SIZE, VIEWPORT, pet);
    expect(p.flippedX).toBe(true);
    expect(p.left).toBe(pet.left - PET_GAP - SIZE.width);
  });

  /**
   * 回归（2026-10-21，用户截图）：角色竖条只被切进 66px——
   * 占卡片总面积约 12%，旧阈值 30% 因此不触发，视觉上是"半张脸被菜单压住"。
   */
  it('细长侵入（仅切进一条边、面积占比很小）也必须让开', () => {
    // 真实实测：窗口 532×409，卡片 216×325，角色热区 left=78 width=144
    const viewport = { width: 532, height: 409 };
    const pet = { left: 78, top: 208, width: 144, height: 144 };
    const p = placeMenu({ x: 150, y: 280 }, { width: 216, height: 325 }, viewport, pet);

    // 卡片不得与角色水平方向有任何交叠
    const cardRight = p.left + 216;
    const gap = Math.max(p.left - (pet.left + pet.width), pet.left - cardRight);
    expect(gap).toBeGreaterThanOrEqual(PET_GAP);
  });

  it('未真正碰到角色时保持光标锚点不动（不该为了避让而乱跳）', () => {
    const pet = { left: 600, top: 400, width: 100, height: 100 };
    const p = placeMenu({ x: 100, y: 100 }, SIZE, VIEWPORT, pet);
    expect(p.left).toBe(100 + MENU_OFFSET);
    expect(p.top).toBe(100 + MENU_OFFSET);
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
