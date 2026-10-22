/**
 * 壁纸层：常驻在应用最底层的一张「场景」。
 *
 * 设计约束（都来自本项目既有约定）：
 * 1. **不占布局**：`position: fixed; inset: 0; z-index: -2`，不走 flex 流，
 *    所以 App 的骨架（TitleBar / Sidebar / TopBar / main）一行都不用改。
 * 2. **不吃事件**：`pointer-events: none`，滚轮、拖拽、右键全部穿透到下层的真实 UI。
 * 3. **只在需要时存在**：壁纸关闭时整层不挂载，DOM 零成本，观感与改造前完全一致。
 * 4. **省电优先**：所有动效只用 `transform` / `opacity`（合成器属性，不触发布局与重绘）；
 *    不做 `filter: blur()` 的全屏逐帧动画（那是最贵的一档）；`prefers-reduced-motion`
 *    下全部静止。
 * 5. **零图片资源**：场景与鲸鱼全部用 CSS 渐变 + 内联 SVG 画，不新增任何二进制文件，
 *    不牵动 `scripts/gen_asset_base64.mjs` 的素材基线。
 */

import { useAppearance } from '../lib/appearance';
import type { SceneId } from '../lib/appearance';

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
    <svg
      className="wp-whale"
      viewBox="0 0 260 160"
      aria-hidden
      focusable="false"
    >
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

function SceneArt({ scene }: { scene: SceneId }) {
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
  // deep-sea（默认）
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
  const { wallpaper, scene } = useAppearance();
  if (!wallpaper) return null;
  return (
    <div className="wp-layer" data-scene={scene} aria-hidden>
      {/* 底色：所有场景共用，随主题（data-theme）换色相 */}
      <div className="wp-base" />
      <SceneArt scene={scene} />
      {/* 顶/底压暗，保证前景文字在任何壁纸上都有对比度 */}
      <div className="wp-vignette" />
    </div>
  );
}
