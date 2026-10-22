/**
 * 外观增强偏好（壁纸 / 场景 / 玻璃强度 / 开场动画）。
 *
 * 为什么单独放这里而不是进 Settings DTO：
 * 这四项是**纯设备本地的视觉偏好**，不参与账号数据、不影响业务、不需要跨机器同步。
 * 塞进 Rust 的 `Settings` 结构会让一个纯 CSS 开关牵动 serde 默认值、SQLite kv 与
 * types.ts 三处契约（且 `settings_set` 是 patch 合并语义，多写的键会静默存进 kv
 * 却读不回类型），代价与收益不成比例。故按桌宠 autoPolicy 的既有先例走 localStorage。
 *
 * 失效安全：读不出 / 值非法一律回默认，绝不让脏值把界面打到不可用状态。
 */

import { useSyncExternalStore } from 'react';

/** 壁纸场景：深海鲸游 / 极光海面 / 深渊微光 */
export type SceneId = 'deep-sea' | 'aurora' | 'abyss';
/** 玻璃强度（off = 实心面板，与改造前的观感完全一致） */
export type GlassLevel = 'off' | 'soft' | 'standard' | 'strong';

export interface Appearance {
  /** 壁纸总开关（关掉后与改造前完全一致） */
  wallpaper: boolean;
  /** 当前场景 */
  scene: SceneId;
  /** 玻璃强度 */
  glass: GlassLevel;
  /** 进应用动画 */
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

export const APPEARANCE_DEFAULTS: Appearance = {
  wallpaper: true,
  scene: 'deep-sea',
  glass: 'soft',
  entryAnim: true,
};

const STORAGE_KEY = 'cf-appearance-v1';

function storage(): Storage | undefined {
  try {
    return typeof window !== 'undefined' ? window.localStorage : undefined;
  } catch {
    return undefined;
  }
}

/** 逐字段归一：非法值回默认（未知主题值归一为默认的同一套思路） */
export function sanitizeAppearance(raw: unknown): Appearance {
  const src = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  const scene = SCENES.some((s) => s.id === src.scene)
    ? (src.scene as SceneId)
    : APPEARANCE_DEFAULTS.scene;
  const glass = GLASS_LEVELS.some((g) => g.id === src.glass)
    ? (src.glass as GlassLevel)
    : APPEARANCE_DEFAULTS.glass;
  return {
    wallpaper:
      typeof src.wallpaper === 'boolean' ? src.wallpaper : APPEARANCE_DEFAULTS.wallpaper,
    scene,
    glass,
    entryAnim:
      typeof src.entryAnim === 'boolean' ? src.entryAnim : APPEARANCE_DEFAULTS.entryAnim,
  };
}

export function loadAppearance(): Appearance {
  const s = storage();
  if (!s) return APPEARANCE_DEFAULTS;
  try {
    const raw = s.getItem(STORAGE_KEY);
    return raw ? sanitizeAppearance(JSON.parse(raw)) : APPEARANCE_DEFAULTS;
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

export function glassBlur(level: GlassLevel): number {
  return GLASS_LEVELS.find((g) => g.id === level)?.blur ?? 0;
}

export function glassAlpha(level: GlassLevel): number {
  return GLASS_LEVELS.find((g) => g.id === level)?.alpha ?? 1;
}

/**
 * 把偏好写到 `<html>` 的 data 属性与 CSS 变量上。
 * 样式表按 `html[data-wallpaper='on']` 取用 —— 与主题同一套「属性驱动样式」的做法，
 * 避免每个组件各自维护一份开关心跳。
 */
export function applyAppearance(a: Appearance) {
  if (typeof document === 'undefined') return;
  const el = document.documentElement;
  el.dataset.wallpaper = a.wallpaper ? 'on' : 'off';
  el.dataset.scene = a.scene;
  el.dataset.glass = a.glass;
  el.style.setProperty('--wp-blur', `${glassBlur(a.glass)}px`);
  el.style.setProperty('--wp-alpha', String(glassAlpha(a.glass)));
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

export function setAppearance(patch: Partial<Appearance>): Appearance {
  ensureInit();
  const next = sanitizeAppearance({ ...current, ...patch });
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
