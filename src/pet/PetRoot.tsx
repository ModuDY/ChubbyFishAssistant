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
import { coreAction, makeTransient, pickTransientAction, raise } from './petState';
import type { PetCoreState, PetTransient } from './petState';
import { applyPetPack, ACTION_LIST, ACTIONS } from './actions';
import { pickLine } from './lines';
import { localDateKey, getTodaysFestivalPlan } from './festivals';
import { pickFlavorAction, pickSlotAction, pushRecent, timeSlot } from './flavor';
import { usePetEvents } from './usePetEvents';
import type { PetSignal } from './usePetEvents';
import { usePetSay, type SayState } from './usePetSay';
import { usePetAutoSay, AUTO_TRIGGER_TYPES } from './usePetAutoSay';
import { usePetGatewayEvents } from './usePetGatewayEvents';
import type { PetAiMode } from '../types';
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
/**
 * 空闲彩蛋 / 节日 / 时段的默认间隔（分钟）。
 * 实际间隔由设置项 `pet_easter_egg_minutes` 决定（默认 1 分钟）；这里只是默认值的兜底。
 * 全量入库上游 75 个彩蛋后，池子从 3 个动作变成 50+，靠分桶轮转保证每类都能轮到，
 * 间隔比原来的固定 45s 略放宽，避免连续动作把待机画面刷得太忙。
 */
const DEFAULT_EASTER_EGG_MINUTES = 1;
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
  /** P3：AI 档位（off/manual/auto） */
  aiMode?: string;
  /** P3：人格档位 */
  persona?: string;
  /** P4-3：当前素材包（builtin = 内置素材） */
  pack?: string;
  /** P6：节日 + 时段触发开关（默认开） */
  anniversary?: boolean;
  /** P6：空闲随机彩蛋开关（默认开） */
  easterEgg?: boolean;
  /** P6：彩蛋 / 节日 / 时段的触发间隔（分钟，默认 1） */
  easterEggMinutes?: number;
  /**
   * API 网关运行态（Rust 在每次启停时推送）。
   * 用于让**已打开的**右键菜单立即改写「启动 / 停止」文案，避免两侧菜单状态不一致。
   */
  gatewayRunning?: boolean;
  /** P5：网关活动联动开关（设置页改完即时下发；设计 §6.5） */
  gatewayLink?: boolean;
};

/**
 * 每次换一个「切入点」：同一情境反复用同一句指令时，模型会复读同一个开头。
 * 只做角度扰动，不追加任何格式要求（格式要求会让它先在思考里"规划怎么满足"）。
 */
const SAY_ANGLES = ['随口说一句', '吐槽一句', '关心我一句', '跟我撒个娇'];

/** 组装给 AI 的提问：只含状态摘要与时间，不含账号名/邮箱/金额等敏感信息 */
function buildSayPrompt(context?: string): string {
  const now = new Date();
  const h = now.getHours();
  const greet = h < 6 ? '凌晨' : h < 12 ? '早上' : h < 14 ? '中午' : h < 18 ? '下午' : '晚上';
  // 口语化时间（「晚上 7 点 47」而不是「19:47」）：给 24 小时制时模型会说成
  // 「都七点四十七啦」，在晚上语境里读着别扭
  const hour12 = h % 12 === 0 ? 12 : h % 12;
  const clock = `${greet}${hour12}点${String(now.getMinutes()).padStart(2, '0')}分`;
  const angle = SAY_ANGLES[Math.floor(Math.random() * SAY_ANGLES.length)];
  // 给「情境」而不是「要求」：知道现在几点、刚发生了什么，才有话可说
  return context
    ? `现在是${clock}。刚发生：${context}。${angle}，就着这件事。`
    : `现在是${clock}，我正对着电脑。${angle}。`;
}

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
  /** AI 气泡结束的时刻：之后 1.5s 内不再弹普通播报气泡，避免画面"一跳" */
  const [sayEndAt, setSayEndAt] = useState(0);
  /** 右键菜单位置（网关运行态由 `gatewayRunning` 统一提供，见下） */
  /**
   * 右键菜单：坐标 + 当前视图。
   * 二级视图（动作点播 / 切换形象）复用同一个卡片换内容，而不是向右再展开一层——
   * 窗口右侧只有留给菜单的 160px，再向右会被窗口裁掉。
   */
  const [menu, setMenu] = useState<{
    x: number;
    y: number;
    view: 'main' | 'actions' | 'packs';
  } | null>(null);
  /** 切换形象：进入 packs 视图时才拉取，避免每次右键都扫一遍用户目录 */
  const [packs, setPacks] = useState<Array<{ id: string; name: string }>>([]);
  /** 动作点播视图里已展开的分组名（默认全部收起，见 toggleGroup） */
  const [openGroups, setOpenGroups] = useState<Set<string>>(new Set());
  /**
   * API 网关运行态（全局单一来源）：
   * - 初始 / 打开菜单时经 `apiServer.status()` 探测；
   * - 之后由 Rust 每次启停推送的 `pet-update{gatewayRunning}` 实时刷新。
   * 两个菜单（托盘 + 桌宠）因此始终显示同一状态，不会各写各的。
   */
  const [gatewayRunning, setGatewayRunning] = useState(false);
  /** 当前生效的素材包 id（切换形象菜单用它标记选中项） */
  const [currentPack, setCurrentPack] = useState('builtin');
  /** P5：网关活动联动开关（默认关；来自 Rust pet-update / pet_status，设计 §6.5） */
  const [gatewayLink, setGatewayLink] = useState(false);
  /** P6：节日 + 时段触发（默认开；来自 Rust pet-update / pet_status） */
  const [anniversaryEnabled, setAnniversaryEnabled] = useState(true);
  /** P6：空闲随机彩蛋（默认开） */
  const [easterEggEnabled, setEasterEggEnabled] = useState(true);
  /** P6：彩蛋 / 节日 / 时段触发间隔（分钟，默认 1） */
  const [easterEggMinutes, setEasterEggMinutes] = useState(DEFAULT_EASTER_EGG_MINUTES);
  const [bubbleEnabled, setBubbleEnabled] = useState(true);
  const [opacity, setOpacity] = useState(1);
  const [sleepMinutes, setSleepMinutes] = useState(5);
  /** AI 对话档位（P3；Rust 下发，默认 off = 只用本地台词） */
  const [aiMode, setAiMode] = useState<PetAiMode>('off');
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
  /** 各动作实测时长（ms）缓存：key = actionId，用于瞬态兜底计时 */
  const durationRef = useRef<Record<string, number>>({});
  /** P6：已播过的节日（键 `YYYY-MM-DD#festival`；纯内存，重启后当天可再播一次） */
  const festivalFiredRef = useRef<Set<string>>(new Set());
  /** P6：已播过的时段（键 `YYYY-MM-DD#slot`；每个时段每天只播一次） */
  const slotFiredRef = useRef<Set<string>>(new Set());
  /** P6：彩蛋分桶轮次（每触发一次 +1，用于依次轮换 4 个桶） */
  const flavorTickRef = useRef(0);
  /** P6：最近播过的彩蛋动作（新→旧，避免短期内重复） */
  const recentFlavorRef = useRef<string[]>([]);
  /** 当前正在播的动作 id（onDuration 回调里需要知道是哪个动作） */
  const actionIdRef = useRef('');
  /** 拖拽中：拖拽期间不插播彩蛋 */
  const draggingRef = useRef(false);
  draggingRef.current = dragging;
  /** working 无进度只碎碎念一次：新事件到来时复位 */
  const stallFiredRef = useRef(false);
  /** 菜单打开期间强制整窗可交互（否则菜单落到角色包围盒外会被点击穿透吃掉） */
  const menuOpenRef = useRef(false);
  /** 暂停态最新值（auto 档「别打扰」判定用） */
  const pausedRef = useRef(false);
  pausedRef.current = paused;
  /** AI 流式态最新值（同上） */
  const sayRef = useRef<SayState>(null);
  /** 最近一次业务事件摘要（供 AI 回复的"背景"） */
  const lastEventSummaryRef = useRef<string | undefined>(undefined);
  /** 最近一次发言的触发来源：决定失败时是否回落本地台词（自动档静默，避免刷屏） */
  const lastTriggerRef = useRef<'user' | 'auto'>('user');
  /** 本次发言是否由我们把核心态抬到了 thinking（结束只回退自己抬高的一档） */
  const sayThinkingRef = useRef(false);

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
      const action = pickTransientAction(kind, actionOverride);
      // 兜底时长按该素材的实测时长算（PetSprite loadedmetadata 上报），
      // 否则 10s 的素材会在第 4s 被掐断
      const t = makeTransient(seqRef.current, kind, action, text, durationRef.current[action]);
      setTransient(t);
      setPlayKey(t.seq);
      showBubble(t.text);
      window.setTimeout(() => {
        setTransient((cur) => (cur && cur.seq === t.seq ? null : cur));
      }, t.ms);
    },
    [showBubble],
  );

  /** 记录各动作的实测时长（ms），供上面的兜底计时器使用 */
  const handleDuration = useCallback((ms: number) => {
    const id = actionIdRef.current;
    if (id && ms > 0) durationRef.current[id] = ms;
  }, []);

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

  /* ------------------------- AI 发言（P3） ------------------------- */
  const handleSayFallback = useCallback(
    (reason: string) => {
      // 档位关闭/网关未启动等"预期内"的拒绝不提示；主动触发给一句本地台词兜底，
      // 自动档静默（否则会变成刷屏）
      if (lastTriggerRef.current !== 'user') return;
      if (reason.includes('未开启')) return;
      showBubble(pickLine('idle', lastLineRef.current));
    },
    [showBubble],
  );

  const { say, startSay, cancelSay } = usePetSay({
    // 首个 reasoning delta → 点亮「深度思考碎碎念」（上游大肥鱼梗的原始设定）
    onReasoningFirst: () => playTransient('idle', undefined, 'mumble-deep'),
    onFallback: handleSayFallback,
  });
  sayRef.current = say;

  /** 主动触发（右键「跟我聊一句」） */
  const sayNow = useCallback(() => {
    lastTriggerRef.current = 'user';
    const ctx = lastEventSummaryRef.current;
    void startSay(buildSayPrompt(ctx), ctx ?? null, 'user');
  }, [startSay]);

  /** 自动点评（auto 档）：节流 + 「别打扰」判定都在 usePetAutoSay 内 */
  const { say: autoSay } = usePetAutoSay({
    enabled: aiMode === 'auto',
    isBusy: () =>
      pausedRef.current ||
      !bubbleEnabled ||
      draggingRef.current ||
      !!transientRef.current ||
      !!sayRef.current?.streaming ||
      document.hidden,
    startSay: (prompt, context, trigger) => {
      lastTriggerRef.current = trigger ?? 'auto';
      return startSay(prompt, context, trigger);
    },
  });

  /* ------------------------- 业务事件 → 状态 ------------------------- */
  /**
   * 业务事件与网关活动（P5）共用的信号处理：两者都产出 `PetSignal`，走同一状态机
   * （`raise` 优先级聚合）与同一套气泡/瞬态逻辑。
   * 用普通箭头函数（每次渲染新引用无妨）：两个 hook 内部都以 ref 持有最新回调，
   * 不会因它换引用而重订阅。
   */
  const handleSignal = (signal: PetSignal) => {
    lastEventRef.current = Date.now();
    stallFiredRef.current = false;
    // 记录最近事件摘要，供 AI 回复当"背景"（只含状态文案，不含账号/金额）
    if (signal.text) lastEventSummaryRef.current = signal.text;

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

    // auto 档点评：只在「有内容可说」的信号且带文案时触发（节流与静默判定在 hook 内）
    if (aiMode === 'auto' && AUTO_TRIGGER_TYPES.has(signal.type) && signal.text) {
      autoSay(buildSayPrompt(signal.text), signal.text);
    }
  };

  /** 业务流（签到 / 切换 / 积分）是否进行中：网关 burst 结束时据此避让核心态回落 */
  const businessActiveRef = useRef(false);
  usePetEvents((signal) => {
    // working/thinking = 一笔业务流进行中；idle = 业务流收尾（done / exit）
    if (signal.type === 'working' || signal.type === 'thinking') businessActiveRef.current = true;
    else if (signal.type === 'idle') businessActiveRef.current = false;
    handleSignal(signal);
  });
  // 网关活动联动（P5，设计 §6.5）：仅在设置项开启时订阅 `gateway-activity`；
  // 一段活跃结束时若业务流进行中则避让（不强降核心态、不叠庆祝），避免打断 working/sad
  usePetGatewayEvents(handleSignal, gatewayLink, {
    isBusinessBusy: () => businessActiveRef.current || coreRef.current === 'sad',
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
      if (patch.aiMode === 'off' || patch.aiMode === 'manual' || patch.aiMode === 'auto') {
        setAiMode(patch.aiMode);
      }
      // 网关启停：Rust 推送的运行态直接驱动菜单文案（两个菜单共用此单一状态）
      if (typeof patch.gatewayRunning === 'boolean') {
        setGatewayRunning(patch.gatewayRunning);
      }
      // P5：网关活动联动开关（默认关）
      if (typeof patch.gatewayLink === 'boolean') {
        setGatewayLink(patch.gatewayLink);
      }
      // P4-3：素材包切换 → 重新装载动作表（包里缺的动作回落内置；加载失败也回落内置）
      if (typeof patch.pack === 'string') {
        setCurrentPack(patch.pack || 'builtin');
        void applyPetPack(patch.pack);
      }
      // P6：节日 / 时段 / 彩蛋三个开关与间隔
      if (typeof patch.anniversary === 'boolean') setAnniversaryEnabled(patch.anniversary);
      if (typeof patch.easterEgg === 'boolean') setEasterEggEnabled(patch.easterEgg);
      if (typeof patch.easterEggMinutes === 'number' && patch.easterEggMinutes > 0) {
        setEasterEggMinutes(Math.min(30, Math.max(1, Math.round(patch.easterEggMinutes))));
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
          aiMode: s.aiMode,
          pack: s.pack,
          gatewayLink: s.gatewayLink,
          anniversary: s.anniversary,
          easterEgg: s.easterEgg,
          easterEggMinutes: s.easterEggMinutes,
        });
      })
      .catch(() => {
        // 状态读取失败按「显示中」运行：宁可多播一段，也不要整块空白
        setPaused(false);
      });

    // 网关运行态初值：Rust 只在启停时推送，窗口加载晚于启动时会漏掉这次推送，故补探一次
    void api.apiServer
      .status()
      .then((s) => {
        if (!disposed) setGatewayRunning(!!s?.running);
      })
      .catch(() => {});

    void listen<PetUpdatePayload>('pet-update', (event) => {
      if (!disposed) apply(event.payload ?? {});
    }).then((un) => {
      if (disposed) un();
      else unlisten = un;
    });

    // 页面变隐藏时补暂停；变可见时**仅当窗口真在显示**才解除暂停
    // （避免页面误报 hidden 后再也没人把它恢复回来 → 角色一直"消失"而气泡还在）
    const onVisibility = () => {
      if (document.hidden) {
        setPaused(true);
        return;
      }
      void api.pet
        .status()
        .then((s) => {
          if (!disposed && s.visible) setPaused(false);
        })
        .catch(() => {});
    };
    document.addEventListener('visibilitychange', onVisibility);

    return () => {
      disposed = true;
      unlisten?.();
      document.removeEventListener('visibilitychange', onVisibility);
    };
  }, []);

  /* ------------------- AI 发言 ↔ 核心态 / 气泡 ------------------- */
  useEffect(() => {
    if (say?.streaming) {
      // 发言期间：把核心态抬到 thinking（优先级低于 working/sad，不打断签到）；
      // 同时清掉普通气泡，避免流结束回落到旧台词
      setBubble(null);
      if (coreRef.current !== 'thinking' && coreRef.current !== 'working' && coreRef.current !== 'sad') {
        sayThinkingRef.current = true;
        setCore('thinking');
      }
    } else if (sayThinkingRef.current) {
      // 流结束：只回退本次自己抬起来的那一档
      sayThinkingRef.current = false;
      setCore((cur) => (cur === 'thinking' ? 'idle' : cur));
    }
  }, [say?.streaming]);

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

  /* --------------- 节日 / 时段 / 空闲彩蛋 调度（P6 全量彩蛋） --------------- */
  /**
   * 优先级：**节日 > 时段 > 空闲随机彩蛋**（命中即 return，同一轮只播一个）。
   *
   * 三个触发源共用本定时器与同一套护栏（纯 idle + 无瞬态 + 非拖拽），因此：
   * - 节日当天：待机后第一次轮到即播（内存去重，当天只播一次）；
   * - 时段：早/午/晚各自当天只播一次（避免"一顿饭吃十遍"）；
   * - 彩蛋：每轮都播，靠 `pickFlavorAction` 的**分桶轮转**保证 4 类都能轮到。
   *
   * 均只读事件、只播动作，不写任何业务数据（红线）。
   */
  useEffect(() => {
    const intervalMs = Math.max(1, Math.min(30, easterEggMinutes)) * 60_000;
    const timer = window.setInterval(() => {
      if (coreRef.current !== 'idle') return;
      if (transientRef.current || draggingRef.current) return;
      const now = new Date();
      const today = localDateKey(now);

      // 1) 节日（含农历节日与节气；当天只播一次）
      if (anniversaryEnabled) {
        const plan = getTodaysFestivalPlan(now);
        if (plan && !festivalFiredRef.current.has(plan.key)) {
          const action = plan.actions.find((id) => ACTIONS[id]?.url);
          if (action) {
            festivalFiredRef.current.add(plan.key);
            playTransient('idle', `${plan.label}到了`, action);
            return;
          }
        }
      }

      // 2) 时段（刷牙 / 早餐 / 午餐 / 晚餐；每个时段每天只播一次）
      if (anniversaryEnabled) {
        const slot = timeSlot(now.getHours());
        if (slot) {
          const slotKey = `${today}#${slot}`;
          if (!slotFiredRef.current.has(slotKey)) {
            const action = pickSlotAction(slot);
            if (ACTIONS[action]?.url) {
              slotFiredRef.current.add(slotKey);
              playTransient('idle', undefined, action);
              return;
            }
          }
        }
      }

      // 3) 空闲随机彩蛋（分桶轮转 + 避开最近播过的）
      if (easterEggEnabled) {
        const tick = flavorTickRef.current;
        flavorTickRef.current += 1;
        const action = pickFlavorAction(tick, recentFlavorRef.current);
        if (!ACTIONS[action]?.url) return;
        recentFlavorRef.current = pushRecent(recentFlavorRef.current, action);
        playTransient('idle', pickLine('idle', lastLineRef.current));
      }
    }, intervalMs);
    return () => window.clearInterval(timer);
  }, [anniversaryEnabled, easterEggEnabled, easterEggMinutes, playTransient]);

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
    // 先按当前已知状态开菜单（不阻塞），随后补一次探测校正新鲜度；
    // 之后的实时变化由 Rust 推送的 pet-update{gatewayRunning} 驱动
    setMenu({ x, y, view: 'main' });
    void api.apiServer
      .status()
      .then((s) => setGatewayRunning(!!s?.running))
      .catch(() => {});
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

  /* ------- 菜单扩展（2026-10-20）：动作点播 / 位置与自愈 / 切换形象 ------- */

  /** 动作分组的中文标题（与 manifest 的 group 字段对应；未知 group 回落显示原值） */
  const GROUP_LABELS: Record<string, string> = {
    idle: '待机',
    work: '工作',
    balance: '积分',
    click: '点击回应',
    mumble: '碎碎念',
    drag: '拖拽',
    turn: '转向',
    jump: '跳跃',
    // P6 全量彩蛋新增的 4 组
    festival: '节日',
    life: '饮食起居',
    talent: '才艺玩物',
    sfx: '特效',
  };

  /** 点播一个动作：走 click 池的瞬态通道并覆写动作 id，时长按素材实测值 */
  const pet_playAction = (id: string) => {
    closeMenu();
    playTransient('click', undefined, id);
  };

  /** 随机来一个：在**有素材**的动作里随机挑一个点播（106 个动作时的省事入口） */
  const pet_playRandom = () => {
    closeMenu();
    const pool = ACTION_LIST.filter((a) => a.url);
    if (pool.length === 0) return;
    const a = pool[Math.floor(Math.random() * pool.length)];
    playTransient('click', `来一个：${a.label}`, a.id);
  };

  /** 动作分组展开态（默认收起：106 个动作全展开会长到滚不完） */
  const toggleGroup = (group: string) => {
    setOpenGroups((cur) => {
      const next = new Set(cur);
      if (next.has(group)) next.delete(group);
      else next.add(group);
      return next;
    });
  };

  const openActionView = useCallback(() => {
    setMenu((m) => (m ? { ...m, view: 'actions' } : m));
  }, []);

  const openPackView = useCallback(() => {
    setMenu((m) => (m ? { ...m, view: 'packs' } : m));
    void api.pet
      .packList()
      .then((list) => setPacks(Array.isArray(list) ? list : []))
      .catch(() => setPacks([]));
  }, []);

  /** 切换形象：写回设置，后端 pet-update{pack} 会触发前端重新装载动作表 */
  const switchPack = useCallback(
    (id: string) => {
      closeMenu();
      void api.misc
        .settingsSet({ pet_pack: id })
        .then(() => showBubble(id === 'builtin' ? '已切回内置形象' : '形象已切换'))
        .catch(() => showBubble('切换形象失败'));
    },
    [closeMenu, showBubble],
  );

  const resetPosition = useCallback(() => {
    closeMenu();
    void api.pet
      .resetPosition()
      .then(() => showBubble('回到初始位置啦'))
      .catch(() => showBubble('回到初始位置失败'));
  }, [closeMenu, showBubble]);

  const reloadPet = useCallback(() => {
    closeMenu();
    void api.pet.reload().catch(() => {});
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
  actionIdRef.current = actionId;

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
  // AI 气泡比普通播报气泡再高 10px：两者切换时不会"原地跳"，也不会把角色头顶压住
  const sayBubbleBottom = hitBox ? `calc(${(100 - hitBox.top).toFixed(2)}% + 18px)` : '52%';

  // 菜单落位：贴着角色**右侧**、垂直居中悬浮。
  // 窗口宽度 = 角色舞台 + 右侧 160px 留白（pet.css 的 --pet-menu-space），
  // 所以菜单能完整画在角色右边而不遮挡角色、也不被窗口裁掉。
  const MENU_W = 148;
  /** 菜单最大高度估值（含可选的「跟我聊一句」按钮，留余量） */
  const MENU_H = 268;
  const MENU_GAP = 6;
  const box = hitBox ?? { left: 14, top: 52, width: 72, height: 48 };
  const charRightPx = ((box.left + box.width) / 100) * window.innerWidth;
  const charMidPx = ((box.top + box.height / 2) / 100) * window.innerHeight;
  const menuLeft = Math.max(
    4,
    Math.min(charRightPx + MENU_GAP, window.innerWidth - MENU_W - 4),
  );
  const menuTop = Math.max(
    4,
    Math.min(charMidPx - MENU_H / 2, window.innerHeight - MENU_H - 4),
  );
  const menuStyle: CSSProperties = { left: menuLeft, top: menuTop };

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
        onDuration={handleDuration}
      />
      {/* 气泡锚点：底边贴在角色头顶上方，而不是窗口顶部 */}
      {bubbleEnabled && say ? (
        // AI 发言优先：流式中文本随 delta 增长（打字机），流结束后按长度延时消失
        <div className="pet-bubble-anchor" style={{ bottom: sayBubbleBottom }}>
          <PetBubble
            seq={say.sessionId}
            text={say.text}
            thinking={say.thinking}
            streaming={say.streaming}
            onDone={() => {
              // 记一下结束时刻：紧接着跳回"积分播报"气泡会显得很突兀
              setSayEndAt(Date.now());
              cancelSay();
            }}
          />
        </div>
      ) : bubbleEnabled && bubble && Date.now() - sayEndAt > 1500 ? (
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
            style={menuStyle}
            onPointerDown={(e) => e.stopPropagation()}
          >
            {menu.view === 'actions' ? (
              <>
                <div className="pet-menu-title">
                  <button type="button" onClick={() => setMenu((m) => (m ? { ...m, view: 'main' } : m))}>
                    ‹ 返回
                  </button>
                  <span>动作点播</span>
                </div>
                <div className="pet-menu-scroll">
                  <button type="button" className="pet-menu-random" onClick={pet_playRandom}>
                    随机来一个
                  </button>
                  {Object.entries(
                    ACTION_LIST.reduce<Record<string, typeof ACTION_LIST>>((acc, a) => {
                      (acc[a.group] ??= []).push(a);
                      return acc;
                    }, {}),
                  ).map(([group, list]) => (
                    <div key={group}>
                      <button
                        type="button"
                        className="pet-menu-group"
                        onClick={() => toggleGroup(group)}
                        aria-expanded={openGroups.has(group)}
                      >
                        {GROUP_LABELS[group] ?? group}（{list.length}）{openGroups.has(group) ? '▾' : '▸'}
                      </button>
                      {openGroups.has(group)
                        ? list.map((a) => (
                            <button key={a.id} type="button" onClick={() => pet_playAction(a.id)}>
                              {a.label}
                            </button>
                          ))
                        : null}
                    </div>
                  ))}
                </div>
              </>
            ) : menu.view === 'packs' ? (
              <>
                <div className="pet-menu-title">
                  <button type="button" onClick={() => setMenu((m) => (m ? { ...m, view: 'main' } : m))}>
                    ‹ 返回
                  </button>
                  <span>切换形象</span>
                </div>
                <div className="pet-menu-scroll">
                  <button
                    type="button"
                    onClick={() => switchPack('builtin')}
                    className={currentPack === 'builtin' ? 'active' : undefined}
                  >
                    内置形象
                  </button>
                  {packs.length === 0 ? (
                    <p className="pet-menu-hint">
                      未发现素材包。放到 %APPDATA%\AIWorkAssistant\pets\&lt;id&gt;\ 下即可。
                    </p>
                  ) : (
                    packs.map((p) => (
                      <button
                        key={p.id}
                        type="button"
                        onClick={() => switchPack(p.id)}
                        className={currentPack === p.id ? 'active' : undefined}
                      >
                        {p.name}
                      </button>
                    ))
                  )}
                </div>
              </>
            ) : (
              <>
                <button type="button" onClick={runCheckin}>
                  一键签到
                </button>
                {aiMode !== 'off' ? (
                  <button
                    type="button"
                    onClick={() => {
                      closeMenu();
                      sayNow();
                    }}
                  >
                    跟我聊一句
                  </button>
                ) : null}
                <button type="button" onClick={() => toggleGateway(gatewayRunning)}>
                  {gatewayRunning ? '停止 API 网关' : '启动 API 网关'}
                </button>
                <div className="pet-menu-sep" />
                <button type="button" onClick={focusMainFromMenu}>
                  呼出主窗口
                </button>
                <button type="button" onClick={openActionView}>
                  动作点播 ›
                </button>
                <button type="button" onClick={openPackView}>
                  切换形象 ›
                </button>
                <button type="button" onClick={resetPosition}>
                  回到初始位置
                </button>
                <div className="pet-menu-sep" />
                <button type="button" onClick={reloadPet}>
                  重新加载桌宠
                </button>
                <button type="button" onClick={hideFromMenu}>
                  隐藏桌宠
                </button>
              </>
            )}
          </div>
        </div>
      ) : null}
    </div>
  );
}
