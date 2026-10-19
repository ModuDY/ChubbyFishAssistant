/**
 * 精灵渲染：一个 `<video>` 播 WebM(VP9 + alpha)。
 *
 * 为什么不用帧序列 + Canvas（上游做法）：本机实测 WebView2 完整保留了视频 alpha
 * （透明像素约 84.5%）、切动作出画面 <120ms，`<video>` 已经够用；帧序列要多维护
 * 预解码/缓存/内存上限三套逻辑，收益不足以抵消复杂度（见方案 §8.3）。
 *
 * **隐藏即停**：窗口不可见时 WebView 的 rAF/定时器可能被冻结，但解码器未必停。
 * 这里由父组件传 `paused`，窗口隐藏时显式 `pause()`，避免后台满速空转吃 CPU。
 *
 * **角色包围盒**：视频画布 640×360，角色只占中间一块，四周全是近透明像素。
 * 首帧解码后采样 alpha 求包围盒反向映射到窗口百分比，交给父组件做「点击热区」与
 * 「气泡锚点」——否则热区等于整个窗口，点空白也会触发动作，气泡也会飘在窗口顶部。
 */
import { useCallback, useEffect, useRef } from 'react';
import { actionUrl } from './actions';
import { isLoopAction } from './petState';

/** 角色在**窗口坐标系**中的可见范围（百分比，0~100） */
export type SpriteBox = {
  left: number;
  width: number;
  top: number;
  height: number;
};

type Props = {
  actionId: string;
  /** 同动作重播时递增（点击回应连续触发时也能从头播） */
  playKey: number;
  paused: boolean;
  onEnded: () => void;
  /** 首帧角色包围盒（只上报一次；测量失败时父组件用保守默认值） */
  onMeasure?: (box: SpriteBox) => void;
  /** 当前帧的「可点击格子」掩码（每次换动作重新采样，供 Rust 侧做透明区点击穿透） */
  onMask?: (cells: Uint8Array) => void;
};

/** 采样分辨率：160×90 足够定位包围盒，又不会在读回像素上花时间 */
const SAMPLE_W = 160;
const SAMPLE_H = 90;
/** alpha 阈值：素材背景残留 alpha 集中在 1~50，取 64 把背景彻底排除 */
const ALPHA_THRESHOLD = 64;
/** 点击穿透掩码的网格尺寸（窗口百分比网格）：格子越细贴合越好，代价是 IPC 字节数 */
export const MASK_COLS = 24;
export const MASK_ROWS = 28;

/**
 * 逐帧采样：一次 drawImage 同时算出「角色包围盒」（给气泡锚点/热区用）
 * 与「可点击格子掩码」（给 Rust 侧点击穿透用）。
 *
 * 掩码规则：任一不透明采样点落在某格子内，该格子即算角色区域（宁大勿小）——
 * 掩码偏大只会让角色边缘多挡一点点，偏小则会让用户点不到角色。
 */
function sampleFrame(video: HTMLVideoElement, winW: number, winH: number) {
  try {
    const canvas = document.createElement('canvas');
    canvas.width = SAMPLE_W;
    canvas.height = SAMPLE_H;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    if (!ctx) return null;
    ctx.drawImage(video, 0, 0, SAMPLE_W, SAMPLE_H);
    const { data } = ctx.getImageData(0, 0, SAMPLE_W, SAMPLE_H);

    let minX = SAMPLE_W;
    let minY = SAMPLE_H;
    let maxX = -1;
    let maxY = -1;

    const cells = new Uint8Array(MASK_COLS * MASK_ROWS);
    const rect = video.getBoundingClientRect();

    for (let y = 0; y < SAMPLE_H; y += 1) {
      // 采样点在窗口坐标系中的位置（CSS px）：视频按宽铺满、贴底对齐
      const wy = rect.top + ((y + 0.5) / SAMPLE_H) * rect.height;
      const cy = Math.floor((wy / winH) * MASK_ROWS);
      for (let x = 0; x < SAMPLE_W; x += 1) {
        if (data[(y * SAMPLE_W + x) * 4 + 3] <= ALPHA_THRESHOLD) continue;
        if (x < minX) minX = x;
        if (x > maxX) maxX = x;
        if (y < minY) minY = y;
        if (y > maxY) maxY = y;
        if (cy < 0 || cy >= MASK_ROWS) continue;
        const wx = rect.left + ((x + 0.5) / SAMPLE_W) * rect.width;
        const cx = Math.floor((wx / winW) * MASK_COLS);
        if (cx < 0 || cx >= MASK_COLS) continue;
        cells[cy * MASK_COLS + cx] = 1;
      }
    }

    if (maxX < 0) return null;
    return {
      box: {
        left: minX / SAMPLE_W,
        right: (maxX + 1) / SAMPLE_W,
        top: minY / SAMPLE_H,
        bottom: (maxY + 1) / SAMPLE_H,
      },
      cells,
    };
  } catch {
    // 读回像素被拒（理论上同源不会发生）按失败处理，父组件用默认热区
    return null;
  }
}

export default function PetSprite({
  actionId,
  playKey,
  paused,
  onEnded,
  onMeasure,
  onMask,
}: Props) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const measuredRef = useRef(false);
  const url = actionUrl(actionId);
  const loop = isLoopAction(actionId);

  /** 最新 paused 值：播放重试与事件回调里避免读到闭包旧值 */
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const playTimerRef = useRef<number | null>(null);

  /**
   * 播放（失败退避重试）。
   *
   * 为什么不能只 play() 一次：窗口按 `visible:false` 建好后才 show，首启时页面
   * 往往仍被判定为 hidden / 合成器未就绪，`play()` 会被拒绝；只试一次且无人重试，
   * 画面就一直是空的——这正是「桌宠首启整块透明」的根因。
   */
  const playWithRetry = useCallback((retry = 0) => {
    if (playTimerRef.current) {
      window.clearTimeout(playTimerRef.current);
      playTimerRef.current = null;
    }
    const video = videoRef.current;
    if (!video || pausedRef.current) return;
    void video.play().catch(() => {
      if (retry >= 6 || pausedRef.current) return;
      playTimerRef.current = window.setTimeout(
        () => playWithRetry(retry + 1),
        250 * (retry + 1),
      );
    });
  }, []);

  const measure = useCallback(
    (retry = 0) => {
      const video = videoRef.current;
      if (!video) return;

      const rect = video.getBoundingClientRect();
      const sample = sampleFrame(video, window.innerWidth || 1, window.innerHeight || 1);
      // 首帧可能还没出画面（窗口隐藏/解码未完成）或布局为 0：退避重试几次；
      // 采样为空通常意味着视频没在播，顺带重试一次播放
      if (!sample || rect.width < 1 || rect.height < 1) {
        playWithRetry();
        if (retry < 4) window.setTimeout(() => measure(retry + 1), 300);
        return;
      }

      // 掩码每次换动作都重采样（不同动作角色姿态/位置不同，穿透区域要跟着变）
      onMask?.(sample.cells);

      // 包围盒只在首帧量一次：气泡锚点/点击热区用，姿态变化不需要跟着抖
      if (measuredRef.current || !onMeasure) return;
      measuredRef.current = true;
      const winW = window.innerWidth || 1;
      const winH = window.innerHeight || 1;
      const box = sample.box;
      onMeasure({
        left: ((rect.left + box.left * rect.width) / winW) * 100,
        width: ((box.right - box.left) * rect.width / winW) * 100,
        top: ((rect.top + box.top * rect.height) / winH) * 100,
        height: ((box.bottom - box.top) * rect.height / winH) * 100,
      });
    },
    [onMeasure, onMask, playWithRetry],
  );

  useEffect(() => {
    const video = videoRef.current;
    if (!video || !url) return;

    video.loop = loop;
    if (video.dataset.action !== actionId) {
      video.dataset.action = actionId;
      video.src = url;
      video.load();
    } else if (playKey > 0) {
      try {
        video.currentTime = 0;
      } catch {
        /* seek 失败（尚未 loadedmetadata）忽略，下一次 play 会从头开始 */
      }
    }

    if (paused) {
      video.pause();
    } else {
      playWithRetry();
    }
  }, [actionId, url, loop, playKey, paused, playWithRetry]);

  // 隐藏窗口时立刻停解码；恢复时由上面的 effect 续播
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    if (paused) {
      video.pause();
      return;
    }
    // 隐藏期间 Chromium 会挂起媒体加载：恢复显示时可能连首帧都没有
    // （窗口可见、画面整块空的直接原因），此时重新触发一次加载再播。
    // 加 currentSrc 判断：首次挂载时 src 还没设上，免得和上面的效应抢着 load()
    if (video.readyState < 2 && video.currentSrc) video.load();
    playWithRetry();
    // 窗口隐藏时首帧可能取不到（抽帧失败）：显示后补测一次
    measure();
  }, [paused, measure, playWithRetry]);

  // 卸载时清掉待执行的播放重试
  useEffect(
    () => () => {
      if (playTimerRef.current) window.clearTimeout(playTimerRef.current);
    },
    [],
  );

  return (
    <video
      ref={videoRef}
      className="pet-video"
      muted
      playsInline
      preload="auto"
      // 首帧常是淡入/起手式（角色还没站定），等 400ms 取稳定姿态再量包围盒
      onLoadedData={() => window.setTimeout(() => measure(), 400)}
      // 缓冲够了再试一次播放：首启时 play() 常被拒，等到 canplay 才有机会成功
      onCanPlay={() => playWithRetry()}
      onEnded={onEnded}
      // 素材缺失/解码失败不该白屏卡死：保留上一帧，仅记录一次
      onError={() => {
        // eslint-disable-next-line no-console
        console.warn('[pet] 动作素材加载失败:', actionId);
      }}
    />
  );
}
