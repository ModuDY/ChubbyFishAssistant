/**
 * 桌宠窗口根组件（`index.html?pet=1`）
 *
 * 职责边界：
 * - **只读**业务事件，绝不回写账号/签到/积分数据（宠物只是"状态化身"）；
 * - 窗口显隐、位置、缩放由 Rust 侧 `commands/pet.rs` 负责，这里只调用命令；
 * - 不跑 `useAppStore.init()`：桌宠不需要账号/分组/快照统计（快照体积统计是重 IO）。
 *
 * 交互约定（P0）：
 * - 拖拽 = 按住移动超过 5px（调用 `startDragging`），松开播落地反馈并记住位置；
 * - 单击 = 随机点击回应 + 台词；双击 = 呼出主窗口（单击延迟 240ms 给双击让路）；
 * - 右键菜单禁用（窗口无边框，菜单会飞出屏幕）。
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from '../lib/tauri';
import PetBubble from './PetBubble';
import PetSprite from './PetSprite';
import { coreAction, makeTransient, raise } from './petState';
import type { PetCoreState, PetTransient } from './petState';
import { pickLine } from './lines';
import { usePetEvents } from './usePetEvents';
import type { PetSignal } from './usePetEvents';
import './pet.css';

/** 单击与双击的判定窗口（ms）：小于该间隔的第二次按下视为双击 */
const CLICK_DELAY = 240;
/** 拖拽判定阈值（px）：小于该位移视为点击，避免手抖被当成拖拽 */
const DRAG_THRESHOLD = 5;
/** 失败态停留时长：给用户足够时间看到"出错了"，然后自行回落 */
const SAD_HOLD_MS = 9000;
/** idle 循环动作交替间隔：长时间一个循环动作会显得死板 */
const IDLE_SWAP_MS = 24_000;
/** 打盹检测轮询间隔 */
const SLEEP_CHECK_MS = 20_000;

type PetUpdatePayload = {
  visible?: boolean;
  bubble?: boolean;
  opacity?: number;
  scale?: number;
  sleepMinutes?: number;
};

export default function PetRoot() {
  const [core, setCore] = useState<PetCoreState>('idle');
  const [transient, setTransient] = useState<PetTransient | null>(null);
  const [dragging, setDragging] = useState(false);
  const [paused, setPaused] = useState(() => document.hidden);
  const [idleTick, setIdleTick] = useState(0);
  const [playKey, setPlayKey] = useState(0);
  const [bubble, setBubble] = useState<{ seq: number; text: string } | null>(null);
  const [bubbleEnabled, setBubbleEnabled] = useState(true);
  const [opacity, setOpacity] = useState(1);
  const [sleepMinutes, setSleepMinutes] = useState(5);

  /** 核心态最新值：事件回调与定时器里避免读到闭包旧值 */
  const coreRef = useRef(core);
  coreRef.current = core;

  const seqRef = useRef(0);
  const bubbleSeqRef = useRef(0);
  const lastEventRef = useRef(Date.now());
  const lastLineRef = useRef<string | undefined>(undefined);
  const sadTimerRef = useRef<number | null>(null);
  const clickTimerRef = useRef<number | null>(null);
  const dragRef = useRef<{ x: number; y: number; started: boolean } | null>(null);

  // 透明窗口要求 html/body 无背景：class 只在本页挂载期间存在
  useEffect(() => {
    document.documentElement.classList.add('pet-window');
    document.body.classList.add('pet-window');
    return () => {
      document.documentElement.classList.remove('pet-window');
      document.body.classList.remove('pet-window');
    };
  }, []);

  const showBubble = useCallback((text?: string) => {
    if (!text) return;
    lastLineRef.current = text;
    bubbleSeqRef.current += 1;
    setBubble({ seq: bubbleSeqRef.current, text });
  }, []);

  const clearSadTimer = useCallback(() => {
    if (sadTimerRef.current) {
      window.clearTimeout(sadTimerRef.current);
      sadTimerRef.current = null;
    }
  }, []);

  /** 播放一次性动作（happy / rich / broke / click），到时自动回落 */
  const playTransient = useCallback(
    (kind: PetTransient['kind'], text?: string, actionOverride?: string) => {
      seqRef.current += 1;
      const t = makeTransient(seqRef.current, kind, text, actionOverride);
      setTransient(t);
      setPlayKey(t.seq);
      showBubble(t.text);
      window.setTimeout(() => {
        setTransient((cur) => (cur && cur.seq === t.seq ? null : cur));
      }, t.ms);
    },
    [showBubble],
  );

  const enterSad = useCallback(
    (text?: string) => {
      clearSadTimer();
      setCore('sad');
      showBubble(text ?? pickLine('sad', lastLineRef.current));
      sadTimerRef.current = window.setTimeout(() => {
        sadTimerRef.current = null;
        setCore('idle');
      }, SAD_HOLD_MS);
    },
    [clearSadTimer, showBubble],
  );

  /* ------------------------- 业务事件 → 状态 ------------------------- */
  usePetEvents((signal: PetSignal) => {
    lastEventRef.current = Date.now();

    switch (signal.type) {
      case 'working':
        // 注意**不能**在这里清 sadTimer：raise 不会把 sad 降级，
        // 清了计时器就会永远停在失败表情上
        setCore((cur) => raise(cur, 'working'));
        showBubble(signal.text);
        break;
      case 'thinking':
        // 思考优先级低于进行中：不会打断签到
        setCore((cur) => raise(cur, 'thinking'));
        showBubble(signal.text);
        break;
      case 'sad':
        enterSad(signal.text);
        break;
      case 'happy':
        playTransient('happy', signal.text ?? pickLine('happy', lastLineRef.current));
        break;
      case 'rich':
        playTransient('rich', signal.text ?? pickLine('rich', lastLineRef.current));
        break;
      case 'broke':
        playTransient('broke', signal.text ?? pickLine('broke', lastLineRef.current));
        break;
      case 'idle':
      default:
        // 轮次结束是强信号：直接回落（exit 事件常跟在 done 之后，
        // 若还死守"失败态不可打断"就会一直停在 sad）
        clearSadTimer();
        setCore('idle');
        showBubble(signal.text);
        break;
    }
  });

  /* --------------------- 隐藏即停（窗口不可见） --------------------- */
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;

    const apply = (patch: PetUpdatePayload) => {
      if (patch.visible === false) setPaused(true);
      else if (patch.visible === true) setPaused(document.hidden);
      if (typeof patch.bubble === 'boolean') setBubbleEnabled(patch.bubble);
      if (typeof patch.opacity === 'number') {
        setOpacity(Math.min(1, Math.max(0.3, patch.opacity)));
      }
      if (typeof patch.sleepMinutes === 'number' && patch.sleepMinutes > 0) {
        setSleepMinutes(patch.sleepMinutes);
      }
    };

    void api.pet
      .status()
      .then((s) => {
        apply({
          visible: s.visible,
          bubble: s.bubble,
          opacity: s.opacity,
          sleepMinutes: s.sleepMinutes,
        });
      })
      .catch(() => {
        /* 状态读取失败按默认值运行，不影响显示 */
      });

    void listen<PetUpdatePayload>('pet-update', (event) => {
      if (!disposed) apply(event.payload ?? {});
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    });

    const onVisibility = () => setPaused(document.hidden);
    document.addEventListener('visibilitychange', onVisibility);

    return () => {
      disposed = true;
      unlisten?.();
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, []);

  /* ----------------------- 打盹（长时间无事件） ----------------------- */
  useEffect(() => {
    const timer = window.setInterval(() => {
      const idleFor = Date.now() - lastEventRef.current;
      if (idleFor < sleepMinutes * 60_000) return;
      // 只有空闲态会进打盹：业务态（working/sad）绝不因为"没新事件"就睡觉
      if (coreRef.current === 'idle') {
        setCore('sleepy');
        showBubble(pickLine('sleepy', lastLineRef.current));
      }
    }, SLEEP_CHECK_MS);
    return () => window.clearInterval(timer);
  }, [sleepMinutes, showBubble]);

  /* --------------------- idle 循环动作交替（避免单调） --------------------- */
  useEffect(() => {
    const timer = window.setInterval(() => setIdleTick((n) => n + 1), IDLE_SWAP_MS);
    return () => window.clearInterval(timer);
  }, []);

  /* --------------------------- 位置记忆 --------------------------- */
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    let timer: number | undefined;

    void getCurrentWindow()
      .onMoved(() => {
        // 拖动过程会连续触发 moved：去抖 500ms 再落盘，避免高频写 SQLite
        if (timer) window.clearTimeout(timer);
        timer = window.setTimeout(() => {
          void api.pet.savePosition().catch(() => {});
        }, 500);
      })
      .then((un) => {
        if (disposed) un();
        else unlisten = un;
      });

    return () => {
      disposed = true;
      unlisten?.();
      if (timer) window.clearTimeout(timer);
    };
  }, []);

  /* --------------------------- 交互 --------------------------- */
  const playClick = useCallback(() => {
    playTransient('click', pickLine('click', lastLineRef.current));
  }, [playTransient]);

  /** 稳定引用：否则父组件每次渲染都会重置气泡的自动消失计时器 */
  const clearBubble = useCallback(() => setBubble(null), []);

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    dragRef.current = { x: e.clientX, y: e.clientY, started: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.started) return;
    if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < DRAG_THRESHOLD) return;
    drag.started = true;
    setDragging(true);
    void getCurrentWindow()
      .startDragging()
      .catch(() => setDragging(false));
  };

  const onPointerUp = () => {
    const drag = dragRef.current;
    dragRef.current = null;

    if (drag?.started) {
      setDragging(false);
      // 落地反馈：接在拖拽悬空之后（上游同样用 click-shy 作为释放反应）
      playTransient('click', pickLine('click', lastLineRef.current), 'click-shy');
      return;
    }

    // 单击延后执行，给双击让路；连点时不叠加多条计时器
    if (clickTimerRef.current) return;
    clickTimerRef.current = window.setTimeout(() => {
      clickTimerRef.current = null;
      playClick();
    }, CLICK_DELAY);
  };

  const onDoubleClick = () => {
    if (clickTimerRef.current) {
      window.clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
    }
    void api.pet.focusMain().catch(() => {});
  };

  /* --------------------------- 播放调度 --------------------------- */
  const actionId = dragging ? 'drag-dangle' : (transient?.action ?? coreAction(core, idleTick));

  const onSpriteEnded = useCallback(() => {
    if (transient) {
      setTransient(null);
      return;
    }
    // 非循环的核心动作（sad）播完即回落，避免停在最后一帧
    if (core === 'sad') setCore('idle');
  }, [transient, core]);

  return (
    <div
      className={`pet-root ${dragging ? 'dragging' : ''}`}
      style={{ opacity }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={onDoubleClick}
      onContextMenu={(e) => e.preventDefault()}
    >
      <PetSprite
        actionId={actionId}
        playKey={playKey}
        paused={paused}
        onEnded={onSpriteEnded}
      />
      {bubbleEnabled && bubble ? (
        <PetBubble seq={bubble.seq} text={bubble.text} onDone={clearBubble} />
      ) : null}
    </div>
  );
}
