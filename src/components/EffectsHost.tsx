/**
 * 特效宿主：把「点击/拖尾光效」与「3D 视差」两个纯 DOM 副作用挂到应用上。
 *
 * 为什么单独一个组件而不是塞进 App：
 * 两者都是命令式副作用（canvas + rAF + 事件监听），跟 React 渲染无关；
 * 放一处便于统一按设置热更、也便于在卸载时干净回收。
 *
 * 它们不渲染任何可见节点（光效 canvas 由模块自己 append），所以组件的返回值无关紧要。
 */

import { useEffect, useRef } from 'react';
import { useAppearance } from '../lib/appearance';
import { startFx, type FxHandle } from '../lib/fx';
import { startParallax, type ParallaxHandle } from '../lib/parallax';

export default function EffectsHost() {
  const { fx, parallax } = useAppearance();
  const fxRef = useRef<FxHandle | null>(null);
  const pxRef = useRef<ParallaxHandle | null>(null);
  // 首次用最新配置启动；后续改动走 refresh（不重建 canvas / 不抖一下）
  const fxCfg = useRef(fx);
  fxCfg.current = fx;
  const pxCfg = useRef(parallax);
  pxCfg.current = parallax;

  useEffect(() => {
    const fxHandle = startFx(fxCfg.current);
    const pxHandle = startParallax(pxCfg.current);
    fxRef.current = fxHandle;
    pxRef.current = pxHandle;
    return () => {
      fxHandle.stop();
      pxHandle.stop();
      fxRef.current = null;
      pxRef.current = null;
    };
  }, []);

  useEffect(() => {
    fxRef.current?.refresh(fx);
  }, [fx]);

  useEffect(() => {
    pxRef.current?.refresh(parallax);
  }, [parallax]);

  return null;
}
