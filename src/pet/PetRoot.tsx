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
 * - 右键 = 打开自绘菜单（无边框窗口没有系统菜单），复用签到 / 网关 / 呼出主窗口 / 隐藏动作。
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import type { CSSProperties, MouseEvent as ReactMouseEvent, PointerEvent as ReactPointerEvent } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { api } from '../lib/tauri';
import PetBubble from './PetBubble';
import PetSprite, { MASK_COLS, MASK_ROWS, type SpriteBox } from './PetSprite';
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
/** 空闲彩蛋间隔：长时间纯 idle 时偶尔来一发东张西望 / 发呆碎碎念 */
const IDLE_FLAVOR_MS = 45_000;
/** working 无进度判定：超过该时长没有新事件就播 mumble-screen（对屏碎碎念） */
const WORK_MUMBLE_MS = 45_000;
/**
 * 素材背景残留 alpha 的底噪（约 8% = 20/255）。
 * 渲染时把 ≤ 该值的 alpha 压成 0，否则在浅色窗口上会看出一个淡灰矩形。
 * 调大 → 背景更干净、角色边缘更硬；调小 → 边缘更柔、可能残留一点灰边。
 */
const ALPHA_FLOOR = 0.1;

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
  /**
   * 是否暂停解码。
   *
   * 初值取 false 而不是 `document.hidden`：窗口是 `visible:false` 建好后才 show，
   * 首启时 WebView2 常仍把页面报成 hidden，若据此置 true 且之后没有正确的
   * visibilitychange，画面就会一直空着（首启整块透明的另一半原因）。
   * 窗口真隐藏时 Rust 会发 pet-update(visible:false) 兜住。
   */
  const [paused, setPaused] = useState(false);
  const [idleTick, setIdleTick] = useState(0);
  const [playKey, setPlayKey] = useState(0);
  const [bubble, setBubble] = useState<{ seq: number; text: string } | null>(null);
  /** 右键菜单：位置 + 打开时探测到的网关运行态（决定「启动 / 停止」文案） */
  const [menu, setMenu] = useState<{ x: number; y: number; gateway: boolean } | null>(null);
  const [bubbleEnabled, setBubbleEnabled] = useState(true);
  const [opacity, setOpacity] = useState(1);
  const [sleepMinutes, setSleepMinutes] = useState(5);
  /** 角色可见包围盒（首帧 alpha 采样得出，窗口百分比）；null = 尚未测量 */
  const [hitBox, setHitBox] = useState<SpriteBox | null>(null);

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
  /** 已上报 Rust 的点击穿透掩码（并集，见 handleMask） */
  const maskRef = useRef<Uint8Array | null>(null);
  /** 瞬态最新值：定时器里判断"是否正忙"，避免彩蛋打断正在播的动作 */
  const transientRef = useRef<PetTransient | null>(null);
  transientRef.current = transient;
  /** 拖拽中：拖拽期间不插播彩蛋 */
  const draggingRef = useRef(false);
  draggingRef.current = dragging;
  /** working 无进度只碎碎念一次：新事件到来时复位 */
  const stallFiredRef = useRef(false);
  /** 菜单打开期间强制整窗可交互（否则菜单落到角色包围盒外会被点击穿透吃掉） */
  const menuOpenRef = useRef(false);

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
    stallFiredRef.current = false;

    // 睡梦中被新事件唤醒：先播 idle-wake 醒神，再进入实际状态
    if (coreRef.current === 'sleepy' && signal.type !== 'idle') {
      playTransient('idle', undefined, 'idle-wake');
    }

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
      // 纯瞬态信号：只叠加播一次动作，不改核心态
      case 'pace':
      case 'archive':
      case 'neutral':
      case 'turn':
      case 'jump':
        playTransient(signal.type, signal.text);
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
      // 窗口可见性以 Rust 下发为准：不再叠加 `document.hidden`——
      // 首启时 WebView2 可能一直把页面报成 hidden，叠加就会永远停在这儿
      if (patch.visible === false) setPaused(true);
      else if (patch.visible === true) setPaused(false);
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
        // 状态读取失败按「显示中」运行：宁可多播一段，也不要整块空白
        setPaused(false);
      });

    void listen<PetUpdatePayload>('pet-update', (event) => {
      if (!disposed) apply(event.payload ?? {});
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    });

    // 只在「页面变隐藏」时补暂停；恢复一律交给 pet-update(visible:true)，
    // 避免页面误报 hidden 后再也没人把它恢复回来
    const onVisibility = () => {
      if (document.hidden) setPaused(true);
    };
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
      // working 且长时间无进度 → 对屏碎碎念（mumble-screen），每次停顿只播一次
      if (
        coreRef.current === 'working' &&
        idleFor >= WORK_MUMBLE_MS &&
        !stallFiredRef.current &&
        !transientRef.current &&
        !draggingRef.current
      ) {
        stallFiredRef.current = true;
        playTransient('mumble', pickLine('working', lastLineRef.current));
        return;
      }
      if (idleFor < sleepMinutes * 60_000) return;
      // 只有空闲态会进打盹：业务态（working/sad）绝不因为"没新事件"就睡觉
      if (coreRef.current === 'idle') {
        // 入睡前先打个哈欠（idle-yawn），再接打盹循环
        playTransient('idle', undefined, 'idle-yawn');
        setCore('sleepy');
        showBubble(pickLine('sleepy', lastLineRef.current));
      }
    }, SLEEP_CHECK_MS);
    return () => window.clearInterval(timer);
  }, [sleepMinutes, showBubble, playTransient]);

  /* --------------------- idle 循环动作交替（避免单调） --------------------- */
  useEffect(() => {
    const timer = window.setInterval(() => setIdleTick((n) => n + 1), IDLE_SWAP_MS);
    return () => window.clearInterval(timer);
  }, []);

  /* ----------------------- 空闲彩蛋（低频） ----------------------- */
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (coreRef.current !== 'idle') return;
      if (transientRef.current || draggingRef.current) return;
      // 纯 idle 长时间无事件：偶尔东张西望 / 发呆碎碎念（idle-look / mumble-daze 随机）
      playTransient('idle', pickLine('idle', lastLineRef.current));
    }, IDLE_FLAVOR_MS);
    return () => window.clearInterval(timer);
  }, [playTransient]);

  /* ----------------------- 启动彩蛋：伸个懒腰 ----------------------- */
  useEffect(() => {
    const timer = window.setTimeout(() => playTransient('idle', undefined, 'idle-stretch'), 900);
    return () => window.clearTimeout(timer);
  }, [playTransient]);

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

  const handleMeasure = useCallback((box: SpriteBox) => setHitBox(box), []);

  /**
   * 点击穿透掩码：把每次采样的格子并集后上报 Rust（只增不减）。
   *
   * 为什么取并集而不是直接覆盖：动作切换后首帧可能还没稳定（起手式角色偏一点），
   * 覆盖式掩码会让角色刚露出来的部分被当成透明区，出现「点不到」。并集偏保守，
   * 最多是角色边缘多挡一点，不会漏点。
   */
  const handleMask = useCallback((cells: Uint8Array) => {
    const prev = maskRef.current;
    const next = prev && prev.length === cells.length ? prev : new Uint8Array(cells.length);
    let changed = !prev || prev.length !== cells.length;
    for (let i = 0; i < cells.length; i += 1) {
      if (cells[i] && !next[i]) {
        next[i] = 1;
        changed = true;
      }
    }
    if (!changed) return;
    maskRef.current = next;
    // 菜单打开期间不覆盖「整窗可交互」掩码，否则菜单会被 Rust 的穿透逻辑吃掉点击
    if (menuOpenRef.current) return;
    void api.pet.setHitMask(MASK_COLS, MASK_ROWS, Array.from(next)).catch(() => {
      /* 上报失败只影响点击穿透，不影响显示，静默忽略 */
    });
  }, []);

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

  const onPointerUp = (e: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    dragRef.current = null;

    if (drag?.started) {
      setDragging(false);
      // 落地反馈：接在拖拽悬空之后；click-shy 害羞 / idle-squat 蹲一下，随机取一个
      const landing = Math.random() < 0.5 ? 'click-shy' : 'idle-squat';
      playTransient('click', pickLine('click', lastLineRef.current), landing);
      return;
    }

    // 右键（contextmenu）不算单击：否则「右键开菜单」会顺带弹一句点击回应
    if (e.button !== 0) return;

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

  /* --------------------------- 右键菜单 --------------------------- */
  const closeMenu = useCallback(() => setMenu(null), []);

  // 右键打开菜单：顺带探一下网关运行态，决定菜单显示「启动 / 停止」
  const onContextMenu = (e: ReactMouseEvent<HTMLDivElement>) => {
    e.preventDefault();
    e.stopPropagation();
    const x = e.clientX;
    const y = e.clientY;
    void api.apiServer
      .status()
      .then((s) => setMenu({ x, y, gateway: !!s?.running }))
      .catch(() => setMenu({ x, y, gateway: false }));
  };

  const runCheckin = useCallback(() => {
    closeMenu();
    void api.checkin
      .start({ scope: 'all', skip_checked_in: true, skip_expired: false })
      .then(() => showBubble('开始签到…'))
      .catch(() => showBubble('签到启动失败'));
  }, [closeMenu, showBubble]);

  const toggleGateway = useCallback(
    (running: boolean) => {
      closeMenu();
      void (running ? api.apiServer.stop() : api.apiServer.start())
        .then(() => showBubble(running ? '网关已停止' : '网关已启动'))
        .catch(() => showBubble('网关操作失败'));
    },
    [closeMenu, showBubble],
  );

  const focusMainFromMenu = useCallback(() => {
    closeMenu();
    void api.pet.focusMain().catch(() => {});
  }, [closeMenu]);

  const hideFromMenu = useCallback(() => {
    closeMenu();
    void api.pet.hide().catch(() => {});
  }, [closeMenu]);

  // Esc 关闭菜单（无边框窗口里没有系统菜单，键盘兜底更稳）
  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setMenu(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [menu]);

  // 菜单开合 ↔ 点击穿透：打开时把掩码置为「整窗可交互」，关闭后恢复真实掩码。
  // 否则菜单若落在角色包围盒外，会被 Rust 的透明区穿透逻辑吃掉点击（按钮点不动）
  useEffect(() => {
    menuOpenRef.current = menu !== null;
    const cells = menu
      ? Array.from({ length: MASK_COLS * MASK_ROWS }, () => 1)
      : maskRef.current
        ? Array.from(maskRef.current)
        : null;
    if (cells) {
      void api.pet.setHitMask(MASK_COLS, MASK_ROWS, cells).catch(() => {});
    }
  }, [menu]);

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

  // 热区：测量前用偏保守的居中区域（覆盖角色主体，避免"点了没反应"）；
  // 测量后严格贴合 alpha 包围盒，左右/上各留少量容错，方便点到头发边缘
  const hitStyle: CSSProperties = hitBox
    ? {
        // alpha 阈值会把头发/裙摆边缘的半透明像素排掉，故四周各补一点余量，
        // 但远小于"整窗可点"——空白区域（左右两侧与顶部大片）仍然不响应
        left: `${Math.max(0, hitBox.left - 4)}%`,
        top: `${Math.max(0, hitBox.top - 3)}%`,
        width: `${Math.min(100, hitBox.width + 8)}%`,
        height: `${Math.min(100, hitBox.height + 4)}%`,
      }
    : { left: '14%', top: '52%', width: '72%', height: '48%' };

  // 气泡底边贴在角色头顶上方 8px（距底部 = 100 - top%）
  const bubbleBottom = hitBox ? `calc(${(100 - hitBox.top).toFixed(2)}% + 8px)` : '48%';

  return (
    <div className={`pet-root ${dragging ? 'dragging' : ''}`} style={{ opacity }}>
      {/*
        素材背景残留 alpha（约 8%，浅色背景上能看出淡灰矩形）在渲染层压掉：
        feComponentTransfer 把 A 通道整体下移一个底噪再线性拉伸回 0~1，
        等价于 a' = (a - FLOOR) / (1 - FLOOR)。编码层改不了（本机无 libvpx，
        VP9+alpha 只能硬件编码、不支持 yuva420p），所以在合成阶段处理。
      */}
      <svg className="pet-filter-defs" aria-hidden="true" focusable="false">
        <defs>
          <filter id="pet-alpha-floor" colorInterpolationFilters="sRGB">
            <feComponentTransfer>
              <feFuncA
                type="linear"
                slope={1 / (1 - ALPHA_FLOOR)}
                intercept={-ALPHA_FLOOR / (1 - ALPHA_FLOOR)}
              />
            </feComponentTransfer>
          </filter>
        </defs>
      </svg>
      <PetSprite
        actionId={actionId}
        playKey={playKey}
        paused={paused}
        onEnded={onSpriteEnded}
        onMeasure={handleMeasure}
        onMask={handleMask}
      />
      {/* 气泡锚点：底边贴在角色头顶上方，而不是窗口顶部 */}
      {bubbleEnabled && bubble ? (
        <div className="pet-bubble-anchor" style={{ bottom: bubbleBottom }}>
          <PetBubble seq={bubble.seq} text={bubble.text} onDone={clearBubble} />
        </div>
      ) : null}
      {/*
        点击/拖拽热区：只覆盖角色的可见包围盒。
        根节点 pointer-events:none，窗口里的空白区域不再触发动作，
        也不会把「点空白」误判成单击回应。
      */}
      <div
        className={`pet-hit ${dragging ? 'dragging' : ''}`}
        style={hitStyle}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onDoubleClick={onDoubleClick}
        onContextMenu={onContextMenu}
      />
      {/*
        右键菜单：无边框窗口没有系统菜单，这里自绘一个轻量卡片。
        位置夹紧在窗口内；backdrop 覆盖全窗以支持「点外部关闭」。
      */}
      {menu ? (
        <div className="pet-menu-backdrop" onPointerDown={closeMenu} onContextMenu={(e) => e.preventDefault()}>
          <div
            className="pet-menu"
            style={{
              left: Math.max(4, Math.min(menu.x, window.innerWidth - 152)),
              top: Math.max(4, Math.min(menu.y, window.innerHeight - 148)),
            }}
            onPointerDown={(e) => e.stopPropagation()}
          >
            <button type="button" onClick={runCheckin}>
              一键签到
            </button>
            <button type="button" onClick={() => toggleGateway(menu.gateway)}>
              {menu.gateway ? '停止 API 网关' : '启动 API 网关'}
            </button>
            <div className="pet-menu-sep" />
            <button type="button" onClick={focusMainFromMenu}>
              呼出主窗口
            </button>
            <button type="button" onClick={hideFromMenu}>
              隐藏桌宠
            </button>
          </div>
        </div>
      ) : null}
    </div>
  );
}
