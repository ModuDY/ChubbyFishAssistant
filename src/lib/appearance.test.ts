import { describe, expect, it } from 'vitest';
import {
  APPEARANCE_DEFAULTS,
  GLASS_LEVELS,
  SCENES,
  glassAlpha,
  glassBlur,
  sanitizeAppearance,
} from './appearance';

describe('sanitizeAppearance', () => {
  it('空值 / 非对象回默认', () => {
    expect(sanitizeAppearance(undefined)).toEqual(APPEARANCE_DEFAULTS);
    expect(sanitizeAppearance(null)).toEqual(APPEARANCE_DEFAULTS);
    expect(sanitizeAppearance('x')).toEqual(APPEARANCE_DEFAULTS);
    expect(sanitizeAppearance(42)).toEqual(APPEARANCE_DEFAULTS);
  });

  it('合法值原样保留', () => {
    const a = sanitizeAppearance({
      wallpaper: false,
      scene: 'aurora',
      glass: 'strong',
      entryAnim: false,
    });
    expect(a).toEqual({ wallpaper: false, scene: 'aurora', glass: 'strong', entryAnim: false });
  });

  it('未知场景 / 玻璃档归一为默认（不抛错、不留脏值）', () => {
    const a = sanitizeAppearance({ scene: 'lava', glass: 'ultra' });
    expect(a.scene).toBe(APPEARANCE_DEFAULTS.scene);
    expect(a.glass).toBe(APPEARANCE_DEFAULTS.glass);
  });

  it('非布尔的开关不被字符串 / 数字污染', () => {
    const a = sanitizeAppearance({ wallpaper: 'yes', entryAnim: 1 });
    expect(a.wallpaper).toBe(APPEARANCE_DEFAULTS.wallpaper);
    expect(a.entryAnim).toBe(APPEARANCE_DEFAULTS.entryAnim);
  });

  it('逐字段独立：一个字段非法不影响其他字段', () => {
    const a = sanitizeAppearance({ wallpaper: false, scene: 'nope', glass: 'off' });
    expect(a.wallpaper).toBe(false);
    expect(a.glass).toBe('off');
    expect(a.scene).toBe(APPEARANCE_DEFAULTS.scene);
  });
});

describe('玻璃档位换算', () => {
  it('每档都有定义，且 off 是"零模糊 + 全不透明"', () => {
    for (const g of GLASS_LEVELS) {
      expect(glassBlur(g.id)).toBe(g.blur);
      expect(glassAlpha(g.id)).toBe(g.alpha);
    }
    expect(glassBlur('off')).toBe(0);
    expect(glassAlpha('off')).toBe(1);
  });

  it('强度越高模糊越大、越透（单调性，防止以后调参调反）', () => {
    const order = GLASS_LEVELS.filter((g) => g.id !== 'off');
    for (let i = 1; i < order.length; i += 1) {
      expect(order[i].blur).toBeGreaterThan(order[i - 1].blur);
      expect(order[i].alpha).toBeLessThan(order[i - 1].alpha);
    }
  });
});

describe('场景表', () => {
  it('id 唯一，且都带名称与说明', () => {
    const ids = SCENES.map((s) => s.id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const s of SCENES) {
      expect(s.name.length).toBeGreaterThan(0);
      expect(s.hint.length).toBeGreaterThan(0);
    }
  });

  it('默认场景必须在表内（否则下拉框会显示空白项）', () => {
    expect(SCENES.some((s) => s.id === APPEARANCE_DEFAULTS.scene)).toBe(true);
  });

  it('默认玻璃档必须在表内', () => {
    expect(GLASS_LEVELS.some((g) => g.id === APPEARANCE_DEFAULTS.glass)).toBe(true);
  });
});
