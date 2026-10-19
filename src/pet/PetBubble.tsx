/**
 * 气泡台词：默认 3.8s 自动消失，不接收鼠标事件（避免挡住拖拽/点击）。
 *
 * 与动作解耦：状态决定动作，台词从对应池随机取——同一状态每次说的话不同，
 * 才不会显得像贴图。
 *
 * P3 流式：`streaming=true` 期间不挂消失计时器（文本随 delta 增长即打字机），
 * 流结束后按最终长度延长停留时间；`thinking` 非空时在正文上方显示一行弱化的
 * 「碎碎念」（对应上游 reasoning_content）。
 */
import { useEffect, useRef } from 'react';

type Props = {
  /** 每条气泡带序号：同文案连续出现时也能重新触发淡入动画 */
  seq: number;
  text: string;
  onDone: () => void;
  /** 正在流式接收（AI 发言）：true 时不消失，false/undefined 走原计时语义 */
  streaming?: boolean;
  /** 思考链小字（可空） */
  thinking?: string;
};

export default function PetBubble({ seq, text, onDone, streaming, thinking }: Props) {
  // 用 ref 读最终文本长度：避免 text 逐字变化时反复重启计时器
  const lenRef = useRef(0);
  lenRef.current = text.length;

  useEffect(() => {
    if (streaming) return; // 流式中：不挂计时器，等流结束
    const extra = Math.min(5200, lenRef.current * 60);
    const timer = window.setTimeout(onDone, 3800 + extra);
    return () => window.clearTimeout(timer);
  }, [seq, streaming, onDone]);

  return (
    <div key={seq} className="pet-bubble" role="status">
      {thinking ? (
        <div className="pet-bubble-think" aria-hidden="true">
          {thinking}
        </div>
      ) : null}
      {text || '…'}
    </div>
  );
}
