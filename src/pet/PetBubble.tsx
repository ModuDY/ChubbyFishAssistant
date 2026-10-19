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

/** 气泡正文上限：模型偶尔会啰嗦（甚至把思考写进正文），超长直接截断，别把气泡撑爆 */
const MAX_TEXT = 80;
/** 思考小字上限：只给个"它在想什么"的感觉，不占满整个气泡 */
const MAX_THINK = 60;

function clip(s: string, max: number): string {
  const one = s.replace(/\s+/g, ' ').trim();
  return one.length > max ? `${one.slice(0, max)}…` : one;
}

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

  const body = text ? clip(text, MAX_TEXT) : '';
  const think = thinking ? clip(thinking, MAX_THINK) : '';

  return (
    <div key={seq} className="pet-bubble" role="status">
      {think ? (
        <div className="pet-bubble-think" aria-hidden="true">
          {think}
        </div>
      ) : null}
      {/* 流式中还没出字：先给个"在酝酿"的占位，别让气泡空着 */}
      {body || (streaming ? '嗯…' : '…')}
    </div>
  );
}
