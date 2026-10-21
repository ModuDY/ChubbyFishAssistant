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
import type { PetAutoPolicy } from './autoPolicy';

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
/**
 * 命中概率（2026-10-19 新增）：
 * 节流只保证"不会太频繁"，但每次事件都点评仍然机械。过闸后再掷一次骰子，
 * 让它像"偶尔插一句"而不是"报幕员"。
 */
const AUTO_PROBABILITY = 0.35;
/** 网关运行态缓存刷新间隔（ms） */
const GATEWAY_POLL_MS = 30_000;

type Options = {
  enabled: boolean;
  /** 「别打扰」判定：正在流式 / 拖拽中 / 窗口隐藏 / 正在播瞬态动作 */
  isBusy: () => boolean;
  /** 统一自动发言策略：每日配额与「今天别主动说话」与时间问候共用 */
  policy: PetAutoPolicy;
  dailyCap: number;
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
    // 统一策略：今天已闭嘴 / 深夜 / 当日配额用尽 → 直接安静
    if (!o.policy.canSpeak(o.dailyCap)) return;
    const now = Date.now();
    const gap = AUTO_GAP_MS + Math.random() * AUTO_JITTER_MS;
    if (lastRef.current && now - lastRef.current < gap) return;
    // 过闸后再掷骰子：大部分事件只是安静播动作，偶尔才接一句话
    if (Math.random() > AUTO_PROBABILITY) return;
    lastRef.current = now;
    o.policy.noteSpoken();
    void o.startSay(prompt, context ?? null, 'auto');
  }, []);

  return { say: maybeSay };
}
