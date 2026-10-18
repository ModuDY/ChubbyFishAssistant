/**
 * 精灵渲染：一个 `<video>` 播 WebM(VP9 + alpha)。
 *
 * 为什么不用帧序列 + Canvas（上游做法）：本机实测 WebView2 完整保留了视频 alpha
 * （透明像素约 84.5%）、切动作出画面 <120ms，`<video>` 已经够用；帧序列要多维护
 * 预解码/缓存/内存上限三套逻辑，收益不足以抵消复杂度（见方案 §8.3）。
 *
 * **隐藏即停**：窗口不可见时 WebView 的 rAF/定时器可能被冻结，但解码器未必停。
 * 这里由父组件传 `paused`，窗口隐藏时显式 `pause()`，避免后台满速空转吃 CPU。
 */
import { useEffect, useRef } from 'react';
import { actionUrl } from './actions';
import { isLoopAction } from './petState';

type Props = {
  actionId: string;
  /** 同动作重播时递增（点击回应连续触发时也能从头播） */
  playKey: number;
  paused: boolean;
  onEnded: () => void;
};

export default function PetSprite({ actionId, playKey, paused, onEnded }: Props) {
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const url = actionUrl(actionId);
  const loop = isLoopAction(actionId);

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
  }, [paused]);

  return (
    <video
      ref={videoRef}
      className="pet-video"
      muted
      playsInline
      preload="auto"
      onEnded={onEnded}
      // 素材缺失/解码失败不该白屏卡死：保留上一帧，仅记录一次
      onError={() => {
        // eslint-disable-next-line no-console
        console.warn('[pet] 动作素材加载失败:', actionId);
      }}
    />
  );
}
