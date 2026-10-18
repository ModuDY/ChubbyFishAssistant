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
};

/** 采样分辨率：160×90 足够定位包围盒，又不会在读回像素上花时间 */
const SAMPLE_W = 160;
const SAMPLE_H = 90;
/** alpha 阈值：素材背景残留 alpha 集中在 1~50，取 64 把背景彻底排除 */
const ALPHA_THRESHOLD = 64;

function measureAlphaBox(video: HTMLVideoElement): {
  left: number;
  right: number;
  top: number;
  bottom: number;
} | null {
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
    for (let y = 0; y < SAMPLE_H; y += 1) {
      for (let x = 0; x < SAMPLE_W; x += 1) {
        if (data[(y * SAMPLE_W + x) * 4 + 3] <= ALPHA_THRESHOLD) continue;
        if (x < minX) minX = x;
        if (x > maxX) maxX = x;
        if (y < minY) minY = y;
        if (y > maxY) maxY = y;
      }
    }
    if (maxX < 0) return null;
    return {
      left: minX / SAMPLE_W,
      right: (maxX + 1) / SAMPLE_W,
      top: minY / SAMPLE_H,
      bottom: (maxY + 1) / SAMPLE_H,
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
}: Props) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const measuredRef = useRef(false);
  const url = actionUrl(actionId);
  const loop = isLoopAction(actionId);

  const measure = useCallback(
    (retry = 0) => {
      if (measuredRef.current || !onMeasure) return;
      const video = videoRef.current;
      if (!video) return;

      const rect = video.getBoundingClientRect();
      const box = measureAlphaBox(video);
      // 首帧可能还没出画面（窗口隐藏/解码未完成）或布局为 0：退避重试几次
      if (!box || rect.width < 1 || rect.height < 1) {
        if (retry < 4) window.setTimeout(() => measure(retry + 1), 300);
        return;
      }

      measuredRef.current = true;
      const winW = window.innerWidth || 1;
      const winH = window.innerHeight || 1;
      onMeasure({
        left: ((rect.left + box.left * rect.width) / winW) * 100,
        width: ((box.right - box.left) * rect.width / winW) * 100,
        top: ((rect.top + box.top * rect.height) / winH) * 100,
        height: ((box.bottom - box.top) * rect.height / winH) * 100,
      });
    },
    [onMeasure],
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
      // 自动播放被拒（策略/时机）时静默忽略：下一次 effect 还会重试
      void video.play().catch(() => {});
    }
  }, [actionId, url, loop, playKey, paused]);

  // 隐藏窗口时立刻停解码；恢复时由上面的 effect 续播
  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    if (paused) video.pause();
    // 窗口隐藏时首帧可能取不到（抽帧失败）：显示后补测一次
    else measure();
  }, [paused, measure]);

  return (
    <video
      ref={videoRef}
      className="pet-video"
      muted
      playsInline
      preload="auto"
      // 首帧常是淡入/起手式（角色还没站定），等 400ms 取稳定姿态再量包围盒
      onLoadedData={() => window.setTimeout(() => measure(), 400)}
      onEnded={onEnded}
      // 素材缺失/解码失败不该白屏卡死：保留上一帧，仅记录一次
      onError={() => {
        // eslint-disable-next-line no-console
        console.warn('[pet] 动作素材加载失败:', actionId);
      }}
    />
  );
}
