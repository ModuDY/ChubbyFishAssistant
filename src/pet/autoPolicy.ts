/**
 * 自动发言统一策略（P12）：时间问候与事件自动点评**共用**同一份配额与静音状态。
 *
 * 为什么必须合并：两套节流各自计数时，用户设了「每日 4 次」，实际可能听到 8 次；
 * 而且"今天别主动说话"必须同时管住两条链路，否则按了还会被点评打断。
 *
 * 持久化口径（只存策略，不存内容）：日期、当日次数、今日静音标记、当天已触发的事件键。
 * 重启后不能绕过上限，也不能绕过静音。
 */
import { isQuietHour, type TimeSlot } from './flavor';

const KEY = 'pet-auto-policy-v1';

export type AutoPolicyState = {
  /** 本地日期键 YYYY-MM-DD：跨天时计数、静音与去重集合一起重置 */
  date: string;
  count: number;
  silencedDate: string | null;
  seen: string[];
};

type StorageLike = Pick<Storage, 'getItem' | 'setItem'>;

/** 跨天重置（纯函数，便于单测） */
export function rollover(state: AutoPolicyState, date: string): AutoPolicyState {
  if (state.date === date) return state;
  return { date, count: 0, silencedDate: null, seen: [] };
}

/** 是否还能自动说一句（静音 / 配额 / 深夜三条一起判） */
export function canAutoSpeak(
  state: AutoPolicyState,
  date: string,
  cap: number,
  hour: number,
): boolean {
  if (state.silencedDate === date) return false;
  if (isQuietHour(hour)) return false;
  return state.count < cap;
}

export class PetAutoPolicy {
  private state: AutoPolicyState;
  private readonly storage: StorageLike;
  private readonly now: () => Date;

  constructor(storage: StorageLike | undefined = safeStorage(), now: () => Date = () => new Date()) {
    this.storage = storage ?? { getItem: () => null, setItem: () => {} };
    this.now = now;
    this.state = this.read();
  }

  private read(): AutoPolicyState {
    const date = dateKey(this.now());
    try {
      const raw = this.storage.getItem(KEY);
      if (raw) {
        const parsed = JSON.parse(raw) as Partial<AutoPolicyState>;
        return rollover(
          {
            date: typeof parsed.date === 'string' ? parsed.date : date,
            count: Number.isFinite(parsed.count) ? Number(parsed.count) : 0,
            silencedDate: typeof parsed.silencedDate === 'string' ? parsed.silencedDate : null,
            seen: Array.isArray(parsed.seen) ? parsed.seen.filter((x) => typeof x === 'string') : [],
          },
          date,
        );
      }
    } catch {
      // 配置损坏按全新一天处理：宁可多问一次，也不要因为脏数据永远闭嘴
    }
    return { date, count: 0, silencedDate: null, seen: [] };
  }

  private save() {
    try {
      this.storage.setItem(KEY, JSON.stringify(this.state));
    } catch {
      // localStorage 写不进去（隐私模式/配额满）不影响本次会话的策略
    }
  }

  private roll() {
    this.state = rollover(this.state, dateKey(this.now()));
  }

  /** 今天是否已被「今天别主动说话」压住 */
  silencedToday(): boolean {
    this.roll();
    return this.state.silencedDate === dateKey(this.now());
  }

  /** 一键闭嘴 / 恢复（当天） */
  setSilenced(on: boolean) {
    this.roll();
    this.state.silencedDate = on ? dateKey(this.now()) : null;
    this.save();
  }

  /** 是否还能自动发言（配额由设置页决定，调用方传入） */
  canSpeak(cap: number): boolean {
    this.roll();
    const now = this.now();
    return canAutoSpeak(this.state, dateKey(now), cap, now.getHours());
  }

  /** 记一次自动发言（时间问候与事件点评在真正发起请求前调用） */
  noteSpoken() {
    this.roll();
    this.state.count += 1;
    this.save();
  }

  /** 当天该事件是否已触发过（键 = YYYY-MM-DD#event） */
  hasSeen(key: string): boolean {
    this.roll();
    return this.state.seen.includes(key);
  }

  markSeen(key: string) {
    this.roll();
    if (!this.state.seen.includes(key)) {
      this.state.seen.push(key);
      this.save();
    }
  }

  /** 当前已用次数（调试/测试用） */
  count(): number {
    this.roll();
    return this.state.count;
  }
}

/** 本地日期键（不依赖 UTC，避免时区把"今天"算成昨天） */
export function dateKey(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

function safeStorage(): StorageLike | undefined {
  try {
    return typeof window !== 'undefined' ? window.localStorage : undefined;
  } catch {
    return undefined;
  }
}

/* ------------------------------ 时段问候 ------------------------------ */

/** 每个时段的本地短句（不出现具体钟点，避免像闹钟） */
export const SLOT_GREETINGS: Record<TimeSlot, string[]> = {
  morning: ['早，今天想干点什么', '早呀，先醒醒神', '早上好，慢慢来就行'],
  noon: ['中午了，吃点东西再继续', '午饭别拖太晚', '中午啦，歇一会儿'],
  evening: ['晚上好，今天辛苦啦', '天黑了，收个尾吧', '晚上啦，别熬太晚'],
};

export function pickGreeting(slot: TimeSlot, rand: () => number = Math.random): string {
  const pool = SLOT_GREETINGS[slot];
  return pool[Math.min(pool.length - 1, Math.floor(rand() * pool.length))];
}

/**
 * AI 独白档的提示词。
 *
 * 明确禁止报时：模型极容易输出「现在是上午九点十二分，建议您…」——那是闹钟，不是桌宠。
 * 只给情境（时段、星期），不给精确分钟。
 */
export function buildProactivePrompt(slot: TimeSlot): string {
  const weekday = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'][new Date().getDay()];
  const part = slot === 'morning' ? '上午' : slot === 'noon' ? '中午' : '晚上';
  return (
    `现在是${weekday}${part}，我正对着电脑。用一句话自然地说点什么，像朋友随口一提。` +
    '不要说「现在是」「已经」「提醒您」，不要报出具体时间，不要解释你为什么说话，不要用感叹号堆情绪。'
  );
}

/** 时段问候的去重键：每个时段每天一次，错过不补 */
export function slotEventKey(date: string, slot: TimeSlot): string {
  return `${date}#greet-${slot}`;
}
