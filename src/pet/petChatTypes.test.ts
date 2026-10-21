import { describe, expect, it } from 'vitest';

import manifest from '../assets/pet/manifest.json';
import {
  MAX_HISTORY_TURNS,
  MAX_TURN_CHARS,
  appendTurn,
  detectIntent,
  overlayRectOnMask,
  shouldSendOnEnter,
  trimHistory,
} from './petChatTypes';
import { TRANSIENT_POOLS } from './pools';
import { MASK_COLS, MASK_ROWS } from './PetSprite';

const byId = new Map((manifest.actions as Array<{ id: string }>).map((a) => [a.id, a]));

describe('shouldSendOnEnter —— 回车语义', () => {
  it('普通回车发送', () => {
    expect(shouldSendOnEnter('Enter', false, false)).toBe(true);
  });

  it('IME 组合期回车**不**发送（中文选词会确认候选词）', () => {
    expect(shouldSendOnEnter('Enter', false, true)).toBe(false);
  });

  it('Shift+Enter 不发送', () => {
    expect(shouldSendOnEnter('Enter', true, false)).toBe(false);
  });

  it('其他按键不发送', () => {
    for (const k of ['a', 'Escape', 'Tab', 'NumpadEnter'.toLowerCase()]) {
      expect(shouldSendOnEnter(k, false, false)).toBe(false);
    }
  });
});

describe('appendTurn / trimHistory —— 多轮上下文裁剪', () => {
  it('追加后保留最近 N 条（丢最早）', () => {
    let h = [] as Array<{ role: 'user' | 'assistant'; content: string }>;
    for (let i = 0; i < MAX_HISTORY_TURNS + 5; i += 1) {
      h = appendTurn(h, { role: 'user', content: `第${i}条` });
    }
    expect(h).toHaveLength(MAX_HISTORY_TURNS);
    expect(h[h.length - 1].content).toBe(`第${MAX_HISTORY_TURNS + 4}条`);
    // 最早那几条应该已经被挤掉
    expect(h.some((t) => t.content === '第0条')).toBe(false);
  });

  it('单条超长被裁到上限', () => {
    const h = appendTurn([], { role: 'user', content: '字'.repeat(MAX_TURN_CHARS + 100) });
    expect(h[0].content).toHaveLength(MAX_TURN_CHARS);
  });

  it('trimHistory 不改动未超长的历史', () => {
    const h = [{ role: 'user' as const, content: 'a' }];
    expect(trimHistory(h)).toEqual(h);
  });
});

describe('detectIntent —— 用户的话 → 动作（确定性联动）', () => {
  it('吃喝类命中 feed', () => {
    for (const s of ['我饿了', '喂你点零食', '想吃西瓜', '来碗长寿面', '整只蟹吧']) {
      expect(detectIntent(s)?.kind, s).toBe('feed');
    }
  });

  it('玩乐类命中 play', () => {
    for (const s of ['陪我玩会儿', '下盘棋', '拧个魔方', '推我荡秋千']) {
      expect(detectIntent(s)?.kind, s).toBe('play');
    }
  });

  it('才艺类命中 perform', () => {
    for (const s of ['唱一个', '跳个舞', '变个魔术', '露一手', '拉段小提琴']) {
      expect(detectIntent(s)?.kind, s).toBe('perform');
    }
  });

  it('撸宠 / 生气各自命中', () => {
    expect(detectIntent('让我摸摸')?.kind).toBe('pet');
    expect(detectIntent('你这个笨蛋')?.kind).toBe('tease');
  });

  it('纯聊天不命中任何意图（只说话，不播额外动作）', () => {
    for (const s of ['今天几号', '你好', '帮我看看日志', '', '   ']) {
      expect(detectIntent(s)).toBeNull();
    }
  });

  it('命中时回报 matched 关键词（排障用）', () => {
    expect(detectIntent('我想吃火锅')?.matched).toBe('火锅');
  });

  it('每类意图都指向真实存在的动作池', () => {
    for (const kind of ['feed', 'play', 'perform', 'pet', 'tease'] as const) {
      const pool = TRANSIENT_POOLS[kind];
      expect(pool.length, kind).toBeGreaterThan(0);
      for (const id of pool) expect(byId.has(id), `${kind} → ${id}`).toBe(true);
    }
  });
});

describe('overlayRectOnMask —— 输入条并入点击穿透掩码', () => {
  const empty = () => new Uint8Array(MASK_COLS * MASK_ROWS);
  const idx = (x: number, y: number) => y * MASK_COLS + x;

  it('矩形覆盖到的格子被置 1，其余保持 0', () => {
    // 窗口 460×360，矩形放在正中偏上
    const out = overlayRectOnMask(empty(), { left: 180, top: 100, width: 100, height: 30 }, 460, 360, MASK_COLS, MASK_ROWS);
    // 中心点必须为 1
    const cx = Math.floor((230 / 460) * MASK_COLS);
    const cy = Math.floor((115 / 360) * MASK_ROWS);
    expect(out[idx(cx, cy)]).toBe(1);
    // 左上角（远离矩形）必须仍为 0
    expect(out[idx(0, 0)]).toBe(0);
  });

  it('向外各扩 1 格（宁大勿小，避免边角点不到）', () => {
    const out = overlayRectOnMask(empty(), { left: 230, top: 180, width: 10, height: 10 }, 460, 360, MASK_COLS, MASK_ROWS);
    const cx = Math.floor((235 / 460) * MASK_COLS);
    const cy = Math.floor((185 / 360) * MASK_ROWS);
    // 紧邻中心格的左右邻居也应为 1
    expect(out[idx(Math.max(0, cx - 1), cy)]).toBe(1);
    expect(out[idx(Math.min(MASK_COLS - 1, cx + 1), cy)]).toBe(1);
  });

  it('保持原掩码（并集语义，不清除 sprite 已有的 1）', () => {
    const base = empty();
    base[idx(3, 3)] = 1;
    const out = overlayRectOnMask(base, { left: 0, top: 0, width: 10, height: 10 }, 460, 360, MASK_COLS, MASK_ROWS);
    expect(out[idx(3, 3)]).toBe(1);
  });

  it('退化输入（0 宽 / 0 高 / 0 尺寸窗口）不崩且不越界', () => {
    const base = empty();
    expect(overlayRectOnMask(base, { left: 0, top: 0, width: 0, height: 0 }, 460, 360, MASK_COLS, MASK_ROWS)).toEqual(base);
    // 窗口尺寸为 0 时用 1 兜底，不应抛异常或写出界
    const out = overlayRectOnMask(base, { left: 0, top: 0, width: 100, height: 100 }, 0, 0, MASK_COLS, MASK_ROWS);
    expect(out).toHaveLength(MASK_COLS * MASK_ROWS);
  });

  it('矩形超出窗口边界时被夹紧在网格内', () => {
    const out = overlayRectOnMask(empty(), { left: -100, top: -100, width: 9999, height: 9999 }, 460, 360, MASK_COLS, MASK_ROWS);
    expect(out).toHaveLength(MASK_COLS * MASK_ROWS);
    for (const v of out) expect(v).toBe(1);
  });
});
