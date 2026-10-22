/**
 * 3D 视差：光标移动时壁纸层与界面组反向分层位移，制造纵深。
 *
 * 算法与约束全部来自研读对象 `dsh-wallpaper-engine` 的 parallax-layer，逐条对照：
 *
 * 1. **口径**：层的目标位移 = `-1 × u × pct / 50`，`u = 光标 − 视口中心`，
 *    `pct` = 该层能达到的最大位移占屏幕对角线的百分比。
 *    （光标贴角时 |u| = 对角线/2 ⇒ 位移 = pct% × 对角线；少那个 ×2 会让位移翻倍。）
 *
 * 2. **必须写独立属性 `translate`，不能写 `transform`** —— 鲸鱼的漂浮动画内联写的是
 *    `transform`，独立属性才能与它叠加而不互相覆盖。
 *
 * 3. **每帧一个自定义属性都不写** —— 自定义属性是继承的，写一次整棵子树重算样式。
 *    每帧只把最终位移写进"要动的那几个元素自己"。
 *
 * 4. **界面组量化到整设备像素 + 迟滞** —— 文字底下是像素网格，落在半像素上会被重采样
 *    成糊的；缓动尾巴会在零点附近来回磨，没有迟滞就会两帧之间 0↔1 像素地翻来翻去。
 *
 * 5. **含 `position: fixed` 后代的元素整组不动** —— 只要写了 `translate`（哪怕 0px），
 *    这个元素就成了那些后代的包含块，会把它们从视口坐标拽到元素坐标。
 *    本项目的 `Modal` 是就地渲染的（没有 portal），所以这条必须真的检查、不能假定。
 *
 * 6. **归零时把 `translate` 整条摘掉**，而不是写 `translate: 0 0`。
 */

import type { ParallaxConfig } from './appearance';

/** 最大位移是"屏幕对角线的 p%"，除以 50 就是 u→位移的折算系数 */
const PARALLAX_STEP_DIV = 50;
/** 界面组的迟滞带宽（设备像素）：小于它的位移变化直接忽略 */
const PARALLAX_GROUP_DEAD_PX = 0.6;

interface Target {
  el: HTMLElement;
  /** 相对于壁纸组的幅度倍数 */
  factor: number;
  /** 是否量化到整设备像素（界面组 = true，图像层 = false） */
  quantize: boolean;
  /** 当前已写入的位移（设备像素），用于迟滞判断 */
  appliedX: number;
  appliedY: number;
}

export interface ParallaxHandle {
  stop(): void;
  /** 设置变化后重新收集目标（壁纸切换、页面切换都会改 DOM） */
  refresh(cfg: ParallaxConfig): void;
}

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    !!window.matchMedia &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  );
}

/** 目标元素里是否有 fixed 后代（有 ⇒ 不能给它写 translate） */
function hasFixedDescendant(el: HTMLElement): boolean {
  const nodes = el.querySelectorAll<HTMLElement>('*');
  for (const n of nodes) {
    if (window.getComputedStyle(n).position === 'fixed') return true;
  }
  return false;
}

export function startParallax(initial: ParallaxConfig): ParallaxHandle {
  let cfg = initial;
  const disabled = prefersReducedMotion();
  let targets: Target[] = [];
  let raf = 0;
  let curX = 0;
  let curY = 0;
  let tgtX = 0;
  let tgtY = 0;
  let running = false;

  const collect = () => {
    const next: Target[] = [];
    const add = (selector: string, factor: number, quantize: boolean) => {
      document.querySelectorAll<HTMLElement>(selector).forEach((el) => {
        if (hasFixedDescendant(el)) return;
        next.push({ el, factor, quantize, appliedX: 0, appliedY: 0 });
      });
    };
    // 壁纸：图像层不量化（量化反而会一顿一顿）
    add('.wp-rays', 0.55, false);
    add('.wp-bubbles', 0.8, false);
    add('.wp-whale', 1, false);
    add('.wp-aurora-wrap', 0.35, false);
    add('.wp-dust-wrap', 0.5, false);
    // 界面：侧栏与卡片，量化 + 迟滞（文字压在上面）
    add('.app-sidebar', 0.45, true);
    add('.card', 0.45, true);
    targets = next;
  };

  const apply = () => {
    const dpr = window.devicePixelRatio || 1;
    for (const t of targets) {
      // 位移 = pct% × 对角线 × (u / (对角线/2)) = pct × u / 50 —— 对角线在推导中消掉了，
      // 所以这里**不能**再乘对角线（乘了会让位移放大近千倍，实测鲸鱼被推出屏幕）。
      const pct = t.quantize ? cfg.ui : cfg.wallpaper;
      let x = (-1 * (tgtX * t.factor * pct)) / PARALLAX_STEP_DIV;
      let y = (-1 * (tgtY * t.factor * pct)) / PARALLAX_STEP_DIV;
      if (t.quantize) {
        const qx = Math.round(x * dpr) / dpr;
        const qy = Math.round(y * dpr) / dpr;
        // 迟滞：没跨过死区就不改，避免两帧之间在 0/1 像素间翻来翻去
        if (Math.abs(qx - t.appliedX) * dpr < PARALLAX_GROUP_DEAD_PX) x = t.appliedX;
        else x = qx;
        if (Math.abs(qy - t.appliedY) * dpr < PARALLAX_GROUP_DEAD_PX) y = t.appliedY;
        else y = qy;
      }
      if (Math.abs(x) < 0.01 && Math.abs(y) < 0.01) {
        // 归零要把整条属性摘掉：属性在就成立包含块
        if (t.appliedX !== 0 || t.appliedY !== 0) {
          t.el.style.removeProperty('translate');
          t.appliedX = 0;
          t.appliedY = 0;
        }
        continue;
      }
      t.el.style.translate = `${x.toFixed(2)}px ${y.toFixed(2)}px`;
      t.appliedX = x;
      t.appliedY = y;
    }
  };

  const frame = () => {
    raf = 0;
    // 指数逼近（按真实 dt 折算 ⇒ 掉帧时跟手程度不变）
    const k = Math.min(1, cfg.smoothing);
    curX += (tgtX - curX) * k;
    curY += (tgtY - curY) * k;
    apply();
    if (Math.abs(tgtX - curX) > 0.5 || Math.abs(tgtY - curY) > 0.5) {
      raf = requestAnimationFrame(frame);
    } else {
      running = false;
      curX = tgtX;
      curY = tgtY;
      apply();
    }
  };

  const kick = () => {
    if (!raf) raf = requestAnimationFrame(frame);
    running = true;
  };

  const onPointerMove = (e: PointerEvent) => {
    if (disabled || !cfg.enabled) return;
    tgtX = e.clientX - window.innerWidth / 2;
    tgtY = e.clientY - window.innerHeight / 2;
    kick();
  };

  const onLeave = () => {
    tgtX = 0;
    tgtY = 0;
    kick();
  };

  const onResize = () => {
    collect();
    apply();
  };

  window.addEventListener('pointermove', onPointerMove, { passive: true });
  window.addEventListener('pointerleave', onLeave, { passive: true });
  window.addEventListener('resize', onResize);
  collect();
  // 页面切换 / 骨架重建后目标会换一批：低频比对"选择器命中数"来决定要不要重收。
  // 为什么不只判 isConnected：App 首次渲染时还处于加载态（没有侧栏与卡片），
  // 那时收集到的是空集合，之后所有节点都是新增的 —— 只判旧节点是否掉线永远发现不了。
  const SELECTORS = [
    '.wp-rays',
    '.wp-bubbles',
    '.wp-whale',
    '.wp-aurora-wrap',
    '.wp-dust-wrap',
    '.app-sidebar',
    '.card',
  ];
  const countAll = () =>
    SELECTORS.reduce((n, sel) => n + document.querySelectorAll(sel).length, 0);
  let lastCount = countAll();
  const recheck = window.setInterval(() => {
    if (!cfg.enabled) return;
    const n = countAll();
    if (n !== lastCount || targets.some((t) => !t.el.isConnected)) {
      lastCount = n;
      collect();
      apply();
    }
  }, 1500);

  return {
    stop() {
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('pointerleave', onLeave);
      window.removeEventListener('resize', onResize);
      window.clearInterval(recheck);
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      for (const t of targets) t.el.style.removeProperty('translate');
      targets = [];
    },
    refresh(next) {
      cfg = next;
      collect();
      if (!cfg.enabled) {
        for (const t of targets) t.el.style.removeProperty('translate');
        if (raf) cancelAnimationFrame(raf);
        raf = 0;
        running = false;
        return;
      }
      apply();
    },
  };
}
