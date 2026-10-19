/**
 * 桌宠自动点评（P3，auto 档）：事件触发时的节流与「别打扰」判定。
 *
 * 为什么单独成 hook：PetRoot 已经很长，把"该不该自动说话"的策略集中在这里，
 * PetRoot 只负责在事件回调里调 `maybeSay(prompt)`。
 *
 * 三重约束（省钱 + 不打扰）：
 * 1. 白名单：只有"有内容可说"的信号才点评（`AUTO_TRIGGER_TYPES`）；
 * 2. 节流：基础间隔 120s + 0~30s 抖动（避免固定节拍）；
 * 3. 静默：网关未运行 / 正在流式 / 拖拽中 / 窗口隐藏 / 正在播瞬态动作时不说话。
 */
import { useCallback, useEffect, useRef } from 'react';
import { api } from '../lib/tauri';
import type { PetSignalType } from './usePetEvents';

/** auto 档允许触发点评的信号（其余 working/thinking/pace/... 一律不触发） */
export const AUTO_TRIGGER_TYPES: ReadonlySet<PetSignalType> = new Set<PetSignalType>([
  'happy',
  'sad',
  'jump',
  'idle',
]);

/** 基础间隔（ms）与抖动上限（ms） */
const AUTO_GAP_MS = 120_000;
const AUTO_JITTER_MS = 30_000;
/** 网关运行态缓存刷新间隔（ms） */
const GATEWAY_POLL_MS = 30_000;

type Options = {
  enabled: boolean;
  /** 「别打扰」判定：正在流式 / 拖拽中 / 窗口隐藏 / 正在播瞬态动作 */
  isBusy: () => boolean;
  startSay: (prompt: string, context?: string | null, trigger?: 'user' | 'auto') => Promise<boolean>;
};

export function usePetAutoSay(opts: Options) {
  const optsRef = useRef(opts);
  optsRef.current = opts;
  const lastRef = useRef(0);
  const gatewayUpRef = useRef(false);

  // 网关运行态低频缓存（不每次发言都打一次 status）
  useEffect(() => {
    if (!opts.enabled) {
      gatewayUpRef.current = false;
      return;
    }
    let stopped = false;
    const tick = () => {
      void api.apiServer
        .status()
        .then((s) => {
          if (!stopped) gatewayUpRef.current = !!s?.running;
        })
        .catch(() => {});
    };
    tick();
    const timer = window.setInterval(tick, GATEWAY_POLL_MS);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [opts.enabled]);

  /** 尝试自动点评一句；不满足条件则静默跳过（不回落本地台词，避免刷屏） */
  const maybeSay = useCallback((prompt: string, context?: string | null) => {
    const o = optsRef.current;
    if (!o.enabled || !gatewayUpRef.current || o.isBusy()) return;
    const now = Date.now();
    const gap = AUTO_GAP_MS + Math.random() * AUTO_JITTER_MS;
    if (lastRef.current && now - lastRef.current < gap) return;
    lastRef.current = now;
    void o.startSay(prompt, context ?? null, 'auto');
  }, []);

  return { say: maybeSay };
}
