/**
 * 壁纸层：常驻在应用最底层的一张"场景"。
 *
 * 两种来源：
 * - `kind === 'css'`：内置场景（纯 CSS + 内联 SVG，零素材、零依赖）
 * - `kind === 'we'`：本机 Wallpaper Engine 工程，交给 webwallgl 实时渲染
 *
 * 设计约束（都来自本项目既有约定）：
 * 1. **不占布局**：`position: fixed; inset: 0; z-index: -2`，不走 flex 流，
 *    所以 App 的骨架（TitleBar / Sidebar / TopBar / main）一行都不用改。
 * 2. **不吃事件**：`pointer-events: none`，滚轮、拖拽、右键全部穿透到下层的真实 UI。
 * 3. **只在需要时存在**：壁纸关闭时整层不挂载，DOM 零成本。
 * 4. **失败必回落**：WE 场景渲染不出来时立刻退回内置场景，绝不留空白。
 * 5. **零图片资源**（内置场景）：场景与鲸鱼全部用 CSS 渐变 + 内联 SVG 画。
 */

import { useEffect, useMemo, useState } from 'react';
import { setAppearance, useAppearance } from '../lib/appearance';
import type { SceneId } from '../lib/appearance';
import { useWeLibrary } from '../lib/useWeLibrary';
import { wePreviewUrl, weVideoUrl } from '../lib/we';
import SceneWallpaper from './SceneWallpaper';

/** 气泡参数表：写死而不用 Math.random —— 渲染必须确定性，否则每次重挂载都跳一次 */
const BUBBLES: ReadonlyArray<{ x: number; size: number; delay: number; dur: number }> = [
  { x: 6, size: 5, delay: 0, dur: 17 },
  { x: 13, size: 3, delay: 4.5, dur: 21 },
  { x: 21, size: 7, delay: 9, dur: 15 },
  { x: 28, size: 4, delay: 2, dur: 19 },
  { x: 36, size: 3, delay: 12, dur: 23 },
  { x: 44, size: 6, delay: 6.5, dur: 16 },
  { x: 52, size: 4, delay: 14, dur: 20 },
  { x: 61, size: 3, delay: 1.5, dur: 24 },
  { x: 69, size: 8, delay: 8, dur: 18 },
  { x: 77, size: 4, delay: 16, dur: 22 },
  { x: 85, size: 3, delay: 3, dur: 17 },
  { x: 92, size: 6, delay: 11, dur: 19 },
];

/** 大肥鱼：蓝鲸造型的吉祥物剪影（纯 SVG，无水印无字样） */
function Whale() {
  return (
    <svg className="wp-whale" viewBox="0 0 260 160" aria-hidden focusable="false">
      <defs>
        <linearGradient id="wp-whale-body" x1="0" y1="0" x2="0.35" y2="1">
          <stop offset="0%" stopColor="#5ea8ff" />
          <stop offset="55%" stopColor="#2f78e0" />
          <stop offset="100%" stopColor="#1a4fa8" />
        </linearGradient>
        <linearGradient id="wp-whale-belly" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor="#cfe6ff" stopOpacity="0.85" />
          <stop offset="100%" stopColor="#8fbdf0" stopOpacity="0.25" />
        </linearGradient>
      </defs>

      {/* 尾鳍：单独一组，绕根部小幅摆动 */}
      <g className="wp-whale-tail">
        <path
          d="M62 82 C46 82 30 74 18 60 C13 54 6 56 7 64 C9 82 22 98 40 106 C28 112 20 124 18 136 C17 144 25 145 30 139 C44 124 58 112 74 106 Z"
          fill="url(#wp-whale-body)"
        />
      </g>

      {/* 身体 */}
      <path
        d="M60 78 C68 46 100 26 142 26 C186 26 214 48 220 76 C226 104 200 128 158 132 C114 136 74 122 62 100 C60 94 59 86 60 78 Z"
        fill="url(#wp-whale-body)"
      />
      {/* 腹部高光 */}
      <path
        d="M78 100 C96 122 132 132 162 128 C192 124 214 108 219 88 C214 112 188 130 154 133 C116 136 82 124 70 104 Z"
        fill="url(#wp-whale-belly)"
      />
      {/* 胸鳍 */}
      <path
        d="M104 106 C112 122 128 132 144 132 C134 122 126 112 122 100 Z"
        fill="#1d55ad"
        opacity="0.75"
      />
      {/* 眼睛 */}
      <circle cx="186" cy="70" r="7.5" fill="#0b1a33" />
      <circle cx="188.5" cy="67.5" r="2.4" fill="#eaf4ff" />
      {/* 微笑 */}
      <path
        d="M196 88 C202 93 210 93 215 89"
        stroke="#0b1a33"
        strokeWidth="3"
        strokeLinecap="round"
        fill="none"
        opacity="0.55"
      />
      {/* 喷气小水花 */}
      <g className="wp-whale-spout" opacity="0.5">
        <circle cx="150" cy="16" r="3" fill="#bfe0ff" />
        <circle cx="142" cy="8" r="2" fill="#bfe0ff" />
        <circle cx="158" cy="9" r="2.4" fill="#bfe0ff" />
      </g>
    </svg>
  );
}

function CssSceneArt({ scene }: { scene: SceneId }) {
  if (scene === 'aurora') {
    return (
      <div className="wp-aurora-wrap" aria-hidden>
        <span className="wp-aurora wp-aurora--a" />
        <span className="wp-aurora wp-aurora--b" />
        <span className="wp-aurora wp-aurora--c" />
      </div>
    );
  }
  if (scene === 'abyss') {
    return (
      <div className="wp-dust-wrap" aria-hidden>
        {BUBBLES.slice(0, 7).map((b, i) => (
          <span
            key={i}
            className="wp-dust"
            style={{
              left: `${b.x}%`,
              width: `${b.size}px`,
              height: `${b.size}px`,
              animationDelay: `${b.delay}s`,
              animationDuration: `${b.dur + 10}s`,
            }}
          />
        ))}
      </div>
    );
  }
  return (
    <>
      <div className="wp-rays" aria-hidden>
        <span className="wp-ray wp-ray--a" />
        <span className="wp-ray wp-ray--b" />
        <span className="wp-ray wp-ray--c" />
      </div>
      <div className="wp-bubbles" aria-hidden>
        {BUBBLES.map((b, i) => (
          <span
            key={i}
            className="wp-bubble"
            style={{
              left: `${b.x}%`,
              width: `${b.size}px`,
              height: `${b.size}px`,
              animationDelay: `${b.delay}s`,
              animationDuration: `${b.dur}s`,
            }}
          />
        ))}
      </div>
      <Whale />
    </>
  );
}

export default function WallpaperLayer() {
  const { wallpaper, kind, scene, weId, weKind } = useAppearance();
  const lib = useWeLibrary(wallpaper && kind === 'we');
  const [failed, setFailed] = useState<string | null>(null);
  const [videoUrl, setVideoUrl] = useState<string | null>(null);
  /** 视频首帧是否已经可以显示（4K 解码要几百毫秒，直接上屏会先黑一下） */
  const [videoReady, setVideoReady] = useState(false);
  /** 预览图：视频就绪前先垫着，避免开屏那段变成纯黑 */
  const [poster, setPoster] = useState<string | null>(null);

  const target = useMemo(
    () => lib.data?.items.find((w) => w.id === weId) ?? null,
    [lib.data, weId],
  );

  // 换壁纸时清掉上一张的失败记忆，否则一次失败会让这张永远回退
  useEffect(() => {
    setFailed(null);
  }, [weId]);

  // 自愈：旧版本选中的条目没有 weKind（那时还没这个字段），库一扫出结果就补写回去。
  // 不补的话每次启动都要等扫描才能判断该走视频还是场景，开屏会先闪一下别的动画。
  useEffect(() => {
    if (kind !== 'we' || !target) return;
    if (target.kind !== weKind) setAppearance({ weKind: target.kind });
  }, [kind, target, weKind]);

  // 视频类壁纸：读整段 mp4 生成 blob URL，同时取预览图垫底
  useEffect(() => {
    let alive = true;
    setVideoUrl(null);
    setVideoReady(false);
    setPoster(null);
    if (kind === 'we' && target?.kind === 'video' && !failed) {
      void weVideoUrl(target).then((url) => {
        if (alive) setVideoUrl(url);
      });
      void wePreviewUrl(target).then((url) => {
        if (alive) setPoster(url);
      });
    }
    return () => {
      alive = false;
    };
  }, [kind, target, failed]);

  if (!wallpaper) return null;

  // 壁纸库还没扫完时用持久化下来的类型先定分支（否则会先闪一整张内置场景再切走）
  const declaredKind = target?.kind ?? weKind ?? null;
  const wantWe = kind === 'we' && !failed;
  const weScene = wantWe && declaredKind === 'scene' && !!target;
  const weVideo = wantWe && declaredKind === 'video' && !!videoUrl;
  // WE 模式下不拿内置场景当垫底：场景/视频之外的（web、未知）才回落
  const cssFallback = !wantWe || (declaredKind !== 'scene' && declaredKind !== 'video');

  return (
    <div
      className="wp-layer"
      data-kind={weScene ? 'we' : cssFallback ? 'css' : 'we-video'}
      data-scene={scene}
      aria-hidden
    >
      {/* 底色：所有来源共用，保证首帧前不是白板 */}
      <div className="wp-base" />
      {weScene && (
        <div className="wp-we">
          <SceneWallpaper
            wallpaper={target}
            onFail={(reason) => {
              // 只在调试信息里留一句，不弹窗打扰
              console.warn('[wallpaper] WE 场景渲染失败，回退内置场景：', reason);
              setFailed(reason);
            }}
          />
        </div>
      )}
      {weVideo && (
        <>
          {poster && !videoReady && (
            <div className="wp-poster" style={{ backgroundImage: `url("${poster}")` }} />
          )}
          <video
            className={`wp-video${videoReady ? ' wp-video--ready' : ''}`}
            src={videoUrl ?? undefined}
            autoPlay
            muted
            loop
            playsInline
            onCanPlay={() => setVideoReady(true)}
          />
        </>
      )}
      {cssFallback && (
        <>
          <div className="wp-layer-scene" data-scene={scene}>
            <CssSceneArt scene={scene} />
          </div>
          <div className="wp-vignette" />
        </>
      )}
      {!cssFallback && <div className="wp-vignette" />}
    </div>
  );
}
