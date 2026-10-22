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
const LEAVE_MS = 620;
/**
 * 最长展示时长：初始化卡住（网络、磁盘、后端超时）时必须让路。
 * 否则用户盯着的是一段循环动画，看不到下面的「正在加载…」、错误原因与重试按钮 ——
 * 装饰不该挡住诊断信息。
 */
const MAX_VISIBLE_MS = 4000;
/**
 * 视频形态的最长展示：按视频时长走，但封顶 10 秒。
 * 壁纸视频可能是几分钟的循环素材，照它播完等于把启动卡死；10 秒足够覆盖
 * 常见的"入场动画"型视频（本机那条 DeepSeek 的是 7 秒）。
 */
const MAX_VIDEO_BOOT_MS = 10_000;
/**
 * 类型待定的最长等待：选了 WE 壁纸但还不知道是场景还是视频时（旧配置没存 weKind、
 * 或库还没扫完）最多等这么久再退回渐变开屏。
 * 冷启动第一次扫描要读注册表 + 多个 Steam 库 + 十几个 project.json，实测可能过 1 秒，
 * 所以给到 2.5 秒；一旦自愈写入 weKind，后续启动不再走这条路径。
 */
const PENDING_MAX_MS = 2500;

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
  /**
   * 选了 WE 壁纸、但类型还不知道（旧版选的老条目没有 weKind，启动时库也还没扫完）。
   * 这时**不能**先按"非视频"走渐变开屏 —— 那正是"开头冒出别的动画、而且一闪就进"的成因。
   * 先什么都不画（露出壁纸底色），最多等 1.5 秒让库给出结论。
   */
  const undecided = kind === 'we' && !declaredKind && lib.status !== 'ready';

  const enabled = entryAnim && !prefersReducedMotion();
  const [phase, setPhase] = useState<'show' | 'leaving' | 'gone'>(
    enabled ? 'show' : 'gone',
  );
  const mountedAt = useRef(Date.now());
  // ready 可能在首次渲染之前就已为 true（本地缓存命中），此时仍要凑满最短时长
  const readyRef = useRef(ready);
  readyRef.current = ready;
  // effect 里要读最新的形态（壁纸库晚一点扫完会切到视频形态），用 ref 传出去
  const overVideoRef = useRef(overVideo);
  overVideoRef.current = overVideo;
  const undecidedRef = useRef(undecided);
  undecidedRef.current = undecided;

  // 组件挂载时就记时，避免 StrictMode 双调用时用 effect 里的新时间戳
  useEffect(() => {
    mountedAt.current = Date.now();
  }, []);

  // 告诉样式表"现在开屏是视频形态"：加载壳里的转圈要藏起来，
  // 否则它会从透明开屏底下透出来，和底部的进度提示重复。
  useEffect(() => {
    const el = document.documentElement;
    // 视频形态与"待定"形态都不盖背景，加载壳里的转圈同样要藏
    const active = enabled && (overVideo || undecided) && phase !== 'gone';
    if (active) el.dataset.boot = 'video';
    else delete el.dataset.boot;
    return () => {
      delete el.dataset.boot;
    };
  }, [enabled, overVideo, undecided, phase]);

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

    /**
     * 本次开屏要展示多久。
     * - 非视频形态：原来的 900ms。
     * - 视频形态：**按视频时长**（提前一个淡出时长开始退场，让淡出与视频回到开头重叠，
     *   否则会在最后一帧突然跳回第一帧），下限 900ms、上限 10s。
     *   视频还没解出时长（readyState < 1）时退回 900ms，绝不干等一个未知值。
     */
    const targetMs = () => {
      if (!overVideoRef.current) return MIN_VISIBLE_MS;
      const v = document.querySelector<HTMLVideoElement>('video.wp-video');
      const d = v?.duration;
      if (!v || !d || !Number.isFinite(d) || v.readyState < 1) return MIN_VISIBLE_MS;
      return Math.min(Math.max(d * 1000 - LEAVE_MS, MIN_VISIBLE_MS), MAX_VIDEO_BOOT_MS);
    };

    const tick = () => {
      if (stopped) return;
      const elapsed = Date.now() - mountedAt.current;
      // 类型还没定（旧条目的 weKind 缺失 / 库还没扫完）：先别急着走
      if (undecidedRef.current && elapsed < PENDING_MAX_MS) {
        window.setTimeout(tick, 120);
        return;
      }
      const target = targetMs();
      // 未就绪时最多等 MAX_VISIBLE_MS：必须让用户看见「正在加载 / 初始化失败」
      const hardCap = readyRef.current ? Math.max(target, MAX_VIDEO_BOOT_MS) : MAX_VISIBLE_MS;
      if ((readyRef.current && elapsed >= target) || elapsed >= hardCap) {
        finish();
        return;
      }
      window.setTimeout(tick, 120);
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

  // 类型待定：什么都别画（连品牌标也不放），只留底色，避免"先闪一段别的动画"
  if (undecided && !overVideo) {
    return (
      <div className={`${cls} entry-splash--pending`} role="presentation" aria-hidden />
    );
  }

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
