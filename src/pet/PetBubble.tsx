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
import { useEffect, useRef, useState } from 'react';

type Props = {
  /** 每条气泡带序号：同文案连续出现时也能重新触发淡入动画 */
  seq: number;
  text: string;
  onDone: () => void;
  /** 正在流式接收（AI 发言）：true 时不消失，false/undefined 走原计时语义 */
  streaming?: boolean;
  /** 思考链小字（可空） */
  thinking?: string;
  /**
   * 长文本模式（P7 对话答复）。
   *
   * 为什么需要：独白档只有"一句话"（上限 80 字够用），而对话档的 token 预算是 1200，
   * 答复常超过 80 字——沿用独白口径会把回答截在半句，用户以为模型没答完。
   * 这里放宽到 200 字并把容器限高抬高；再长仍截断（气泡是"贴在头顶"的，
   * 无限长会盖住角色，也超出窗口上方可用空间）。
   */
  long?: boolean;
  /**
   * 正在做的事（P9 联网检索）。
   *
   * 为什么需要：自主多轮检索时，模型先决定"去查"再回答，中间这几秒一个字都不出。
   * 沿用「在想…」会让用户以为它卡住或者发呆；显示「在查『北京天气』」才说明
   * 它正在干活。正文一到就自动让位（见下面的 `body ||` 分支）。
   */
  hint?: string;
};

/** 气泡正文上限：模型偶尔会啰嗦（甚至把思考写进正文），超长直接截断，别把气泡撑爆 */
const MAX_TEXT = 80;
/** 对话答复的正文上限（见 `long` 说明） */
const MAX_TEXT_LONG = 200;
/** 思考小字上限：只给个"它在想什么"的感觉，不占满整个气泡 */
const MAX_THINK = 60;

function clip(s: string, max: number): string {
  const one = s.replace(/\s+/g, ' ').trim();
  return one.length > max ? `${one.slice(0, max)}…` : one;
}

/**
 * 思考小字只在「确实是中文思考」时显示：
 * 部分模型用英文思考（"The user wants me to be a clingy…"），摆给中文用户看只是噪音。
 */
function worthShowingThink(s: string): boolean {
  const cjk = (s.match(/[\u4e00-\u9fa5]/g) ?? []).length;
  return cjk >= 4;
}

export default function PetBubble({ seq, text, onDone, streaming, thinking, long, hint }: Props) {
  // 用 ref 读最终文本长度：避免 text 逐字变化时反复重启计时器
  const lenRef = useRef(0);
  lenRef.current = text.length;

  // 等待反馈（P7 提速体验）：正文一个字都没到时显示「在想…」+ 已等秒数。
  // 为什么加计时：上游首字可能要几秒（重型模型/排队），静态占位看起来像卡死；
  // 秒数在跳 = 它活着在等上游，发送后的空窗从"黑箱"变成"可见的等待"。
  // 每 500ms 跳一次（整数秒，500ms 间隔保证 1s 边界最多晚半秒出现）。
  const [waited, setWaited] = useState(0);
  const waiting = streaming && !text;
  useEffect(() => {
    if (!waiting) {
      setWaited(0);
      return;
    }
    const t0 = Date.now();
    const timer = window.setInterval(() => {
      setWaited(Math.floor((Date.now() - t0) / 1000));
    }, 500);
    return () => window.clearInterval(timer);
  }, [waiting]);

  useEffect(() => {
    if (streaming) return; // 流式中：不挂计时器，等流结束
    const extra = Math.min(5200, lenRef.current * 60);
    const timer = window.setTimeout(onDone, 3800 + extra);
    return () => window.clearTimeout(timer);
  }, [seq, streaming, onDone]);

  const body = text ? clip(text, long ? MAX_TEXT_LONG : MAX_TEXT) : '';
  // 正文一旦开吐就收起思考：模型常在 thinking 里复述人设与要求，和真话并排显示很出戏
  const think =
    !body && thinking && worthShowingThink(thinking) ? clip(thinking, MAX_THINK) : '';

  return (
    <div key={seq} className={`pet-bubble${long ? ' pet-bubble-long' : ''}`} role="status">
      {think ? (
        <div className="pet-bubble-think" aria-hidden="true">
          {think}
        </div>
      ) : null}
      {/* 流式中还没出字：给"在想/在查…"+秒数的动态占位，别让气泡空着也不像卡死 */}
      {body ||
        (streaming
          ? hint
            ? waited > 0
              ? `${hint} ${waited}s`
              : hint
            : waited > 0
              ? `在想… ${waited}s`
              : '在想…'
          : '…')}
    </div>
  );
}
