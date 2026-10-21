/**
 * 桌宠互动小游戏：记忆亮灯 Simon（P10）
 *
 * 与 `PetRpsGame` 同一套硬约束（都是实测踩出来的，不是假设）：
 * 1. **点击穿透**：窗口按 sprite alpha 掩码 40Hz 切换 `set_ignore_cursor_events`，
 *    面板不在 alpha 里 → 挂载即 `setChatFocus(true)` 暂停穿透，卸载复位；
 *    PetRoot 另外把面板矩形并入掩码（双保险）。
 * 2. **焦点**：窗口是 `focus:false` 建的，`focusForInput()` 才能让键盘 1-4 生效。
 * 3. **不自带动作播放**：组件只跑序列与判定，把 `SimonRoundPayload` 交给 PetRoot，
 *    由它统一走 `playTransient`——游戏 UI 与 106 个动作素材彻底解耦。
 *
 * 每轮边界（复现成功 / 按错结束）都会回调一次，PetRoot 据此播动作 + 发起 AI 解说。
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../lib/tauri';
import {
  SIMON_PADS,
  extendSequence,
  judgePress,
  nextPad,
  showTiming,
  type SimonPad,
  type SimonRoundPayload,
} from './simon';

type Props = {
  onClose: () => void;
  /** 一轮结束（复现成功 / 按错）：由 PetRoot 播动作 + 决定要不要请 AI 解说 */
  onRoundEnd: (payload: SimonRoundPayload) => void;
};

type Phase = 'idle' | 'show' | 'clear' | 'input' | 'over';

export default function PetSimonGame({ onClose, onRoundEnd }: Props) {
  const [phase, setPhase] = useState<Phase>('idle');
  const [seq, setSeq] = useState<SimonPad[]>([]);
  const [inputIdx, setInputIdx] = useState(0);
  /** 当前亮着的灯（展示序列 / 玩家按键回显） */
  const [lit, setLit] = useState<SimonPad | null>(null);
  /** 按错的灯：闪红 400ms 再熄灭，让玩家知道错在哪 */
  const [wrongPad, setWrongPad] = useState<SimonPad | null>(null);
  /** 本局最高纪录（组件存活期间跨局保留，关掉面板才清零） */
  const [best, setBest] = useState(0);

  const aliveRef = useRef(true);
  const timersRef = useRef<number[]>([]);

  const clearTimers = useCallback(() => {
    timersRef.current.forEach((id) => window.clearTimeout(id));
    timersRef.current = [];
  }, []);

  /** 统一 setTimeout：卸载后的回调一律不执行（避免 setState on unmounted） */
  const later = useCallback((fn: () => void, ms: number) => {
    timersRef.current.push(
      window.setTimeout(() => {
        if (aliveRef.current) fn();
      }, ms),
    );
  }, []);

  /** 挂载即暂停穿透 + 激活窗口；卸载复位（含异常路径） */
  useEffect(() => {
    aliveRef.current = true;
    void api.pet.setChatFocus(true).catch(() => {});
    void api.pet.focusForInput().catch(() => {});
    return () => {
      aliveRef.current = false;
      clearTimers();
      void api.pet.setChatFocus(false).catch(() => {});
    };
  }, [clearTimers]);

  const showSequence = useCallback(
    (s: SimonPad[]) => {
      clearTimers();
      setPhase('show');
      setLit(null);
      setWrongPad(null);
      setInputIdx(0);
      const { on, gap } = showTiming(s.length);
      let t = 260;
      s.forEach((pad) => {
        later(() => setLit(pad), t);
        t += on;
        later(() => setLit(null), t);
        t += gap;
      });
      // 最后一盏灭掉后留 120ms 再交给玩家，避免"刚灭就得按"的急迫感
      later(() => setPhase('input'), t + 120);
    },
    [clearTimers, later],
  );

  const start = useCallback(() => {
    const first = [nextPad([])];
    setSeq(first);
    showSequence(first);
  }, [showSequence]);

  const pressPad = (pad: SimonPad) => {
    if (phase !== 'input') return;
    if (judgePress(seq, inputIdx, pad) === 'wrong') {
      clearTimers();
      setPhase('over');
      setWrongPad(pad);
      setLit(null);
      onRoundEnd({ event: 'miss', round: seq.length, best, wrongAt: inputIdx + 1 });
      return;
    }
    // 按键回显：亮 180ms，让"我按到了"可见
    setLit(pad);
    later(() => setLit(null), 180);
    if (inputIdx + 1 < seq.length) {
      setInputIdx(inputIdx + 1);
      return;
    }
    // 完整复现：先结算记录，再延长序列进入下一轮
    const nextBest = Math.max(best, seq.length);
    const isRecord = seq.length > best;
    setBest(nextBest);
    setPhase('clear');
    onRoundEnd({ event: isRecord ? 'record' : 'clear', round: seq.length, best: nextBest });
    later(() => {
      const next = extendSequence(seq);
      setSeq(next);
      showSequence(next);
    }, 700);
  };
  // 用 ref 持有最新按键处理：键盘监听只注册一次，不随每轮状态重绑
  const pressRef = useRef(pressPad);
  pressRef.current = pressPad;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
        return;
      }
      if ((e.key === ' ' || e.key === 'Enter') && (phase === 'idle' || phase === 'over')) {
        e.preventDefault();
        start();
        return;
      }
      const idx = ['1', '2', '3', '4'].indexOf(e.key);
      if (idx >= 0) {
        e.preventDefault();
        pressRef.current(SIMON_PADS[idx]);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose, start, phase]);

  const stageText =
    phase === 'idle'
      ? '看灯的顺序，再按一遍'
      : phase === 'show'
        ? `看好了…（${seq.length} 个）`
        : phase === 'input'
          ? `该你了 ${inputIdx + 1}/${seq.length}`
          : phase === 'clear'
            ? '记对了！'
            : `按错了 · 这局 ${seq.length} 个`;

  return (
    <div className="pet-game pet-simon" onPointerDown={(e) => e.stopPropagation()}>
      <div className="pet-game-head">
        <span className="pet-game-score">最高 {best} 个</span>
        <button type="button" className="pet-game-close" onClick={onClose} title="结束（Esc）">
          结束
        </button>
      </div>

      <div className="pet-game-stage" aria-live="polite">
        <span
          className={`pet-game-outcome${
            phase === 'over' ? ' is-lose' : phase === 'clear' ? ' is-win' : ''
          }`}
        >
          {stageText}
        </span>
      </div>

      <div className="pet-simon-grid">
        {SIMON_PADS.map((p, i) => (
          <button
            key={p}
            type="button"
            className={`pet-simon-pad is-p${p}${lit === p ? ' is-lit' : ''}${
              wrongPad === p ? ' is-wrong' : ''
            }`}
            disabled={phase !== 'input'}
            onClick={() => pressRef.current(p)}
            aria-label={`${i + 1} 号灯`}
            title={`${i + 1} 号灯`}
          >
            <span className="pet-simon-num">{i + 1}</span>
          </button>
        ))}
      </div>

      {phase === 'idle' || phase === 'over' ? (
        <div className="pet-game-buttons">
          <button type="button" className="pet-game-btn" onClick={start}>
            <span>{phase === 'over' ? '再来一局' : '开始'}</span>
            <span className="pet-game-btn-label">空格</span>
          </button>
        </div>
      ) : null}
    </div>
  );
}
