/**
 * 时间驱动主动开口（P12）：每 45 秒做一次轻量检查，命中"当天首次进入某时段"就开口。
 *
 * 为什么与彩蛋定时器分开：彩蛋间隔可由用户调到 30 分钟，若共用，早晚问候会被拖到
 * 时段结束之后；这里保持独立节拍，只共用「自动发言配额 / 今日静音」策略。
 *
 * 语义：进入时段后**第一次有空**时触发；忙碌时不追赶，时段结束后不补播。
 */
import { useEffect, useRef } from 'react';
import { timeSlot } from './flavor';
import { dateKey, slotEventKey, type PetAutoPolicy } from './autoPolicy';
import type { TimeSlot } from './flavor';
import type { PetProactiveMode } from '../types';

const CHECK_MS = 45_000;
/** 启动后先等一会儿：桌宠刚出现就开口会显得很急 */
const FIRST_DELAY_MS = 8_000;

type Options = {
  mode: PetProactiveMode;
  /** 自动发言每日上限（与事件自动点评共用） */
  dailyCap: number;
  policy: PetAutoPolicy;
  /** 隐藏 / 全屏 / 拖拽 / 输入 / 对局 / 正在说：让位 */
  isBusy: () => boolean;
  onGreet: (slot: TimeSlot) => void;
};

export function usePetProactive(opts: Options) {
  const optsRef = useRef(opts);
  optsRef.current = opts;

  useEffect(() => {
    if (opts.mode === 'off') return;

    const tick = () => {
      const o = optsRef.current;
      if (o.mode === 'off' || o.isBusy()) return;
      const now = new Date();
      const slot = timeSlot(now.getHours());
      if (!slot) return;
      // 深夜（23–5）由 policy.canSpeak 一并挡住；这里再判一次时段已天然排除了深夜
      const key = slotEventKey(dateKey(now), slot);
      if (o.policy.hasSeen(key)) return;
      if (!o.policy.canSpeak(o.dailyCap)) return;
      // 先记账再开口：即使后续请求失败，也不在 45 秒后重复打扰
      o.policy.markSeen(key);
      o.policy.noteSpoken();
      o.onGreet(slot);
    };

    const first = window.setTimeout(tick, FIRST_DELAY_MS);
    const timer = window.setInterval(tick, CHECK_MS);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(timer);
    };
  }, [opts.mode, opts.dailyCap]);
}
