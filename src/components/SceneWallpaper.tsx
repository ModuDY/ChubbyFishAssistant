/**
 * 真实 Wallpaper Engine 场景壁纸：把工程目录交给 webwallgl 实时渲染。
 *
 * 四条硬约束：
 * 1. **首帧之前不闪黑**：容器初始透明，引擎 `onReady` 后才淡入；失败则把原因交给
 *    上层回退到内置 CSS 场景（绝不留下一个空白层）。
 * 2. **一定销毁**：`destroy()` 会停 rAF、释放贴图与 WebGL 上下文；换壁纸不销毁
 *    等于每次切换泄一个渲染上下文（浏览器上限约 16 个，很快就黑屏）。
 * 3. **每次尝试用全新的宿主节点**：开发模式的 StrictMode 会把 effect 跑两遍，
 *    两个实例先后往同一个容器里挂 —— 前一个被销毁时会把后一个正在用的 GL 状态一起带走，
 *    症状是 `WebGL 上下文丢失：画面已冻结且不再更新`（实测踩过）。
 *    所以这里由组件自己建一个子节点，每次重试换新节点 + 新上下文。
 * 4. **失败要能自愈**：上下文丢失属于可恢复错误，退避重试两次；两次都失败才回退。
 */

import { useEffect, useRef, useState } from 'react';
import { mount, type SceneInstance } from 'webwallgl';
import { weSource, type WeWallpaper } from '../lib/we';

interface Props {
  wallpaper: WeWallpaper;
  /** 渲染彻底失败（重试也用尽）时的回落回调 */
  onFail: (reason: string) => void;
  onReady?: () => void;
}

/** 上下文丢失这类错误值得重试；其它（比如工程缺文件）重试也没用 */
function isRecoverable(reason: string): boolean {
  return /上下文|context\s*lost|frozen|冻结|未更新/i.test(reason);
}

const MAX_ATTEMPTS = 3;

export default function SceneWallpaper({ wallpaper, onFail, onReady }: Props) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const [ready, setReady] = useState(false);
  const [gen, setGen] = useState(0);
  const attemptRef = useRef(0);
  const failRef = useRef(onFail);
  failRef.current = onFail;
  const readyCbRef = useRef(onReady);
  readyCbRef.current = onReady;

  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;

    // 每次尝试都建新节点：旧节点的 GL 上下文随旧实例一起废弃，互不干扰
    const host = document.createElement('div');
    host.className = 'wp-we-host';
    wrap.appendChild(host);

    let instance: SceneInstance | null = null;
    let cancelled = false;
    let retryTimer = 0;

    const retryOrFail = (reason: string) => {
      if (cancelled) return;
      attemptRef.current += 1;
      if (isRecoverable(reason) && attemptRef.current < MAX_ATTEMPTS) {
        retryTimer = window.setTimeout(() => {
          if (!cancelled) setGen((g) => g + 1);
        }, 350 * attemptRef.current);
        return;
      }
      failRef.current(reason);
    };

    setReady(false);

    void (async () => {
      try {
        const mounted = await mount(host, {
          source: weSource(wallpaper.dir, wallpaper.entry),
          fit: 'cover',
          fps: 60,
          volume: 0,
          autoplay: true,
          // 30 秒：本机最大的场景包也在数秒内出首帧，超过就是真的挂不上
          mountTimeoutMs: 30000,
          // 工坊场景的 Script 来自第三方作者；与本应用同源，保留但失败不致命
          scripts: true,
          autoQuality: true,
          onError: (err) => retryOrFail(String(err?.message ?? err)),
        });
        if (cancelled) {
          // 已切走：立刻释放，避免留下一个还在跑 rAF 的实例
          try {
            void mounted.destroy();
          } catch {
            /* 忽略 */
          }
          return;
        }
        instance = mounted;
        attemptRef.current = 0;
        host.classList.add('wp-we-host--ready');
        setReady(true);
        readyCbRef.current?.();
      } catch (e) {
        if (!cancelled) retryOrFail(String((e as Error)?.message ?? e));
      }
    })();

    return () => {
      cancelled = true;
      window.clearTimeout(retryTimer);
      if (instance) {
        try {
          void instance.destroy();
        } catch {
          /* 忽略 */
        }
      }
      // 让子节点随实例一起消失（下一次尝试会新建一个）
      host.remove();
    };
  }, [wallpaper.dir, wallpaper.entry, gen]);

  return <div ref={wrapRef} className="wp-we-slot" data-ready={ready || undefined} aria-hidden />;
}
