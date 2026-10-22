import { describe, expect, it } from 'vitest';
import {
  APPEARANCE_DEFAULTS,
  GLASS_LEVELS,
  GLASS_SURFACES,
  SCENES,
  levelOf,
  resolveSurface,
  sanitizeAppearance,
} from './appearance';

describe('sanitizeAppearance', () => {
  it('空值 / 非对象回默认', () => {
    expect(sanitizeAppearance(undefined)).toEqual(APPEARANCE_DEFAULTS);
    expect(sanitizeAppearance(null)).toEqual(APPEARANCE_DEFAULTS);
    expect(sanitizeAppearance('x')).toEqual(APPEARANCE_DEFAULTS);
  });

  it('合法值原样保留', () => {
    const a = sanitizeAppearance({ wallpaper: false, scene: 'aurora', kind: 'we', weId: '123' });
    expect(a.wallpaper).toBe(false);
    expect(a.scene).toBe('aurora');
    expect(a.kind).toBe('we');
    expect(a.weId).toBe('123');
  });

  it('未知场景 / 玻璃档归一为默认', () => {
    const a = sanitizeAppearance({ scene: 'lava', glassLevel: 'ultra' });
    expect(a.scene).toBe(APPEARANCE_DEFAULTS.scene);
    expect(a.glassLevel).toBe(APPEARANCE_DEFAULTS.glassLevel);
  });

  it('kind 只接受 css / we，其余归一为 css', () => {
    expect(sanitizeAppearance({ kind: 'video' }).kind).toBe('css');
    expect(sanitizeAppearance({ kind: 'we' }).kind).toBe('we');
  });

  it('数值越界被钳制而不是丢弃', () => {
    const a = sanitizeAppearance({ fx: { intensity: 99 }, parallax: { wallpaper: -5 } });
    expect(a.fx.intensity).toBe(2);
    expect(a.parallax.wallpaper).toBe(0);
  });

  it('非布尔的开关不被字符串 / 数字污染', () => {
    const a = sanitizeAppearance({ wallpaper: 'yes', entryAnim: 1 });
    expect(a.wallpaper).toBe(APPEARANCE_DEFAULTS.wallpaper);
    expect(a.entryAnim).toBe(APPEARANCE_DEFAULTS.entryAnim);
  });

  it('glass 缺字段时逐面回落默认，而不是整块丢掉', () => {
    const a = sanitizeAppearance({ glass: { sidebar: { inherit: false, blur: 30, alpha: 0.3 } } });
    expect(a.glass.sidebar).toEqual({ inherit: false, blur: 30, alpha: 0.3 });
    expect(a.glass.titlebar).toEqual(APPEARANCE_DEFAULTS.glass.titlebar);
  });
});

describe('分面玻璃解析', () => {
  it('inherit=true 时回落到全局档位', () => {
    const a = sanitizeAppearance({ glassLevel: 'strong' });
    const r = resolveSurface(a, 'sidebar');
    expect(r.blur).toBe(levelOf('strong').blur);
    expect(r.alpha).toBe(levelOf('strong').alpha);
  });

  it('inherit=false 时用自己的值，不受全局影响', () => {
    const a = sanitizeAppearance({
      glassLevel: 'strong',
      glass: { card: { inherit: false, blur: 4, alpha: 0.9 } },
    });
    expect(resolveSurface(a, 'card').blur).toBe(4);
    expect(resolveSurface(a, 'card').alpha).toBeCloseTo(0.9, 6);
  });

  it('卡片那一路按 0.92 折算（面积大，太透会压掉正文对比度）', () => {
    const a = sanitizeAppearance({
      glass: { card: { inherit: false, blur: 4, alpha: 0.8 } },
    });
    expect(resolveSurface(a, 'card', 0.92).alpha).toBeCloseTo(0.736, 6);
  });

  it('每档都落在合法区间，且 off 是"零模糊 + 全不透明"', () => {
    for (const g of GLASS_LEVELS) {
      expect(g.blur).toBeGreaterThanOrEqual(0);
      expect(g.alpha).toBeGreaterThan(0);
      expect(g.alpha).toBeLessThanOrEqual(1);
    }
    expect(levelOf('off').blur).toBe(0);
    expect(levelOf('off').alpha).toBe(1);
  });

  it('强度越高模糊越大、越透（防以后调参调反）', () => {
    const order = GLASS_LEVELS.filter((g) => g.id !== 'off');
    for (let i = 1; i < order.length; i += 1) {
      expect(order[i].blur).toBeGreaterThan(order[i - 1].blur);
      expect(order[i].alpha).toBeLessThan(order[i - 1].alpha);
    }
  });

  it('四个面都有定义，且与默认配置一一对应', () => {
    expect(GLASS_SURFACES).toHaveLength(4);
    for (const { id } of GLASS_SURFACES) {
      expect(APPEARANCE_DEFAULTS.glass[id]).toBeDefined();
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
});
