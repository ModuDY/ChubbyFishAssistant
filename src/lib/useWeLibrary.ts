/**
 * 本机壁纸库的加载状态（模块级缓存 + 订阅）。
 *
 * 为什么不用 useState + useEffect 各组件自己扫：
 * 扫描要遍历注册表、多个 Steam 库、几十个工程目录，重复调用很贵；
 * 而壁纸层与设置面板都要用同一份结果 —— 扫一次，全应用共享。
 */

import { useEffect, useSyncExternalStore } from 'react';
import { scanWe, type WeScanResult } from './we';

type State = {
  status: 'idle' | 'loading' | 'ready' | 'error';
  data: WeScanResult | null;
  error: string | null;
};

let state: State = { status: 'idle', data: null, error: null };
const listeners = new Set<() => void>();

function emit(next: State) {
  state = next;
  for (const fn of listeners) fn();
}

let inflight: Promise<void> | null = null;

/** 触发一次扫描（已在飞行中则复用同一个 Promise） */
export function loadWeLibrary(force = false): Promise<void> {
  if (!force && state.status === 'ready') return Promise.resolve();
  if (inflight) return inflight;
  emit({ ...state, status: 'loading', error: null });
  inflight = scanWe()
    .then((data) => {
      emit({ status: 'ready', data, error: null });
    })
    .catch((e) => {
      emit({ status: 'error', data: null, error: String(e) });
    })
    .finally(() => {
      inflight = null;
    });
  return inflight;
}

function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

const getSnapshot = () => state;

export function useWeLibrary(autoLoad = true) {
  const snap = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  useEffect(() => {
    if (autoLoad && snap.status === 'idle') void loadWeLibrary();
  }, [autoLoad, snap.status]);
  return snap;
}
