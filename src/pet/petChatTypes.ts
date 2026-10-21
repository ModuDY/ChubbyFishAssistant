/**
 * 桌宠内联对话（P7，2026-10-21）
 *
 * 为什么内联：用户明确选择「在桌宠窗口里直接打字」，沉浸感优先于"不抢前台焦点"的保守设计。
 * 但这带来两个**必须在本层解决**的硬问题，否则功能一上线就是坏的：
 *
 * 1. **点击穿透会吃掉输入框**。Rust 侧按 sprite 首帧 alpha 掩码 40Hz 切换 `set_ignore_cursor_events`，
 *    输入框不属于 sprite → 光标移上去的瞬间窗口变穿透，输入框点不动、也无法二次聚焦。
 *    解法：`overlayRectOnMask` 把输入框矩形**并进**掩码后再上报（只增不减，与既有并集策略一致）。
 *
 * 2. **IME 组合期回车不是"发送"**。中文输入法选词时按回车是确认候选词，
 *    若直接当发送，用户打字打到一半就被发出去半截拼音。故 `isComposing` 必须短路。
 *
 * 另外这里放「用户的话 → 动作」的**确定性意图路由**：用户主动说了"唱一个"，就真的播唱歌动作。
 * 刻意**不**让模型输出动作指令——`pet.rs` 的注释记录了教训：一旦给模型加输出格式规则，
 * 它会先规划怎么满足规则，思考链膨胀、正文被截断（2026-10-19 修过一次）。
 * 反向解析用户输入既零 token、零 prompt 污染，又能复用 `pools.ts` 的动作池。
 */
/**
 * 用户意图 → 动作类别。
 *
 * 取值刻意与 `PetTransientKind` 的对应项**同名**（feed/play/perform/pet/tease），
 * 这样调用方可以直接 `playTransient(intent.kind)`，不必再写一层映射表。
 */
export type ChatIntentKind = 'feed' | 'play' | 'perform' | 'pet' | 'tease';

/** 一轮对话消息（与 Rust `PetSayTurn` 字段一一对应） */
export type ChatTurn = {
  role: 'user' | 'assistant';
  content: string;
};

/**
 * 送给模型的历史轮数上限。
 *
 * 为什么不是无限：每轮都进 prompt，成本随对话线性增长；桌宠聊天的价值在"记得刚才说了什么"，
 * 而不是"记得三小时前"。12 条 ≈ 6 轮问答，足够连贯，token 可控。
 */
export const MAX_HISTORY_TURNS = 12;
/** 单条历史的最大字符数（防止一次粘贴把上下文撑爆） */
export const MAX_TURN_CHARS = 600;
/** 用户输入的最大字符数（输入框 maxLength 与提交前裁剪共用） */
export const MAX_INPUT_CHARS = 200;

/** 追加一条历史并裁剪（返回新数组，便于 React 状态更新） */
export function appendTurn(history: ChatTurn[], turn: ChatTurn, max = MAX_HISTORY_TURNS): ChatTurn[] {
  const clipped: ChatTurn = {
    role: turn.role,
    content: turn.content.slice(0, MAX_TURN_CHARS),
  };
  const next = [...history, clipped];
  return next.length > max ? next.slice(next.length - max) : next;
}

/** 仅裁剪（历史从外部恢复 / 兜底用） */
export function trimHistory(history: ChatTurn[], max = MAX_HISTORY_TURNS): ChatTurn[] {
  return history.length > max ? history.slice(history.length - max) : history;
}

/**
 * 输入回车是否应当「发送消息」。
 *
 * @param key        按键名
 * @param shift      是否按着 Shift（Shift+Enter 换行/不发送）
 * @param composing  IME 是否正在组合（中文选词阶段）
 */
export function shouldSendOnEnter(key: string, shift: boolean, composing: boolean): boolean {
  if (key !== 'Enter') return false;
  if (shift) return false;
  // 组合期回车用于确认候选词，绝不能当发送
  return !composing;
}

/* ------------------------- 用户的话 → 动作意图 ------------------------- */

/**
 * 关键词表：命中即播对应类别的动作。
 *
 * 用词都挑「日常对话里不会歧义」的，刻意避开单字：
 * - 不收裸「跳」（"心跳""跳槽"）/ 裸「摸」（"摸索"）/ 裸「吃」（"吃了吗"其实也算，保留）
 * - 收「跳舞 / 跳个」这类带上下文的写法
 *
 * 顺序即优先级：一句话同时命中多类时取靠前的（吃的 > 玩的 > 才艺 > 撸 > 生气）。
 */
const INTENT_WORDS: ReadonlyArray<{ kind: ChatIntentKind; words: readonly string[] }> = [
  {
    kind: 'feed',
    words: ['吃', '饿', '喂', '零食', '点心', '奶茶', '西瓜', '火锅', '宵夜', '糖葫芦', '冰淇淋', '长寿面', '蟹'],
  },
  {
    kind: 'play',
    // "盘棋 / 局棋" 覆盖「下盘棋 / 来局棋」这类量词插入的说法（"下棋"匹配不到它们）
    words: ['玩', '陪', '游戏', '下棋', '盘棋', '局棋', '魔方', '秋千', '竹马', '陀螺', '气球', '水枪', '小车', '踢'],
  },
  {
    kind: 'perform',
    words: ['唱', '跳舞', '跳个', '才艺', '表演', '魔术', '变个', '小提琴', '笛', '口琴', '杂技', '露一手', '写书法', '打鼓'],
  },
  {
    kind: 'pet',
    words: ['摸摸', '撸', '顺毛', '抱抱', '亲一个'],
  },
  {
    kind: 'tease',
    words: ['讨厌', '好烦', '笨蛋', '丑', '坏蛋', '住手'],
  },
];

export type ChatIntent = {
  kind: ChatIntentKind;
  /** 命中的关键词（用于气泡/调试，知道"为什么播这个"） */
  matched: string;
};

/**
 * 解析用户输入里的动作意图；无命中返回 null（纯聊天，只播说话动作）。
 *
 * @param text 用户原话
 */
export function detectIntent(text: string): ChatIntent | null {
  const t = text.trim();
  if (!t) return null;
  // 类别按表顺序优先（吃的 > 玩的 > 才艺 > 撸 > 生气）；
  // 同一类别内取**最长**命中词——"我想吃火锅"应报 matched:"火锅"（有信息量），
  // 而不是先撞上的单字"吃"（那样排障时看不出到底命中了什么）。
  for (const row of INTENT_WORDS) {
    let best: string | null = null;
    for (const w of row.words) {
      if (t.includes(w) && (best === null || w.length > best.length)) best = w;
    }
    if (best) return { kind: row.kind, matched: best };
  }
  return null;
}

/* --------------------------- 掩码合成（点击穿透） --------------------------- */

/**
 * 把一个矩形（CSS px，相对窗口左上角）并入点击穿透掩码。
 *
 * 为什么要单独成函数：这条逻辑错了的表现是"输入框时好时坏、点不进去"，
 * 极难在真机上复现；做成纯函数后可以直接断言格子范围。
 *
 * 向外各扩 1 格（宁大勿小）：与 `PetSprite` 的掩码策略一致——多挡一格只是
 * 边缘多拦一点鼠标，少挡一格会让输入框边角点不到。
 *
 * @param base  原掩码（sprite alpha 采样结果，长度 cols*rows）
 * @param rect  并入的矩形（CSS px）
 * @param winW  窗口 CSS 宽度
 * @param winH  窗口 CSS 高度
 */
export function overlayRectOnMask(
  base: Uint8Array,
  rect: { left: number; top: number; width: number; height: number },
  winW: number,
  winH: number,
  cols: number,
  rows: number,
): Uint8Array {
  const out = new Uint8Array(base);
  const w = Math.max(1, winW);
  const h = Math.max(1, winH);
  if (rect.width <= 0 || rect.height <= 0) return out;

  const clampCol = (v: number) => Math.min(cols - 1, Math.max(0, v));
  const clampRow = (v: number) => Math.min(rows - 1, Math.max(0, v));
  const x0 = clampCol(Math.floor((rect.left / w) * cols) - 1);
  const x1 = clampCol(Math.ceil(((rect.left + rect.width) / w) * cols));
  const y0 = clampRow(Math.floor((rect.top / h) * rows) - 1);
  const y1 = clampRow(Math.ceil(((rect.top + rect.height) / h) * rows));

  for (let y = y0; y <= y1; y += 1) {
    for (let x = x0; x <= x1; x += 1) {
      out[y * cols + x] = 1;
    }
  }
  return out;
}
