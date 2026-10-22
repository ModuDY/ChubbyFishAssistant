/**
 * 点击与拖尾光效（Canvas2D）。
 *
 * 性能口径全部照抄研读对象 `dsh-wallpaper-engine` 的 fx-layer 结论 —— 那里踩过的坑：
 *
 * 1. **`shadowBlur` 是最贵的一笔**：每次 `stroke()`/`fill()` 都要做一次离屏高斯模糊。
 *    原实现把它落在"每帧每图元"上，一帧上百次模糊。这里按**共享状态合批**：
 *    - 彗尾按**年龄带**把同带图元合成一条路径，只描一次（带数 = 性能档）；
 *    - 一簇星火 12 条共享同一套 alpha/颜色/线宽，合并成一条路径只描一次。
 *    合并的是**绘制调用**、不是形状 ⇒ 每条火花的角度与长度差异全部保留。
 *
 * 2. **帧耗时驱动的自适应降档**：连续慢帧降档、连续快帧升档，且升档阈值远大于降档
 *    （避抖），并且可回退 —— 只降不升等于一次抖动永久降级。
 *
 * 3. **rAF 内容驱动起停**：没有活粒子就停帧，空闲零成本；连 canvas 都不建。
 *
 * 4. 所有动效只画在独立 canvas 上，`pointer-events: none`，不碰 DOM 布局。
 */

import type { FxConfig } from './appearance';

const FX_DPR_MAX = 2;
const FX_POINT_MAX = 96;
const FX_CLICK_MAX = 24;
const FX_CLICK_LIFE_MS = 900;
const FX_TRAIL_STEP_PX = 3;
const FX_DUST_STEP_PX = 10;
const FX_SPARK_COUNT = 12;
/** 拖尾年龄带数：性能档 0/1/2 ⇒ 2/4/8 条路径（越少越省） */
const FX_TRAIL_BANDS_BY_LEVEL = [2, 4, 8];
const FX_TRAIL_LIFE_MS = 620;
const FX_SPARK_LIFE_MS = 620;
const FX_DUST_LIFE_MS = 1500;

interface Ring {
  x: number;
  y: number;
  born: number;
}
interface TrailPoint {
  x: number;
  y: number;
  born: number;
}
interface Spark {
  x: number;
  y: number;
  vx: number;
  vy: number;
  born: number;
  len: number;
}
interface Dust {
  x: number;
  y: number;
  vy: number;
  born: number;
  r: number;
}

export interface FxHandle {
  stop(): void;
  refresh(cfg: FxConfig): void;
}

function prefersReducedMotion(): boolean {
  return (
    typeof window !== 'undefined' &&
    !!window.matchMedia &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  );
}

export function startFx(initial: FxConfig): FxHandle {
  let cfg = initial;
  // 关掉 / 系统要求减弱动效 ⇒ 完全不建 canvas
  if (!cfg.enabled || prefersReducedMotion() || typeof document === 'undefined') {
    return {
      stop() {},
      refresh(next) {
        cfg = next;
      },
    };
  }

  const canvas = document.createElement('canvas');
  canvas.className = 'fx-canvas';
  canvas.setAttribute('aria-hidden', 'true');
  document.body.appendChild(canvas);
  const ctx = canvas.getContext('2d');

  let dpr = Math.min(window.devicePixelRatio || 1, FX_DPR_MAX);
  let w = 0;
  let h = 0;

  const rings: Ring[] = [];
  const trail: TrailPoint[] = [];
  const sparks: Spark[] = [];
  const dust: Dust[] = [];

  /** 性能档：0=高（8 带）、1=中（4）、2=低（2）；慢帧降、快帧升 */
  let perfLevel = 0;
  let slowFrames = 0;
  let fastFrames = 0;
  let raf = 0;
  let lastFrameAt = 0;
  let lastX = 0;
  let lastY = 0;
  let hasPointer = false;

  function resize() {
    dpr = Math.min(window.devicePixelRatio || 1, FX_DPR_MAX);
    w = window.innerWidth;
    h = window.innerHeight;
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    ctx?.setTransform(dpr, 0, 0, dpr, 0, 0);
  }
  resize();

  const now = () => performance.now();
  const scale = () => cfg.intensity;

  function onPointerDown(e: PointerEvent) {
    if (!cfg.enabled) return;
    const t = now();
    if (cfg.click) {
      if (rings.length < FX_CLICK_MAX) rings.push({ x: e.clientX, y: e.clientY, born: t });
    }
    if (cfg.sparks) {
      const n = Math.round(FX_SPARK_COUNT * scale());
      for (let i = 0; i < n; i += 1) {
        // 均匀角度 + 轻微随机长度，确定性由角度决定，观感是"迸射"而不是"抖动"
        const a = (Math.PI * 2 * i) / n + (i % 3) * 0.12;
        const sp = 90 + ((i * 37) % 60);
        sparks.push({
          x: e.clientX,
          y: e.clientY,
          vx: Math.cos(a) * sp,
          vy: Math.sin(a) * sp - 30,
          born: t,
          len: 8 + ((i * 13) % 10),
        });
      }
    }
    kick();
  }

  function onPointerMove(e: PointerEvent) {
    if (!cfg.enabled) return;
    if (!hasPointer) {
      hasPointer = true;
      lastX = e.clientX;
      lastY = e.clientY;
      return;
    }
    const dx = e.clientX - lastX;
    const dy = e.clientY - lastY;
    const dist = Math.hypot(dx, dy);
    if (cfg.trail && dist >= FX_TRAIL_STEP_PX && trail.length < FX_POINT_MAX) {
      trail.push({ x: e.clientX, y: e.clientY, born: now() });
    }
    if (cfg.dust && dist >= FX_DUST_STEP_PX && dust.length < FX_POINT_MAX * 2) {
      dust.push({
        x: e.clientX + (Math.random() - 0.5) * 18,
        y: e.clientY + (Math.random() - 0.5) * 18,
        vy: -6 - Math.random() * 10,
        born: now(),
        r: 0.8 + Math.random() * 1.4,
      });
    }
    lastX = e.clientX;
    lastY = e.clientY;
    kick();
  }

  function kick() {
    if (raf === 0) raf = requestAnimationFrame(frame);
  }

  function frame() {
    raf = 0;
    const t = now();
    const dt = lastFrameAt ? Math.min(t - lastFrameAt, 64) : 16;
    lastFrameAt = t;

    // 自适应降档：持续慢帧降一档，持续快帧（且久）升一档
    if (dt > 24) {
      slowFrames += 1;
      fastFrames = 0;
      if (slowFrames >= 6 && perfLevel < 2) {
        perfLevel += 1;
        slowFrames = 0;
      }
    } else if (dt < 14) {
      fastFrames += 1;
      slowFrames = 0;
      if (fastFrames >= 90 && perfLevel > 0) {
        perfLevel -= 1;
        fastFrames = 0;
      }
    }

    if (!ctx) return;
    ctx.clearRect(0, 0, w, h);
    ctx.globalCompositeOperation = 'lighter';

    const bands = FX_TRAIL_BANDS_BY_LEVEL[perfLevel];

    // ---- 浮尘：小圆点直接 fill（每个都很小，合批收益低） ----
    if (cfg.dust) {
      for (let i = dust.length - 1; i >= 0; i -= 1) {
        const d = dust[i];
        const age = t - d.born;
        if (age > FX_DUST_LIFE_MS) {
          dust.splice(i, 1);
          continue;
        }
        const k = age / FX_DUST_LIFE_MS;
        d.y += (d.vy * dt) / 1000;
        ctx.beginPath();
        ctx.globalAlpha = (1 - k) * 0.5;
        ctx.fillStyle = 'rgba(190, 225, 255, 0.9)';
        ctx.arc(d.x, d.y, d.r, 0, Math.PI * 2);
        ctx.fill();
      }
    }

    // ---- 彗尾：按年龄带合批，每带一条路径一次描边 ----
    if (cfg.trail) {
      for (let i = trail.length - 1; i >= 0; i -= 1) {
        if (t - trail[i].born > FX_TRAIL_LIFE_MS) trail.splice(i, 1);
      }
      if (trail.length >= 2) {
        const maxBand = Math.min(bands, trail.length) - 1;
        for (let band = 0; band <= maxBand; band += 1) {
          // 带号越大 = 越新（越靠近光标）
          const from = Math.floor((band * trail.length) / (maxBand + 1));
          const to = Math.floor(((band + 1) * trail.length) / (maxBand + 1));
          if (to - from < 2) continue;
          const k = (band + 1) / (maxBand + 1);
          ctx.beginPath();
          ctx.globalAlpha = 0.06 + k * 0.34;
          ctx.strokeStyle = 'rgba(150, 205, 255, 0.95)';
          ctx.lineWidth = 1 + k * 4;
          ctx.lineCap = 'round';
          ctx.lineJoin = 'round';
          ctx.moveTo(trail[from].x, trail[from].y);
          for (let i = from + 1; i < to; i += 1) ctx.lineTo(trail[i].x, trail[i].y);
          ctx.stroke();
        }
      }
    }

    // ---- 点击涟漪：两道扩散环 ----
    if (cfg.click) {
      for (let i = rings.length - 1; i >= 0; i -= 1) {
        const r = rings[i];
        const age = t - r.born;
        if (age > FX_CLICK_LIFE_MS) {
          rings.splice(i, 1);
          continue;
        }
        const k = age / FX_CLICK_LIFE_MS;
        const radius = (12 + k * 74) * scale();
        ctx.beginPath();
        ctx.globalAlpha = (1 - k) * 0.7;
        ctx.strokeStyle = 'rgba(160, 210, 255, 0.9)';
        ctx.lineWidth = 2 * (1 - k) + 0.4;
        ctx.arc(r.x, r.y, radius, 0, Math.PI * 2);
        ctx.stroke();
        // 内圈：反向相位，视觉上像"爆闪后回缩"
        ctx.beginPath();
        ctx.globalAlpha = (1 - k) * 0.35;
        ctx.arc(r.x, r.y, radius * 0.55, 0, Math.PI * 2);
        ctx.stroke();
      }
    }

    // ---- 星火：12 条共享同一套样式 ⇒ 合成一条路径只描一次 ----
    if (cfg.sparks && sparks.length) {
      ctx.beginPath();
      ctx.globalAlpha = 0.8;
      ctx.strokeStyle = 'rgba(200, 232, 255, 0.95)';
      ctx.lineWidth = 1.4;
      ctx.lineCap = 'round';
      for (let i = sparks.length - 1; i >= 0; i -= 1) {
        const s = sparks[i];
        const age = t - s.born;
        if (age > FX_SPARK_LIFE_MS) {
          sparks.splice(i, 1);
          continue;
        }
        const k = age / FX_SPARK_LIFE_MS;
        // 阻尼 + 轻微重力
        const px = s.x + (s.vx * age) / 1000;
        const py = s.y + (s.vy * age) / 1000 + 0.0005 * age * age * 0.5;
        const nx = px - (s.vx / 1000) * s.len * (1 - k);
        const ny = py - (s.vy / 1000) * s.len * (1 - k);
        ctx.moveTo(nx, ny);
        ctx.lineTo(px, py);
      }
      ctx.stroke();
    }

    ctx.globalAlpha = 1;
    ctx.globalCompositeOperation = 'source-over';

    // 还有活粒子就续帧，否则彻底停下（空闲零成本）
    const alive =
      rings.length > 0 || trail.length > 1 || sparks.length > 0 || dust.length > 0;
    if (alive) kick();
    else lastFrameAt = 0;
  }

  window.addEventListener('pointerdown', onPointerDown, { passive: true });
  window.addEventListener('pointermove', onPointerMove, { passive: true });
  window.addEventListener('resize', resize);

  return {
    stop() {
      window.removeEventListener('pointerdown', onPointerDown);
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('resize', resize);
      if (raf) cancelAnimationFrame(raf);
      raf = 0;
      canvas.remove();
    },
    refresh(next) {
      cfg = next;
      // 关掉时把画布清空并停帧（不销毁，重新打开无需重挂）
      if (!cfg.enabled && ctx) {
        ctx.clearRect(0, 0, w, h);
        rings.length = 0;
        trail.length = 0;
        sparks.length = 0;
        dust.length = 0;
      }
    },
  };
}
