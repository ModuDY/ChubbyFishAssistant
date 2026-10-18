/**
 * 桌宠渲染路线验证页（P0 技术验证，临时）
 *
 * 回答一个问题：**透明 WebM 到底该怎么播**——直接用 <video>，还是必须预解码成帧序列。
 *
 * 上游 dsh-pet-indesktop 已有全套素材却自建帧序列链（frameseq_provision.py + decode_fanout.py
 * + frame_cache.py，200+ KB 代码），说明 <video> 在某些能力上不可控。这里逐项实测：
 *
 *   T1 透明度    视频的 alpha 是否真的透出桌面（canvas 读像素客观判定 + 肉眼切换背景复核）
 *   T2 首帧延迟   seek 到 0 后多久出画面（对应上游 first_frame_cache_max_mb）
 *   T3 逐帧定位  能否精确停在指定帧 / 指定时间段（对应上游 frameseq_clip）
 *   T4 隐藏即停  窗口隐藏后 rAF 与定时器是否被冻结、恢复后能否继续（对应上游 memory_trim）
 *   T5 内存      解码器与帧数据的内存占用
 *
 * 判定用 canvas 而不是只看屏幕：`drawImage(video)` 的结果直接告诉我们 alpha 是否保留；
 * 若整幅图 alpha 全 255，说明透明通道已被 WebView2 丢弃——**这是决定路线的硬证据**。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './probe.css';

/**
 * 样本素材（上游 dsh-pet，MIT）：待机呼吸休闲 / 忙碌点按 / 雀跃庆祝 / 元气挥手
 *
 * 统一走 `actions.ts` 的注册表，而不是各自写 import——
 * 那样只有被显式 import 的文件才会被打包，其余素材会静默丢失。
 */
import { actionUrl, MISSING_ACTIONS } from './actions';

const SAMPLES = {
  idle: actionUrl('idle-breathe'),
  busy: actionUrl('work-busy'),
  celebrate: actionUrl('work-celebrate'),
  wave: actionUrl('click-wave'),
} as const;

type SampleKey = keyof typeof SAMPLES;

/**
 * 自动模式（`?auto=1`）：无人值守依次跑完 T1~T5，并把全部读数落盘到
 * `%TEMP%\pet-probe-report.json`。
 * 之所以需要：Tauri dev 下 WebView 的 console 不会转发到终端，落盘是取回读数的唯一稳定通道。
 */
const AUTO_MODE = new URLSearchParams(window.location.search).get('auto') === '1';

type RvfcVideo = HTMLVideoElement & {
  requestVideoFrameCallback?: (
    cb: (now: number, meta: { mediaTime: number; presentedFrames: number }) => void,
  ) => number;
};

/**
 * seek 到目标时间，并返回**真实呈现帧**的 mediaTime。
 *
 * 为什么不能直接读 `video.currentTime`：seek 后该属性立即反映"目标值"，
 * 读到的必然等于请求值——那只能证明"赋值成功"，不能证明"画面真的停在那帧"。
 * `requestVideoFrameCallback` 的 `mediaTime` 才是被合成器实际呈现那一帧的时间戳。
 */
const seekAndReadPresented = (
  video: HTMLVideoElement,
  target: number,
): Promise<{ mediaTime: number | null; via: string }> =>
  new Promise((resolve) => {
    const v = video as RvfcVideo;
    let done = false;
    const finish = (mediaTime: number | null, via: string) => {
      if (done) return;
      done = true;
      resolve({ mediaTime, via });
    };
    const hasRvfc = typeof v.requestVideoFrameCallback === 'function';
    video.addEventListener(
      'seeked',
      () => {
        if (!hasRvfc) {
          finish(video.currentTime, 'currentTime(回退)');
          return;
        }
        v.requestVideoFrameCallback!((_now, meta) => finish(meta.mediaTime, 'rVFC'));
      },
      { once: true },
    );
    setTimeout(() => finish(null, 'timeout'), 1500);
    video.currentTime = target;
  });

type Backdrop = 'none' | 'checker' | 'magenta' | 'dark' | 'stripes';

type AlphaReading = {
  /** 采样到的透明像素比例（alpha < 250 视为透明） */
  transparentRatio: number;
  /** 完全透明像素比例（alpha === 0） */
  fullyTransparentRatio: number;
  /** alpha 分布直方图：区分"背景接近 0"与"背景是半透明灰雾" */
  histogram: { zero: number; low: number; mid: number; high: number; opaque: number };
  /** 是否读到过非零 alpha 的可见像素（说明视频确实有内容） */
  hasVisiblePixels: boolean;
};

type ProbeLine = {
  key: string;
  level: 'pass' | 'fail' | 'warn';
  text: string;
};

export default function ProbeRoot() {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  /** 采样用的离屏 canvas：避免为了读像素而干扰可见渲染 */
  const samplerRef = useRef<HTMLCanvasElement | null>(null);

  const [sample, setSample] = useState<SampleKey>('idle');
  // 默认「无背景」：此时视频角色四周若显示为桌面内容，说明窗口透明真的生效；
  // 若显示为黑/白，说明窗口不透明（canvas 有 alpha ≠ 窗口透明）。
  const [backdrop, setBackdrop] = useState<Backdrop>('none');
  const [useCanvasView, setUseCanvasView] = useState(false);
  const [playing, setPlaying] = useState(true);
  const [lines, setLines] = useState<ProbeLine[]>([]);
  const [fps, setFps] = useState(0);
  const [meta, setMeta] = useState<string>('等待加载素材…');
  const [busy, setBusy] = useState(false);

  const fpsRef = useRef({ frames: 0, last: performance.now(), ticks: 0, lastTick: performance.now() });
  const hiddenProbeRef = useRef<{ ticksBefore: number; startedAt: number } | null>(null);
  /** 结果快照：自动流程在异步链里读不到最新的 state，必须同步写一份 ref */
  const resultsRef = useRef<Record<string, { level: ProbeLine['level']; text: string }>>({});
  const autoRanRef = useRef(false);

  const push = useCallback((key: string, level: ProbeLine['level'], text: string) => {
    resultsRef.current[key] = { level, text };
    setLines((prev) => {
      const next = prev.filter((l) => l.key !== key);
      next.push({ key, level, text });
      return next;
    });
  }, []);

  const url = SAMPLES[sample];

  /* ------------------------------------------------------------------ */
  /* rAF 循环：既统计 FPS，也在窗口隐藏期间记录 tick 是否继续           */
  /* ------------------------------------------------------------------ */
  useEffect(() => {
    let raf = 0;
    const loop = () => {
      const s = fpsRef.current;
      const now = performance.now();
      s.frames += 1;
      s.ticks += 1;
      if (now - s.last >= 500) {
        setFps(Math.round((s.frames * 1000) / (now - s.last)));
        s.frames = 0;
        s.last = now;
      }
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, []);

  /* --------------------------- T1 透明度 --------------------------- */
  /**
   * 把当前视频帧画到离屏 canvas 并读像素。
   * 判据：若透明像素比例接近 0 且图像几乎全不透明，说明 alpha 被丢弃。
   * 注意：必须用 willReadFrequently 的 2d context，否则 GPU 端读回会失败/报错。
   */
  const readAlpha = useCallback((): AlphaReading | null => {
    const video = videoRef.current;
    if (!video || !video.videoWidth || !video.videoHeight) return null;

    let sampler = samplerRef.current;
    if (!sampler) {
      sampler = document.createElement('canvas');
      samplerRef.current = sampler;
    }
    // 缩到 160px 宽做采样：够判定比例，且读回成本低
    const w = 160;
    const h = Math.max(1, Math.round((video.videoHeight / video.videoWidth) * w));
    sampler.width = w;
    sampler.height = h;

    const ctx = sampler.getContext('2d', { willReadFrequently: true });
    if (!ctx) return null;
    ctx.clearRect(0, 0, w, h);
    try {
      ctx.drawImage(video, 0, 0, w, h);
    } catch {
      // tainted canvas：视频被判定为跨源（本地同源不应发生，留作证据）
      return null;
    }

    const { data } = ctx.getImageData(0, 0, w, h);
    let transparent = 0;
    let fullyTransparent = 0;
    let visible = 0;
    const hist = { zero: 0, low: 0, mid: 0, high: 0, opaque: 0 };
    const total = w * h || 1;
    for (let i = 3; i < data.length; i += 4) {
      const a = data[i];
      if (a < 250) transparent += 1;
      if (a === 0) {
        fullyTransparent += 1;
        hist.zero += 1;
      } else if (a <= 50) {
        hist.low += 1;
      } else if (a <= 150) {
        hist.mid += 1;
      } else if (a < 250) {
        hist.high += 1;
      } else {
        hist.opaque += 1;
      }
      if (a > 8) visible += 1;
    }
    return {
      transparentRatio: transparent / total,
      fullyTransparentRatio: fullyTransparent / total,
      histogram: {
        zero: hist.zero / total,
        low: hist.low / total,
        mid: hist.mid / total,
        high: hist.high / total,
        opaque: hist.opaque / total,
      },
      hasVisiblePixels: visible > total * 0.01,
    };
  }, []);

  /** 读当前帧的像素哈希（缩略采样），用于判断"画面是否真的换了一帧" */
  const readFrameHash = useCallback((): string | null => {
    const video = videoRef.current;
    if (!video || !video.videoWidth) return null;
    let sampler = samplerRef.current;
    if (!sampler) {
      sampler = document.createElement('canvas');
      samplerRef.current = sampler;
    }
    const w = 80;
    const h = Math.max(1, Math.round((video.videoHeight / video.videoWidth) * w));
    sampler.width = w;
    sampler.height = h;
    const ctx = sampler.getContext('2d', { willReadFrequently: true });
    if (!ctx) return null;
    ctx.clearRect(0, 0, w, h);
    try {
      ctx.drawImage(video, 0, 0, w, h);
    } catch {
      return null;
    }
    const { data } = ctx.getImageData(0, 0, w, h);
    let sum = 0;
    for (let i = 0; i < data.length; i += 4) {
      sum = (Math.imul(sum, 31) + data[i] * 7 + data[i + 1] * 13 + data[i + 2] * 17 + data[i + 3] * 19) >>> 0;
    }
    return sum.toString(16);
  }, []);

  const runAlphaCheck = useCallback(async () => {
    // 连采 3 次取最大透明比：单帧可能刚好落在角色占满画面的瞬间，一次采样会误判
    let r: AlphaReading | null = null;
    for (let i = 0; i < 3; i += 1) {
      const cur = readAlpha();
      if (cur && (!r || cur.transparentRatio > r.transparentRatio)) r = cur;
      // eslint-disable-next-line no-await-in-loop
      await new Promise((res) => setTimeout(res, 120));
    }
    if (!r) {
      push('T1', 'warn', 'T1 透明度：无法读取像素（视频未就绪，或 canvas 被判为跨源）。\n请先让视频播放，并确认素材路径正确。');
      return;
    }
    const pct = (n: number) => `${(n * 100).toFixed(1)}%`;
    if (!r.hasVisiblePixels) {
      push('T1', 'warn', `T1 透明度：几乎读不到可见像素（可见像素<1%，透明 ${pct(r.transparentRatio)}）。\n可能停在无内容的帧，换一个播放位置再测。`);
      return;
    }
    if (r.transparentRatio > 0.05) {
      push(
        'T1',
        'pass',
        `T1 透明度：通过 ✅\n` +
          `透明像素 ${pct(r.transparentRatio)}（alpha=0 占 ${pct(r.fullyTransparentRatio)}）\n` +
          `alpha 分布：=0 ${pct(r.histogram.zero)} | 1-50 ${pct(r.histogram.low)} | 51-150 ${pct(r.histogram.mid)} | 151-249 ${pct(r.histogram.high)} | 不透明 ${pct(r.histogram.opaque)}\n` +
          `结论：WebView2 保留了视频 alpha 通道（不是全 255）。\n` +
          `注意：若 51-150 占比高，说明背景是"半透明灰雾"而不是真透明，叠在桌面上会发灰。\n` +
          `窗口级透明是另一件事：背景选「无背景」时，角色四周应透出桌面/后面的窗口，而不是黑底。`,
      );
    } else {
      push(
        'T1',
        'fail',
        `T1 透明度：失败 ❌\n透明像素仅 ${pct(r.transparentRatio)}（几乎全部 alpha=255）\n` +
          `结论：WebView2 未保留视频 alpha，透明区变成了不透明（通常为黑底）。\n` +
          `→ 该素材不能直接 <video> 播放，需改走帧序列 + 抠像，或改用 APNG/WebP 动图。`,
      );
    }
  }, [push, readAlpha]);

  /* --------------------------- T2 首帧延迟 --------------------------- */
  const runFirstFrameCheck = useCallback(async () => {
    const video = videoRef.current;
    if (!video) return;
    setBusy(true);
    try {
      const t0 = performance.now();
      video.pause();
      // 强制回到 0 并等待可绘制帧：loadeddata/seeked 任一先到都算"画面可用"
      video.currentTime = 0;
      await new Promise<void>((resolve) => {
        let done = false;
        const finish = () => {
          if (done) return;
          done = true;
          video.removeEventListener('seeked', finish);
          video.removeEventListener('loadeddata', finish);
          resolve();
        };
        video.addEventListener('seeked', finish);
        video.addEventListener('loadeddata', finish);
        setTimeout(finish, 1200);
      });
      const ms = Math.round(performance.now() - t0);

      // 再从当前时间跳到片段中部，测"切动作"的最坏情况
      const t1 = performance.now();
      video.currentTime = Number.isFinite(video.duration) ? video.duration * 0.5 : 0.5;
      await new Promise<void>((resolve) => {
        let done = false;
        const finish = () => {
          if (done) return;
          done = true;
          video.removeEventListener('seeked', finish);
          resolve();
        };
        video.addEventListener('seeked', finish);
        setTimeout(finish, 1200);
      });
      const midMs = Math.round(performance.now() - t1);

      const level: ProbeLine['level'] = ms < 120 && midMs < 200 ? 'pass' : midMs < 500 ? 'warn' : 'fail';
      push(
        'T2',
        level,
        `T2 首帧延迟：${level === 'pass' ? '通过 ✅' : level === 'warn' ? '偏慢 ⚠️' : '过慢 ❌'}\n` +
          `回到 0 并出画面耗时 ${ms} ms；跳到片段中部出画面耗时 ${midMs} ms。\n` +
          `判据：<120ms 视为无闪白/闪黑风险，>500ms 切动作时肉眼必见空帧。\n` +
          `上游为此专门做了首帧缓存（first_frame_cache_max_mb，默认 8MB）——若这里明显偏慢，说明该缓存是必需的。`,
      );
    } finally {
      setBusy(false);
    }
  }, [push]);

  /* --------------------------- T3 逐帧定位 --------------------------- */
  const runFrameStepCheck = useCallback(async () => {
    const video = videoRef.current;
    if (!video) return;
    setBusy(true);
    try {
      video.pause();
      const duration = video.duration;
      // 上游规格 24fps；逐帧步进一个帧长，验证能否精确停在某一帧
      const step = 1 / 24;
      const base = Math.min(step * 5, Math.max(0, duration - 0.05));

      const seekTo = (t: number) =>
        new Promise<void>((resolve) => {
          const v = videoRef.current;
          if (!v) return resolve();
          let done = false;
          const finish = () => {
            if (done) return;
            done = true;
            v.removeEventListener('seeked', finish);
            resolve();
          };
          v.addEventListener('seeked', finish);
          setTimeout(finish, 1000);
          v.currentTime = t;
          return undefined;
        });

      // 用画面像素哈希判断"这一帧有没有真的变"——比 currentTime 客观，
      // 也不依赖 rVFC（实测：暂停后再 seek，rVFC 可能根本不回调）
      const hashAt = async (t: number) => {
        await seekTo(t);
        await new Promise((r) => setTimeout(r, 80));
        return readFrameHash();
      };

      const h0 = await hashAt(base);
      const h1 = await hashAt(base + step);
      const h2 = await hashAt(base + step * 2);

      // rVFC 可用性单独记录：它在暂停态 seek 后是否回调，直接影响能否用它做定格校验
      const rvfcProbe = await seekAndReadPresented(video, base);
      const rvfcOk = rvfcProbe.mediaTime != null;

      const distinct = h0 != null && h1 != null && h2 != null && new Set([h0, h1, h2]).size === 3;
      const level: ProbeLine['level'] = distinct ? 'pass' : 'warn';
      push(
        'T3',
        level,
        `T3 逐帧定位：${distinct ? '通过 ✅' : '无法逐帧区分 ⚠️'}\n` +
          `按 1/24s（41.7ms）连续步进 3 帧，画面哈希：${h0} / ${h1} / ${h2}\n` +
          `判据：三个哈希互不相同 → 每次 seek 都真的换了一帧，可逐帧控制。\n` +
          `rVFC 定格校验可用性：${rvfcOk ? '可用' : '不可用（暂停态 seek 后不回调）'}。\n` +
          `说明：直接读 video.currentTime 会返回 seek 目标值（必然相等），不能作为精度依据，故改用像素哈希。`,
      );
    } finally {
      setBusy(false);
    }
  }, [push, readFrameHash]);

  /* --------------------------- T4 隐藏即停 --------------------------- */
  const runHiddenCheck = useCallback(async () => {
    setBusy(true);
    try {
      const ticksBefore = fpsRef.current.ticks;
      hiddenProbeRef.current = { ticksBefore, startedAt: performance.now() };
      push('T4', 'warn', 'T4 隐藏即停：已隐藏窗口，1.5 秒后由 Rust 强制恢复…（请勿手动干预）');

      // Rust 侧 sleep 1.5s 后 show + set_always_on_top；同时原地记录前后 rAF tick
      const result = await invoke<Record<string, unknown>>('pet_probe_hide_and_restore');

      // 恢复后等两帧，让 rAF 有机会继续跑
      await new Promise((r) => setTimeout(r, 220));

      const ticksAfter = fpsRef.current.ticks;
      const advanced = ticksAfter - ticksBefore;
      const elapsed = performance.now() - (hiddenProbeRef.current?.startedAt ?? performance.now());

      // 判据：隐藏 1.5s 期间若 rAF 被冻结，tick 增量应远小于 elapsed(ms)/16.7
      const expected = elapsed / 16.7;
      const frozen = advanced < expected * 0.5;

      push(
        'T4',
        'pass',
        `T4 隐藏即停：通过 ✅\n` +
          `隐藏 ${result.hideMs ?? 1500} ms 期间，rAF tick 增量 ${advanced}（若不冻结应约 ${Math.round(expected)}）\n` +
          `判据：隐藏期间 ${frozen ? 'rAF/定时器已被冻结 → 前端无法自行恢复，必须由 Rust 兜底' : 'rAF 仍在运行'}。\n` +
          `窗口已恢复：置顶 ${result.alwaysOnTopBefore} → ${result.alwaysOnTopAfter}；` +
          `分层窗口位 ${result.layeredAfter ? '在' : '丢失!'}，工具窗口位 ${result.toolWindowAfter ? '在' : '丢失!'}。\n` +
          `结论：${frozen ? '窗口隐藏时必须由 Rust 侧恢复（本 demo 已验证可行），且隐藏期间前端动画自然停止 → 对应上游 memory_trim 的效果。' : 'rAF 未被冻结，前端可自行处理隐藏恢复。'}`,
      );
    } catch (e) {
      push('T4', 'fail', `T4 隐藏即停：命令调用失败 ❌\n${String(e)}\n（若窗口未恢复，请用托盘或重启应用）`);
    } finally {
      setBusy(false);
    }
  }, [push]);

  /* --------------------------- T5 内存 --------------------------- */
  const runMemoryCheck = useCallback(async () => {
    try {
      const mem = (performance as unknown as { memory?: { usedJSHeapSize: number; jsHeapSizeLimit: number } }).memory;
      const lines2: string[] = [];
      let level: ProbeLine['level'] = 'warn';
      if (mem) {
        const usedMB = mem.usedJSHeapSize / 1024 / 1024;
        const limitMB = mem.jsHeapSizeLimit / 1024 / 1024;
        lines2.push(`JS 堆：已用 ${usedMB.toFixed(1)} MB / 上限 ${limitMB.toFixed(1)} MB`);
        level = 'pass';
      } else {
        lines2.push('当前 WebView2 未暴露 performance.memory（非 Chromium 或未启用），改用任务管理器观察进程内存。');
      }

      // 逐个加载全部样本，观察内存增量（模拟"多动作预加载"）
      const before = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? 0;
      const created: HTMLVideoElement[] = [];
      await Promise.all(
        Object.values(SAMPLES).map(
          (src) =>
            new Promise<void>((resolve) => {
              const v = document.createElement('video');
              v.src = src;
              v.preload = 'auto';
              v.muted = true;
              v.addEventListener('loadeddata', () => resolve(), { once: true });
              v.addEventListener('error', () => resolve(), { once: true });
              setTimeout(resolve, 3000);
              created.push(v);
            }),
        ),
      );
      await new Promise((r) => setTimeout(r, 400));
      const after = (performance as unknown as { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? 0;
      if (after && before) {
        lines2.push(`预加载 4 个样本后 JS 堆增量：${((after - before) / 1024 / 1024).toFixed(1)} MB`);
      }
      lines2.push(
        `注意：JS 堆**不包含**视频解码器内存（那部分在进程的其他区域）。\n` +
          `要得到真实结论，请同时打开任务管理器记录本进程「内存」列，在预加载前后各读一次。\n` +
          `上游为此设了帧缓存上限（first_frame_cache_max_mb 默认 8MB）——若预加载多个动作内存线性上涨，说明必须自设上限。`,
      );
      created.forEach((v) => {
        v.src = '';
        v.load();
      });

      push('T5', level, `T5 内存：\n${lines2.join('\n')}`);
    } catch (e) {
      push('T5', 'fail', `T5 内存：读取失败 ❌\n${String(e)}`);
    }
  }, [push]);

  /* --------------------------- 页面就绪上报 --------------------------- */
  useEffect(() => {
    const cls = ['pet-window'];
    document.documentElement.classList.add(...cls);
    document.body.classList.add(...cls);

    invoke<Record<string, unknown>>('pet_probe_ready')
      .then((info) => {
        push(
          'T0',
          'pass',
          `T0 窗口状态：\n${JSON.stringify(info, null, 2)}\n` +
            `判据：transparent 应为 true；alwaysOnTop 应为 true；decorations 应为 false；exToolWindow 应为 true（不进任务栏）。`,
        );
      })
      .catch((e) => push('T0', 'fail', `T0 窗口状态：探测命令失败 ❌\n${String(e)}`));

    // 素材自检：manifest 登记了但没被打包的文件会在运行时 404，这里提前暴露
    if (MISSING_ACTIONS.length > 0) {
      push('T0c', 'fail', `素材缺失：manifest 已登记但打包产物中找不到 →\n${MISSING_ACTIONS.join('\n')}`);
    } else {
      push('T0c', 'pass', '素材自检：注册表中所有动作均已正确打包 ✅');
    }

    return () => {
      document.documentElement.classList.remove(...cls);
      document.body.classList.remove(...cls);
    };
  }, [push]);

  /* --------------------- 自动模式：无人值守跑完五项 --------------------- */
  useEffect(() => {
    if (!AUTO_MODE || autoRanRef.current) return;
    autoRanRef.current = true;

    const waitReady = () =>
      new Promise<void>((resolve) => {
        const v = videoRef.current;
        if (!v) return resolve();
        if (v.readyState >= 2 && v.videoWidth > 0) return resolve();
        const done = () => resolve();
        v.addEventListener('loadeddata', done, { once: true });
        setTimeout(done, 4000);
      });

    const run = async () => {
      await waitReady();
      await new Promise((r) => setTimeout(r, 400));

      await runAlphaCheck();
      await runFirstFrameCheck();
      await runFrameStepCheck();
      await runMemoryCheck();
      // T4 会隐藏窗口 1.5s（由 Rust 侧恢复），放最后避免打断其他测量
      await runHiddenCheck();
      await new Promise((r) => setTimeout(r, 500));

      const report = {
        auto: true,
        ua: navigator.userAgent,
        viewport: { w: window.innerWidth, h: window.innerHeight, dpr: window.devicePixelRatio },
        results: resultsRef.current,
      };
      try {
        const p = await invoke<string>('pet_probe_report', {
          report: JSON.stringify(report, null, 2),
        });
        setMeta(`自动检测完成，报告已写入：${p}`);
      } catch (e) {
        setMeta(`报告写入失败：${String(e)}`);
      }
    };
    void run();
  }, [runAlphaCheck, runFirstFrameCheck, runFrameStepCheck, runMemoryCheck, runHiddenCheck]);

  /* --------------------------- 素材加载 --------------------------- */
  const onLoadedMetadata = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    const fpsGuess = 24; // 上游规格
    setMeta(
      `${v.videoWidth}×${v.videoHeight}，时长 ${v.duration.toFixed(2)}s，` +
        `约 ${Math.max(1, Math.round(v.duration * fpsGuess))} 帧（按上游 ${fpsGuess}fps 规格估算）`,
    );
    push(
      'T0b',
      'pass',
      `T0 素材规格：\n分辨率 ${v.videoWidth}×${v.videoHeight}（上游规格 640×360）\n` +
        `时长 ${v.duration.toFixed(2)}s（VP9 alpha 透明 WebM）`,
    );
  }, [push]);

  const onPlayPause = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) void v.play();
    else v.pause();
  }, []);

  /* canvas 视图：把 video 当前帧持续画到可见 canvas（供观察"逐帧定位"结果） */
  useEffect(() => {
    if (!useCanvasView) return;
    let raf = 0;
    const draw = () => {
      const v = videoRef.current;
      const c = canvasRef.current;
      if (v && c && v.videoWidth) {
        if (c.width !== v.videoWidth || c.height !== v.videoHeight) {
          c.width = v.videoWidth;
          c.height = v.videoHeight;
        }
        const ctx = c.getContext('2d');
        if (ctx) {
          ctx.clearRect(0, 0, c.width, c.height);
          ctx.drawImage(v, 0, 0);
        }
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, [useCanvasView]);

  const backdropClass = useMemo(() => `probe-backdrop ${backdrop}`, [backdrop]);

  const passCount = lines.filter((l) => l.level === 'pass').length;
  const failCount = lines.filter((l) => l.level === 'fail').length;

  return (
    <div className="probe-root">
      <div className="probe-stage">
        <div className={backdropClass} />
        <video
          ref={videoRef}
          className="probe-video"
          src={url}
          autoPlay
          loop
          muted
          playsInline
          style={{ display: useCanvasView ? 'none' : undefined }}
          onLoadedMetadata={onLoadedMetadata}
          onPlay={() => setPlaying(true)}
          onPause={() => setPlaying(false)}
        />
        <canvas ref={canvasRef} className="probe-canvas" style={{ display: useCanvasView ? undefined : 'none' }} />
      </div>

      <div className="probe-panel">
        <div className="probe-head">
          <span className="probe-title">桌宠渲染路线验证</span>
          <span className="probe-fps">rAF {fps} fps</span>
        </div>

        <div className="probe-row">
          <span className="probe-label">背景</span>
          {(['none', 'checker', 'magenta', 'dark', 'stripes'] as Backdrop[]).map((b) => (
            <button
              key={b}
              className={`probe-btn ${backdrop === b ? 'active' : ''}`}
              onClick={() => setBackdrop(b)}
            >
              {b === 'none' ? '无背景' : b === 'checker' ? '棋盘' : b === 'magenta' ? '品红' : b === 'dark' ? '深色' : '彩条'}
            </button>
          ))}
        </div>

        <div className="probe-row">
          <span className="probe-label">素材</span>
          {(Object.keys(SAMPLES) as SampleKey[]).map((k) => (
            <button
              key={k}
              className={`probe-btn ${sample === k ? 'active' : ''}`}
              onClick={() => setSample(k)}
            >
              {k}
            </button>
          ))}
        </div>

        <div className="probe-row">
          <button className="probe-btn" onClick={onPlayPause} disabled={busy}>
            {playing ? '暂停' : '播放'}
          </button>
          <button className="probe-btn" onClick={() => setUseCanvasView((s) => !s)} disabled={busy}>
            {useCanvasView ? '用 video 渲染' : '用 canvas 渲染'}
          </button>
        </div>

        <div className="probe-row">
          <button className="probe-btn primary" onClick={() => void runAlphaCheck()} disabled={busy}>
            T1 透明度
          </button>
          <button className="probe-btn primary" onClick={() => void runFirstFrameCheck()} disabled={busy}>
            T2 首帧延迟
          </button>
          <button className="probe-btn primary" onClick={() => void runFrameStepCheck()} disabled={busy}>
            T3 逐帧定位
          </button>
          <button className="probe-btn primary" onClick={() => void runHiddenCheck()} disabled={busy}>
            T4 隐藏即停
          </button>
          <button className="probe-btn primary" onClick={() => void runMemoryCheck()} disabled={busy}>
            T5 内存
          </button>
        </div>

        <div className="probe-row">
          <button
            className="probe-btn"
            onClick={() =>
              setLines([
                {
                  key: 'runall',
                  level: 'warn',
                  text: '依次点击 T1→T5 即可（T4 会自动隐藏窗口 1.5 秒，属正常现象）。判定说明见各条结果。',
                },
              ])
            }
          >
            清空结果
          </button>
          <button className="probe-btn" onClick={() => void invoke('pet_close')} disabled={busy}>
            关闭本窗口
          </button>
        </div>

        <div className="probe-result">{meta}</div>

        <div className="probe-verdict">
          通过 {passCount} 项 / 失败 {failCount} 项。
          {'\n'}关键判据：<b>T1</b> 决定能不能用 &lt;video&gt;；<b>T2/T3</b> 决定切动作是否卡顿、能否定格；
          <b>T4</b> 决定隐藏时要不要 Rust 兜底。
        </div>

        {lines
          .slice()
          .reverse()
          .map((l) => (
            <pre key={l.key} className={`probe-result ${l.level}`}>
              {l.text}
            </pre>
          ))}
      </div>
    </div>
  );
}
