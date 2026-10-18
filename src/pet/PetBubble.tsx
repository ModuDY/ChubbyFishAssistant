/**
 * 气泡台词：3.8s 自动消失，不接收鼠标事件（避免挡住拖拽/点击）。
 *
 * 与动作解耦：状态决定动作，台词从对应池随机取——同一状态每次说的话不同，
 * 才不会显得像贴图。
 */
import { useEffect } from 'react';

type Props = {
  /** 每条气泡带序号：同文案连续出现时也能重新触发淡入动画 */
  seq: number;
  text: string;
  onDone: () => void;
};

export default function PetBubble({ seq, text, onDone }: Props) {
  useEffect(() => {
    const timer = window.setTimeout(onDone, 3800);
    return () => window.clearTimeout(timer);
  }, [seq, onDone]);

  return (
    <div key={seq} className="pet-bubble" role="status">
      {text}
    </div>
  );
}
