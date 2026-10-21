/**
 * 桌宠互动小游戏：石头剪刀布（P9）
 *
 * 三条约束与内联对话输入条同源（都踩过，不是假设）：
 * 1. **点击穿透**：窗口按 sprite alpha 掩码 40Hz 切换 `set_ignore_cursor_events`，
 *    本面板不属于 sprite → 不暂停就会被判成透明区，三个出招按钮"看得见点不动"。
 *    挂载即 `setChatFocus(true)`（暂停穿透线程），卸载复位；PetRoot 另把面板矩形
 *    并进掩码（双保险，与输入条同一套 `overlayRectOnMask`）。
 * 2. **键盘/焦点**：窗口是 `focus:false` 建的，`focusForInput()` 让 Rust 激活窗口。
 * 3. **不自带动作播放**：组件只算结果，把 `reactionAction` / `linePool` 交给
 *    `PetRoot.playTransient`——游戏 UI 与 106 个动作素材彻底解耦，换素材不用改这里。
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../lib/tauri';
import {
  applyOutcome,
  isValidMove,
  judgeRound,
  linePoolForOutcome,
  petMove,
  reactionAction,
  updateStreak,
  type RpsLinePool,
  type RpsMove,
  type RpsOutcome,
  type RpsScore,
} from './games';

/** 出招的 emoji（只在展示层用；语义层见 games.ts 的 RpsMove） */
const MOVE_FACE: Record<RpsMove, string> = {
  rock: '✊',
  paper: '✋',
  scissors: '✌️',
};

const MOVE_NAME: Record<RpsMove, string> = {
  rock: '石头',
  paper: '布',
  scissors: '剪刀',
};

/** 结果文案（pet 视角） */
const OUTCOME_TEXT: Record<RpsOutcome, string> = {
  win: '我赢啦',
  lose: '你赢了',
  draw: '打平',
};

/** 一轮结束交给 PetRoot 的载荷（纯数据，不含 React 状态） */
export type RpsRoundPayload = {
  playerMove: RpsMove;
  petMove: RpsMove;
  outcome: RpsOutcome;
  /** 要播的动作 id（见 games.reactionAction） */
  action: string;
  /** 台词池 key（PetRoot 用 pickLine 取一句） */
  linePool: RpsLinePool;
  /** 当前连续同侧胜场数（连击阈值判定用） */
  streak: number;
  score: RpsScore;
};

type Props = {
  onClose: () => void;
  /** 一轮判定完成：由 PetRoot 播放动作 + 弹台词 */
  onRoundEnd: (payload: RpsRoundPayload) => void;
};

type LastRound = {
  player: RpsMove;
  pet: RpsMove;
  outcome: RpsOutcome;
};

export default function PetRpsGame({ onClose, onRoundEnd }: Props) {
  const [score, setScore] = useState<RpsScore>({ pet: 0, player: 0 });
  const [streak, setStreak] = useState<{ kind: RpsOutcome | null; count: number }>({
    kind: null,
    count: 0,
  });
  const [last, setLast] = useState<LastRound | null>(null);
  /** 出招到揭晓之间的一小段"顿"：太快会像随机数刷屏，没有对局的仪式感 */
  const [revealing, setRevealing] = useState(false);

  /** 挂载即暂停穿透；卸载复位（含异常路径，保证不会把穿透永久关掉） */
  useEffect(() => {
    void api.pet.setChatFocus(true).catch(() => {});
    void api.pet.focusForInput().catch(() => {});
    return () => {
      void api.pet.setChatFocus(false).catch(() => {});
    };
  }, []);

  /** 组件卸载后仍有定时器在跑时不要 setState（React 会警告） */
  const aliveRef = useRef(true);
  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  const play = useCallback(
    (move: RpsMove) => {
      if (revealing || !isValidMove(move)) return;
      const pet = petMove();
      setRevealing(true);
      // 260ms 的"定格"：让玩家先看到自己出的招，再看到桌宠出招，才有回合感
      window.setTimeout(() => {
        if (!aliveRef.current) return;
        const outcome = judgeRound(pet, move);
        const nextScore = applyOutcome(score, outcome);
        const nextStreak = updateStreak(streak, outcome);
        setScore(nextScore);
        setStreak(nextStreak);
        setLast({ player: move, pet, outcome });
        setRevealing(false);
        onRoundEnd({
          playerMove: move,
          petMove: pet,
          outcome,
          action: reactionAction(outcome, nextStreak),
          linePool: linePoolForOutcome(outcome, nextStreak),
          streak: nextStreak.kind === outcome ? nextStreak.count : 0,
          score: nextScore,
        });
      }, 260);
    },
    [revealing, score, streak, onRoundEnd],
  );

  // 键盘 1/2/3 也可出招（窗口已被 focusForInput 激活）：手不用来回找鼠标
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
        return;
      }
      const byDigit: Record<string, RpsMove> = { '1': 'rock', '2': 'paper', '3': 'scissors' };
      const move = byDigit[e.key];
      if (move) {
        e.preventDefault();
        play(move);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [play, onClose]);

  return (
    <div className="pet-game" onPointerDown={(e) => e.stopPropagation()}>
      <div className="pet-game-head">
        <span className="pet-game-score">
          我 {score.pet} : {score.player} 你
        </span>
        <button type="button" className="pet-game-close" onClick={onClose} title="结束对局（Esc）">
          结束
        </button>
      </div>

      <div className="pet-game-stage" aria-live="polite">
        {revealing ? (
          <span className="pet-game-vs">石头剪刀…布！</span>
        ) : last ? (
          <span className="pet-game-vs">
            你 {MOVE_FACE[last.player]} 我 {MOVE_FACE[last.pet]} ·{' '}
            <b className={`pet-game-outcome is-${last.outcome}`}>{OUTCOME_TEXT[last.outcome]}</b>
          </span>
        ) : (
          <span className="pet-game-vs">出一招吧</span>
        )}
      </div>

      <div className="pet-game-buttons">
        {(['rock', 'paper', 'scissors'] as RpsMove[]).map((m, i) => (
          <button
            key={m}
            type="button"
            className="pet-game-btn"
            disabled={revealing}
            onClick={() => play(m)}
            title={`${MOVE_NAME[m]}（${i + 1}）`}
          >
            <span aria-hidden>{MOVE_FACE[m]}</span>
            <span className="pet-game-btn-label">{MOVE_NAME[m]}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
