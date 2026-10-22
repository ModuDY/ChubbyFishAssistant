/**
 * 「壁纸与动效」设置区：壁纸来源 / 玻璃分面 / 光效 / 视差。
 *
 * 与 GeneralSettingsPanel 的其它区块不同，这里的改动**即时生效**、不走「保存设置」：
 * 它们是设备本地的纯视觉偏好（localStorage），没有"草稿"语义，实时预览才有意义。
 */

import { useEffect, useState } from 'react';
import { Image as ImageIcon, Layers, MonitorPlay, Sparkles, Waves } from 'lucide-react';
import {
  GLASS_LEVELS,
  GLASS_SURFACES,
  SCENES,
  resolveSurface,
  setAppearance,
  useAppearance,
  type GlassLevel,
  type GlassSurface,
  type SceneId,
} from '../lib/appearance';
import { loadWeLibrary, useWeLibrary } from '../lib/useWeLibrary';
import { wePreviewUrl, type WeWallpaper } from '../lib/we';

/** 缩略图：异步取 blob URL，取不到就显示占位 */
function WeThumb({ item, active }: { item: WeWallpaper; active: boolean }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    void wePreviewUrl(item).then((u) => {
      if (alive) setUrl(u);
    });
    return () => {
      alive = false;
    };
  }, [item]);

  return (
    <button
      type="button"
      onClick={() => setAppearance({ kind: 'we', weId: item.id, wallpaper: true })}
      title={`${item.title}${item.workshop ? '（创意工坊）' : '（本机工程）'}`}
      className={`group relative overflow-hidden rounded-lg border text-left transition ${
        active
          ? 'border-zinc-900 ring-2 ring-zinc-900/20 dark:border-zinc-100 dark:ring-zinc-100/25'
          : 'border-slate-200 hover:border-slate-400 dark:border-zinc-700 dark:hover:border-zinc-500'
      }`}
    >
      <div className="aspect-video w-full bg-slate-100 dark:bg-zinc-800">
        {url ? (
          <img src={url} alt="" className="h-full w-full object-cover" loading="lazy" />
        ) : (
          <div className="flex h-full w-full items-center justify-center text-slate-400">
            <ImageIcon size={18} />
          </div>
        )}
      </div>
      <div className="absolute inset-x-0 bottom-0 truncate bg-gradient-to-t from-black/70 to-transparent px-2 py-1 text-[11px] text-white">
        {item.title}
      </div>
      {item.kind !== 'scene' && (
        <span className="absolute right-1 top-1 rounded bg-black/60 px-1 py-0.5 text-[10px] text-white/90">
          {item.kind}
        </span>
      )}
    </button>
  );
}

function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  suffix,
  onChange,
  disabled,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  suffix?: string;
  onChange: (v: number) => void;
  disabled?: boolean;
}) {
  return (
    <label className={`block ${disabled ? 'opacity-45' : ''}`}>
      <span className="flex items-baseline justify-between text-xs text-slate-500 dark:text-zinc-400">
        <span>{label}</span>
        <span className="tabular-nums">
          {value}
          {suffix}
        </span>
      </span>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(Number(e.target.value))}
        className="mt-1 w-full"
      />
    </label>
  );
}

export default function AppearanceSettings() {
  const a = useAppearance();
  const lib = useWeLibrary(a.wallpaper && a.kind === 'we');

  return (
    <>
      {/* 壁纸 */}
      <section className="card p-4">
        <h3 className="mb-1 font-medium">壁纸</h3>
        <p className="mb-3 text-xs text-slate-400">
          场景壁纸常驻在界面最底层，玻璃面板透过它取色。<b>改动即时生效</b>，保存在本机
          （只影响外观，不参与账号备份与同步）。
        </p>

        <label className="mb-3 flex items-center gap-2">
          <input
            type="checkbox"
            checked={a.wallpaper}
            onChange={(e) => setAppearance({ wallpaper: e.target.checked })}
          />
          <span>启用场景壁纸</span>
          <span className="text-xs text-slate-400">（关掉后界面与改造前完全一致）</span>
        </label>

        <div className="mb-3 flex gap-1 rounded-lg bg-slate-100 p-1 text-xs dark:bg-zinc-800">
          <button
            type="button"
            onClick={() => setAppearance({ kind: 'css' })}
            className={`flex-1 rounded-md px-2 py-1.5 transition ${
              a.kind === 'css'
                ? 'bg-white font-medium shadow-sm dark:bg-zinc-700'
                : 'text-slate-500 dark:text-zinc-400'
            }`}
          >
            <Layers size={13} className="mr-1 inline" />
            内置场景
          </button>
          <button
            type="button"
            onClick={() => {
              setAppearance({ kind: 'we' });
              void loadWeLibrary();
            }}
            className={`flex-1 rounded-md px-2 py-1.5 transition ${
              a.kind === 'we'
                ? 'bg-white font-medium shadow-sm dark:bg-zinc-700'
                : 'text-slate-500 dark:text-zinc-400'
            }`}
          >
            <MonitorPlay size={13} className="mr-1 inline" />
            Wallpaper Engine
          </button>
        </div>

        {a.kind === 'css' ? (
          <div>
            <label className="label">内置场景</label>
            <select
              value={a.scene}
              disabled={!a.wallpaper}
              onChange={(e) => setAppearance({ scene: e.target.value as SceneId })}
              className="input disabled:opacity-50"
            >
              {SCENES.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name}
                </option>
              ))}
            </select>
            <p className="mt-1 text-xs text-slate-400">
              {SCENES.find((s) => s.id === a.scene)?.hint}
            </p>
          </div>
        ) : (
          <div>
            <div className="mb-2 flex items-center justify-between">
              <span className="label mb-0">本机壁纸</span>
              <button
                type="button"
                className="text-xs text-slate-500 underline decoration-dotted hover:text-slate-800 dark:text-zinc-400 dark:hover:text-zinc-100"
                onClick={() => void loadWeLibrary(true)}
              >
                重新扫描
              </button>
            </div>

            {lib.status === 'loading' && (
              <p className="text-xs text-slate-400">正在扫描本机 Wallpaper Engine…</p>
            )}
            {lib.status === 'error' && (
              <p className="text-xs text-rose-500">扫描失败：{lib.error}</p>
            )}
            {lib.status === 'ready' && lib.data && lib.data.items.length === 0 && (
              <p className="text-xs text-slate-400">
                没找到壁纸。需要本机安装 Wallpaper Engine（Steam 应用 431960）；
                装好后点「重新扫描」。
              </p>
            )}
            {lib.status === 'ready' && lib.data && lib.data.items.length > 0 && (
              <>
                <p className="mb-2 text-xs text-slate-400">
                  找到 {lib.data.items.length} 张
                  {lib.data.installDir ? '，来自本机安装目录' : ''}。
                  场景类由 WebGL 实时渲染，视频类按原片播放。
                </p>
                <div className="grid max-h-72 grid-cols-3 gap-2 overflow-auto pr-1 sm:grid-cols-4">
                  {lib.data.items.map((item) => (
                    <WeThumb
                      key={item.id}
                      item={item}
                      active={a.weId === item.id}
                    />
                  ))}
                </div>
              </>
            )}
          </div>
        )}
      </section>

      {/* 玻璃分面 */}
      <section className="card p-4">
        <h3 className="mb-1 font-medium">液态玻璃</h3>
        <p className="mb-3 text-xs text-slate-400">
          四个面可以各调各的。勾「跟随全局」时用下方档位，取消后可单独拖动。
        </p>

        <label className="mb-3 block">
          <span className="label">全局档位</span>
          <select
            value={a.glassLevel}
            disabled={!a.wallpaper}
            onChange={(e) => setAppearance({ glassLevel: e.target.value as GlassLevel })}
            className="input disabled:opacity-50"
          >
            {GLASS_LEVELS.map((g) => (
              <option key={g.id} value={g.id}>
                {g.name}
              </option>
            ))}
          </select>
        </label>

        <div className="space-y-3">
          {GLASS_SURFACES.map((s) => {
            const cfg = a.glass[s.id as GlassSurface];
            const resolved = resolveSurface(a, s.id);
            return (
              <div
                key={s.id}
                className="rounded-lg border border-slate-200 p-3 dark:border-zinc-700/70"
              >
                <div className="flex items-center justify-between">
                  <div>
                    <div className="text-sm">{s.name}</div>
                    <div className="text-[11px] text-slate-400">{s.hint}</div>
                  </div>
                  <label className="flex shrink-0 items-center gap-1 text-xs text-slate-500 dark:text-zinc-400">
                    <input
                      type="checkbox"
                      checked={cfg.inherit}
                      disabled={!a.wallpaper}
                      onChange={(e) =>
                        setAppearance({
                          glass: {
                            ...a.glass,
                            [s.id]: { ...cfg, inherit: e.target.checked },
                          },
                        })
                      }
                    />
                    跟随全局
                  </label>
                </div>
                <div className="mt-2 grid gap-2 sm:grid-cols-2">
                  <Slider
                    label="模糊"
                    value={cfg.inherit ? resolved.blur : cfg.blur}
                    min={0}
                    max={40}
                    suffix="px"
                    disabled={!a.wallpaper || cfg.inherit || s.id === 'card'}
                    onChange={(v) =>
                      setAppearance({
                        glass: { ...a.glass, [s.id]: { ...cfg, blur: v } },
                      })
                    }
                  />
                  <Slider
                    label="不透明度"
                    value={Math.round((cfg.inherit ? resolved.alpha : cfg.alpha) * 100)}
                    min={20}
                    max={100}
                    suffix="%"
                    disabled={!a.wallpaper || cfg.inherit}
                    onChange={(v) =>
                      setAppearance({
                        glass: { ...a.glass, [s.id]: { ...cfg, alpha: v / 100 } },
                      })
                    }
                  />
                </div>
                {s.id === 'card' && (
                  <p className="mt-1 text-[11px] text-slate-400">
                    卡片刻意不加背景模糊：壁纸在动，一页十来张卡片逐帧重算模糊太贵。
                  </p>
                )}
              </div>
            );
          })}
        </div>
      </section>

      {/* 光效 */}
      <section className="card p-4">
        <h3 className="mb-1 flex items-center gap-1.5 font-medium">
          <Sparkles size={15} /> 点击与拖尾光效
        </h3>
        <p className="mb-3 text-xs text-slate-400">
          独立 canvas 绘制，不碰页面布局。没有粒子时会完全停帧（空闲零开销）。
        </p>

        <label className="mb-2 flex items-center gap-2">
          <input
            type="checkbox"
            checked={a.fx.enabled}
            onChange={(e) => setAppearance({ fx: { enabled: e.target.checked } })}
          />
          <span>启用光效</span>
        </label>

        <div className={`grid gap-x-6 gap-y-1 sm:grid-cols-2 ${a.fx.enabled ? '' : 'opacity-45'}`}>
          {(
            [
              ['click', '点击爆闪环'],
              ['trail', '彗尾拖尾'],
              ['sparks', '点击星火'],
              ['dust', '指针浮尘'],
            ] as const
          ).map(([key, label]) => (
            <label key={key} className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={a.fx[key]}
                disabled={!a.fx.enabled}
                onChange={(e) => setAppearance({ fx: { [key]: e.target.checked } })}
              />
              {label}
            </label>
          ))}
        </div>

        <div className="mt-2">
          <Slider
            label="强度"
            value={Number(a.fx.intensity.toFixed(1))}
            min={0.4}
            max={2}
            step={0.1}
            suffix="×"
            disabled={!a.fx.enabled}
            onChange={(v) => setAppearance({ fx: { intensity: v } })}
          />
        </div>
      </section>

      {/* 视差 */}
      <section className="card p-4">
        <h3 className="mb-1 flex items-center gap-1.5 font-medium">
          <Waves size={15} /> 3D 视差
        </h3>
        <p className="mb-3 text-xs text-slate-400">
          光标移动时壁纸与界面分层跟随，制造纵深。界面组会量化到整设备像素（否则文字发虚）。
        </p>

        <label className="mb-2 flex items-center gap-2">
          <input
            type="checkbox"
            checked={a.parallax.enabled}
            onChange={(e) => setAppearance({ parallax: { enabled: e.target.checked } })}
          />
          <span>启用视差</span>
        </label>

        <div className={`grid gap-3 sm:grid-cols-3 ${a.parallax.enabled ? '' : 'opacity-45'}`}>
          <Slider
            label="壁纸幅度"
            value={Number(a.parallax.wallpaper.toFixed(1))}
            min={0}
            max={3}
            step={0.1}
            suffix="%"
            disabled={!a.parallax.enabled}
            onChange={(v) => setAppearance({ parallax: { wallpaper: v } })}
          />
          <Slider
            label="界面幅度"
            value={Number(a.parallax.ui.toFixed(1))}
            min={0}
            max={3}
            step={0.1}
            suffix="%"
            disabled={!a.parallax.enabled}
            onChange={(v) => setAppearance({ parallax: { ui: v } })}
          />
          <Slider
            label="缓动"
            value={Number(a.parallax.smoothing.toFixed(2))}
            min={0.05}
            max={0.4}
            step={0.01}
            disabled={!a.parallax.enabled}
            onChange={(v) => setAppearance({ parallax: { smoothing: v } })}
          />
        </div>
      </section>

      {/* 开屏 */}
      <section className="card p-4">
        <h3 className="mb-1 font-medium">进应用动画</h3>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={a.entryAnim}
            onChange={(e) => setAppearance({ entryAnim: e.target.checked })}
          />
          <span>启用开屏</span>
          <span className="text-xs text-slate-400">（约 1 秒，点任意处可跳过）</span>
        </label>
      </section>
    </>
  );
}
