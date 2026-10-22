/**
 * 进应用动画（开屏）。
 *
 * 两种形态：
 * - **视频壁纸形态**：当前选中的是本机 Wallpaper Engine 视频壁纸时，开屏不再盖渐变，
 *   而是**露出壁纸层自己那个 `<video>`**，只叠一条极简的进度提示。
 *   关键：复用同一路视频，**不另开播放器** —— 否则开屏那份和主界面那份是两个解码器、
 *   两个播放位置，切换时必然跳帧；复用之后"开机动画 → 桌面壁纸"是同一帧连续播下去。
 * - **默认形态**：内置场景 / 非视频壁纸时，仍用原来的渐变开屏。
 *
 * 三条硬约束：
 * 1. **绝不拖慢启动**：最短展示 900ms，到点立刻淡出；无论 `ready` 何时就绪都不额外等待。
 * 2. **可跳过**：点任意处、按任意键立即结束。
 * 3. **失败即让路**：`prefers-reduced-motion` 或设置里关掉 ⇒ 直接不渲染；
 *    初始化卡住超过 4 秒也强制退场，把错误与重试按钮让出来。
 */

import { useEffect, useRef, useState } from 'react';
import petAvatar from '../assets/brand/pet-avatar.png';
import { useAppearance } from '../lib/appearance';
import { useWeLibrary } from '../lib/useWeLibrary';
import { APP_NAME } from '../lib/about';

/** 最短展示时长：低于动画自身时长（约 0.62s + 0.56s 延迟）会看到"闪一下" */
const MIN_VISIBLE_MS = 900;
/** 淡出时长，须与 index.css 的 entry-fade-out 一致 */
const LEAVE_MS = 440;
/**
 * 最长展示时长：初始化卡住（网络、磁盘、后端超时）时必须让路。
 * 否则用户盯着的是一段循环动画，看不到下面的「正在加载…」、错误原因与重试按钮 ——
 * 装饰不该挡住诊断信息。
 */
const MAX_VISIBLE_MS = 4000;

function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || !window.matchMedia) return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

export default function EntrySplash({ ready }: { ready: boolean }) {
  const { entryAnim, kind, weId, weKind, wallpaper } = useAppearance();
  const lib = useWeLibrary(wallpaper && kind === 'we' && entryAnim);
  const target = lib.data?.items.find((w) => w.id === weId) ?? null;
  /** 视频壁纸：开屏让位给壁纸层里的那个 video。
      用持久化的 weKind 兜底 —— 刚启动时壁纸库还没扫完，等它就等于先闪一下渐变开屏。 */
  const declaredKind = target?.kind ?? weKind ?? null;
  const overVideo = kind === 'we' && declaredKind === 'video';

  const enabled = entryAnim && !prefersReducedMotion();
  const [phase, setPhase] = useState<'show' | 'leaving' | 'gone'>(
    enabled ? 'show' : 'gone',
  );
  const mountedAt = useRef(Date.now());
  // ready 可能在首次渲染之前就已为 true（本地缓存命中），此时仍要凑满最短时长
  const readyRef = useRef(ready);
  readyRef.current = ready;

  // 组件挂载时就记时，避免 StrictMode 双调用时用 effect 里的新时间戳
  useEffect(() => {
    mountedAt.current = Date.now();
  }, []);

  // 告诉样式表"现在开屏是视频形态"：加载壳里的转圈要藏起来，
  // 否则它会从透明开屏底下透出来，和底部的进度提示重复。
  useEffect(() => {
    const el = document.documentElement;
    const active = enabled && overVideo && phase !== 'gone';
    if (active) el.dataset.boot = 'video';
    else delete el.dataset.boot;
    return () => {
      delete el.dataset.boot;
    };
  }, [enabled, overVideo, phase]);

  useEffect(() => {
    if (!enabled) return;
    let goneTimer = 0;
    let stopped = false;

    const finish = () => {
      if (stopped) return;
      setPhase('leaving');
      goneTimer = window.setTimeout(() => {
        if (!stopped) setPhase('gone');
      }, LEAVE_MS);
    };

    const tick = () => {
      if (stopped) return;
      const elapsed = Date.now() - mountedAt.current;
      // 就绪且凑满最短时长 → 走；或者已经超过上限（初始化卡住）→ 也必须让路
      if ((readyRef.current && elapsed >= MIN_VISIBLE_MS) || elapsed >= MAX_VISIBLE_MS) {
        finish();
        return;
      }
      // 未就绪 → 继续等；已就绪但不足最短时长 → 补足剩余时间
      const wait = readyRef.current ? MIN_VISIBLE_MS - elapsed : 120;
      window.setTimeout(tick, wait);
    };

    tick();

    const skip = () => finish();
    window.addEventListener('pointerdown', skip, { once: true });
    window.addEventListener('keydown', skip, { once: true });

    return () => {
      stopped = true;
      window.clearTimeout(goneTimer);
      window.removeEventListener('pointerdown', skip);
      window.removeEventListener('keydown', skip);
    };
  }, [enabled]);

  if (phase === 'gone') return null;

  const cls = [
    'entry-splash',
    overVideo ? 'entry-splash--video' : '',
    phase === 'leaving' ? 'entry-splash--leaving' : '',
  ]
    .filter(Boolean)
    .join(' ');

  // 视频形态：不盖背景，只在底部放一枚很小的品牌标 + 流动进度线
  if (overVideo) {
    return (
      <div className={cls} role="presentation" aria-hidden>
        <div className="entry-video-foot">
          <img className="entry-video-avatar" src={petAvatar} alt="" draggable={false} />
          <span className="entry-video-name">{APP_NAME}</span>
          <span className="entry-bar entry-bar--video" />
        </div>
      </div>
    );
  }

  return (
    <div className={cls} role="presentation" aria-hidden>
      <div className="entry-inner">
        <span className="entry-ring entry-ring--outer" />
        <span className="entry-ring" />
        <img className="entry-avatar" src={petAvatar} alt="" draggable={false} />
        <div className="entry-title">{APP_NAME}</div>
        <div className="entry-sub">正在唤醒工作台…</div>
        <div className="entry-bar" />
      </div>
    </div>
  );
}
