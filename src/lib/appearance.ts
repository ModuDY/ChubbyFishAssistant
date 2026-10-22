/**
 * 外观增强偏好（壁纸 / 玻璃分面 / 光效 / 视差 / 开屏）。
 *
 * 为什么单独放这里而不是进 Settings DTO：
 * 这些都是**纯设备本地的视觉偏好**，不参与账号数据、不影响业务、不需要跨机器同步。
 * 塞进 Rust 的 `Settings` 结构会让一个纯 CSS 开关牵动 serde 默认值、SQLite kv 与
 * types.ts 三处契约（且 `settings_set` 是 patch 合并语义，多写的键会静默存进 kv
 * 却读不回类型），代价与收益不成比例。故按桌宠 autoPolicy 的既有先例走 localStorage。
 *
 * 失效安全：读不出 / 值非法一律回默认，绝不让脏值把界面打到不可用状态。
 */

import { useSyncExternalStore } from 'react';

/** 内置 CSS 场景（不依赖任何外部素材） */
export type SceneId = 'deep-sea' | 'aurora' | 'abyss';
/** 壁纸来源：内置场景 / 本机 Wallpaper Engine 场景包 */
export type WallpaperEngineKind = 'css' | 'we';
/** 玻璃面：与 index.css 里的 `--wp-*` 分面变量一一对应 */
export type GlassSurface = 'titlebar' | 'topbar' | 'sidebar' | 'card';
export type GlassLevel = 'off' | 'soft' | 'standard' | 'strong';

/** 单个玻璃面的"跟随全局 / 独立配置"两态（对齐研读对象 ADR-0008 的收口方式） */
export interface GlassSurfaceConfig {
  /** true = 用全局档位；false = 用本面的 blur/alpha */
  inherit: boolean;
  blur: number;
  alpha: number;
}

export interface FxConfig {
  enabled: boolean;
  /** 点击爆闪环 */
  click: boolean;
  /** 彗尾拖尾 */
  trail: boolean;
  /** 点击星火 */
  sparks: boolean;
  /** 指针浮尘 */
  dust: boolean;
  /** 0.4 ~ 2.0，统一缩放粒子数量与尺寸 */
  intensity: number;
}

export interface ParallaxConfig {
  enabled: boolean;
  /** 壁纸层位移幅度（屏幕对角线的百分比，0-3） */
  wallpaper: number;
  /** 界面组（输入卡片/会话区/侧栏）位移幅度，比壁纸略大才有纵深 */
  ui: number;
  /** 缓动系数 0.05-0.4，越大越跟手 */
  smoothing: number;
}

export interface Appearance {
  wallpaper: boolean;
  kind: WallpaperEngineKind;
  /** 内置场景 id */
  scene: SceneId;
  /** 选中的 WE 壁纸目录名（kind==='we' 时生效） */
  weId: string | null;
  /**
   * 选中那张壁纸的类型（scene / video / web）。
   * 为什么要存：启动瞬间壁纸库还没扫完（扫描要读注册表与多个 Steam 库），
   * 只靠 weId 无法判断该挂 WebGL 还是 <video>，界面会先闪一下内置场景再切过去。
   * 把类型一起持久化，开屏与壁纸层就能第一帧走对分支。
   */
  weKind: string | null;
  glassLevel: GlassLevel;
  glass: Record<GlassSurface, GlassSurfaceConfig>;
  fx: FxConfig;
  parallax: ParallaxConfig;
  entryAnim: boolean;
}

export const SCENES: ReadonlyArray<{ id: SceneId; name: string; hint: string }> = [
  { id: 'deep-sea', name: '深海鲸游', hint: '深蓝水层 + 光柱 + 上浮气泡 + 大肥鱼巡游' },
  { id: 'aurora', name: '极光海面', hint: '夜蓝海面 + 缓慢漂移的极光带' },
  { id: 'abyss', name: '深渊微光', hint: '近黑深蓝 + 稀疏浮尘，最省电' },
];

export const GLASS_LEVELS: ReadonlyArray<{ id: GlassLevel; name: string; blur: number; alpha: number }> = [
  { id: 'off', name: '关闭（实心面板）', blur: 0, alpha: 1 },
  { id: 'soft', name: '轻雾', blur: 10, alpha: 0.72 },
  { id: 'standard', name: '标准', blur: 16, alpha: 0.6 },
  { id: 'strong', name: '浓雾', blur: 26, alpha: 0.46 },
];

export const GLASS_SURFACES: ReadonlyArray<{ id: GlassSurface; name: string; hint: string }> = [
  { id: 'titlebar', name: '标题栏', hint: '窗口最上一条（品牌图标 + 窗口按钮）' },
  { id: 'topbar', name: '顶栏', hint: '页面顶部工具条' },
  { id: 'sidebar', name: '侧栏', hint: '左侧应用导航' },
  { id: 'card', name: '卡片', hint: '内容区所有 .card（不加模糊，只半透）' },
];

const DEFAULT_LEVEL = GLASS_LEVELS.find((g) => g.id === 'soft')!;

function defaultGlass(): Record<GlassSurface, GlassSurfaceConfig> {
  const make = (): GlassSurfaceConfig => ({
    inherit: true,
    blur: DEFAULT_LEVEL.blur,
    alpha: DEFAULT_LEVEL.alpha,
  });
  return {
    titlebar: make(),
    topbar: make(),
    sidebar: make(),
    card: make(),
  };
}

export const APPEARANCE_DEFAULTS: Appearance = {
  wallpaper: true,
  kind: 'css',
  scene: 'deep-sea',
  weId: null,
  weKind: null,
  glassLevel: 'soft',
  glass: defaultGlass(),
  fx: { enabled: true, click: true, trail: true, sparks: true, dust: true, intensity: 1 },
  parallax: { enabled: true, wallpaper: 1.2, ui: 0.5, smoothing: 0.14 },
  entryAnim: true,
};

const STORAGE_KEY = 'cf-appearance-v2';
/** v1 的旧键：只用来迁移一次，迁完即删 */
const LEGACY_KEY = 'cf-appearance-v1';

function storage(): Storage | undefined {
  try {
    return typeof window !== 'undefined' ? window.localStorage : undefined;
  } catch {
    return undefined;
  }
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

function num(v: unknown, fallback: number, lo: number, hi: number): number {
  return typeof v === 'number' && Number.isFinite(v) ? clamp(v, lo, hi) : fallback;
}

function bool(v: unknown, fallback: boolean): boolean {
  return typeof v === 'boolean' ? v : fallback;
}

function sanitizeGlass(raw: unknown): Record<GlassSurface, GlassSurfaceConfig> {
  const out = defaultGlass();
  const src = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  for (const { id } of GLASS_SURFACES) {
    const item = src[id];
    if (!item || typeof item !== 'object') continue;
    const it = item as Record<string, unknown>;
    out[id] = {
      inherit: bool(it.inherit, true),
      blur: num(it.blur, out[id].blur, 0, 40),
      alpha: num(it.alpha, out[id].alpha, 0.2, 1),
    };
  }
  return out;
}

/** 逐字段归一：非法值回默认 */
export function sanitizeAppearance(raw: unknown): Appearance {
  const src = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  const scene = SCENES.some((s) => s.id === src.scene)
    ? (src.scene as SceneId)
    : APPEARANCE_DEFAULTS.scene;
  const glassLevel = GLASS_LEVELS.some((g) => g.id === src.glassLevel)
    ? (src.glassLevel as GlassLevel)
    : APPEARANCE_DEFAULTS.glassLevel;
  const fxSrc = (src.fx && typeof src.fx === 'object' ? src.fx : {}) as Record<string, unknown>;
  const pSrc = (src.parallax && typeof src.parallax === 'object' ? src.parallax : {}) as Record<
    string,
    unknown
  >;
  return {
    wallpaper: bool(src.wallpaper, APPEARANCE_DEFAULTS.wallpaper),
    kind: src.kind === 'we' ? 'we' : 'css',
    scene,
    weId: typeof src.weId === 'string' && src.weId ? src.weId : null,
    weKind: typeof src.weKind === 'string' && src.weKind ? src.weKind : null,
    glassLevel,
    glass: sanitizeGlass(src.glass),
    fx: {
      enabled: bool(fxSrc.enabled, APPEARANCE_DEFAULTS.fx.enabled),
      click: bool(fxSrc.click, APPEARANCE_DEFAULTS.fx.click),
      trail: bool(fxSrc.trail, APPEARANCE_DEFAULTS.fx.trail),
      sparks: bool(fxSrc.sparks, APPEARANCE_DEFAULTS.fx.sparks),
      dust: bool(fxSrc.dust, APPEARANCE_DEFAULTS.fx.dust),
      intensity: num(fxSrc.intensity, APPEARANCE_DEFAULTS.fx.intensity, 0.4, 2),
    },
    parallax: {
      enabled: bool(pSrc.enabled, APPEARANCE_DEFAULTS.parallax.enabled),
      wallpaper: num(pSrc.wallpaper, APPEARANCE_DEFAULTS.parallax.wallpaper, 0, 3),
      ui: num(pSrc.ui, APPEARANCE_DEFAULTS.parallax.ui, 0, 3),
      smoothing: num(pSrc.smoothing, APPEARANCE_DEFAULTS.parallax.smoothing, 0.05, 0.4),
    },
    entryAnim: bool(src.entryAnim, APPEARANCE_DEFAULTS.entryAnim),
  };
}

/** 从 v1 迁移：只搬还能对应的三项，其余走默认 */
function migrateLegacy(raw: unknown): Partial<Appearance> {
  if (!raw || typeof raw !== 'object') return {};
  const it = raw as Record<string, unknown>;
  const out: Partial<Appearance> = {};
  if (typeof it.wallpaper === 'boolean') out.wallpaper = it.wallpaper;
  if (typeof it.entryAnim === 'boolean') out.entryAnim = it.entryAnim;
  if (SCENES.some((s) => s.id === it.scene)) out.scene = it.scene as SceneId;
  if (GLASS_LEVELS.some((g) => g.id === it.glass)) {
    out.glassLevel = it.glass as GlassLevel;
    const hit = GLASS_LEVELS.find((g) => g.id === it.glass)!;
    out.glass = defaultGlass();
    for (const { id } of GLASS_SURFACES) {
      out.glass[id] = { inherit: true, blur: hit.blur, alpha: hit.alpha };
    }
  }
  return out;
}

export function loadAppearance(): Appearance {
  const s = storage();
  if (!s) return APPEARANCE_DEFAULTS;
  try {
    const raw = s.getItem(STORAGE_KEY);
    if (raw) return sanitizeAppearance(JSON.parse(raw));
    const legacy = s.getItem(LEGACY_KEY);
    if (legacy) {
      const migrated = sanitizeAppearance({ ...APPEARANCE_DEFAULTS, ...migrateLegacy(JSON.parse(legacy)) });
      s.setItem(STORAGE_KEY, JSON.stringify(migrated));
      s.removeItem(LEGACY_KEY);
      return migrated;
    }
    return APPEARANCE_DEFAULTS;
  } catch {
    return APPEARANCE_DEFAULTS;
  }
}

function persist(next: Appearance) {
  try {
    storage()?.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // 隐私模式 / 配额满：本次会话内仍然生效，不阻断
  }
}

export function levelOf(level: GlassLevel) {
  return GLASS_LEVELS.find((g) => g.id === level) ?? DEFAULT_LEVEL;
}

/** 某个面最终生效的 blur/alpha（inherit 时回落全局档位） */
export function resolveSurface(a: Appearance, surface: GlassSurface, factor = 1) {
  const cfg = a.glass[surface];
  const level = levelOf(a.glassLevel);
  const blur = cfg.inherit ? level.blur : cfg.blur;
  const alpha = cfg.inherit ? level.alpha : cfg.alpha;
  return { blur, alpha: clamp(alpha * factor, 0.05, 1) };
}

/**
 * 把偏好写到 `<html>` 的 data 属性与 CSS 变量上。
 * 样式表按 `html[data-wallpaper='on']` 取用 —— 与主题同一套「属性驱动样式」的做法。
 */
export function applyAppearance(a: Appearance) {
  if (typeof document === 'undefined') return;
  const el = document.documentElement;
  el.dataset.wallpaper = a.wallpaper ? 'on' : 'off';
  el.dataset.scene = a.scene;
  el.dataset.wkind = a.kind;
  el.dataset.glass = a.glassLevel;
  el.dataset.fx = a.fx.enabled ? 'on' : 'off';
  el.dataset.parallax = a.parallax.enabled ? 'on' : 'off';

  // 全局档位（保留旧变量名，兼容既有样式）
  const level = levelOf(a.glassLevel);
  el.style.setProperty('--wp-blur', `${level.blur}px`);
  el.style.setProperty('--wp-alpha', String(level.alpha));

  // 分面变量：卡片按 0.92 折算，其余 1.0（卡片面积大、太透会压掉正文对比度）
  for (const { id } of GLASS_SURFACES) {
    const factor = id === 'card' ? 0.92 : 1;
    const { blur, alpha } = resolveSurface(a, id, factor);
    el.style.setProperty(`--wp-blur-${id}`, `${blur}px`);
    el.style.setProperty(`--wp-alpha-${id}`, String(alpha));
  }

  el.style.setProperty('--wp-px-wallpaper', String(a.parallax.wallpaper));
  el.style.setProperty('--wp-px-ui', String(a.parallax.ui));
}

/* ------------------------------ 订阅 ------------------------------ */

let current: Appearance = APPEARANCE_DEFAULTS;
let initialized = false;
const listeners = new Set<() => void>();

function ensureInit() {
  if (initialized) return;
  initialized = true;
  current = loadAppearance();
  applyAppearance(current);
}

export function getAppearance(): Appearance {
  ensureInit();
  return current;
}

/** 补丁类型：glass / fx / parallax 支持只改其中几个字段（调用点不必抄全量） */
export type AppearancePatch = Partial<Omit<Appearance, 'glass' | 'fx' | 'parallax'>> & {
  glass?: Partial<Record<GlassSurface, GlassSurfaceConfig>>;
  fx?: Partial<FxConfig>;
  parallax?: Partial<ParallaxConfig>;
};

/** 深合并只做一层（glass / fx / parallax 是对象字段） */
export function setAppearance(patch: AppearancePatch): Appearance {
  ensureInit();
  const merged = {
    ...current,
    ...patch,
    glass: patch.glass ? { ...current.glass, ...patch.glass } : current.glass,
    fx: patch.fx ? { ...current.fx, ...patch.fx } : current.fx,
    parallax: patch.parallax ? { ...current.parallax, ...patch.parallax } : current.parallax,
  };
  const next = sanitizeAppearance(merged);
  current = next;
  persist(next);
  applyAppearance(next);
  for (const fn of listeners) fn();
  return next;
}

function subscribe(fn: () => void) {
  ensureInit();
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

/** React 侧读取（任何一处 setAppearance 都会让所有消费者重渲染） */
export function useAppearance(): Appearance {
  return useSyncExternalStore(subscribe, getAppearance, getAppearance);
}
