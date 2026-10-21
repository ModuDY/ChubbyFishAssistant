/**
 * 桌宠 AI 发言（P3）：订阅 Rust `pet-say-*` 事件，维护流式气泡状态。
 *
 * 为什么要 sessionId 过滤：Rust 侧「新的盖旧的」会 abort 上一轮流，但被 abort 的流
 * 仍可能已发出若干 delta 事件；前端按单调递增的 sessionId 丢弃过期事件（与 abort 双保险）。
 *
 * 失败语义（重要）：
 * - `api.pet.say` 同步 reject（未开启 / 网关未启动 / 节流 / 超上限）→ 立刻回调 `onFallback`，
 *   **不弹 AI 气泡**（避免"先出半个气泡再消失"的闪烁），调用方直接用本地台词；
 * - 流中途 `pet-say-error` → 同样清空占位并回调 `onFallback`。
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { api } from '../lib/tauri';

export type SayState = {
  sessionId: number;
  /** 流式中（气泡不自动消失，等待流结束） */
  streaming: boolean;
  text: string;
  /** 思考链（碎碎念）累计文本 */
  thinking: string;
} | null;

/** 一轮上下文（P7 多轮对话；字段名与 Rust `PetSayTurn` 一致） */
export type SayTurn = { role: 'user' | 'assistant'; content: string };

type StartPayload = { sessionId: number; trigger?: string; model?: string };
type DeltaPayload = { sessionId: number; kind: 'text' | 'reasoning'; delta: string };
type DonePayload = { sessionId: number; ok: boolean; text: string; reasoning: string };
type ErrorPayload = { sessionId: number; code: string; message: string };

type Options = {
  /** 首次收到 reasoning delta（用于点亮 mumble-deep 动作） */
  onReasoningFirst?: () => void;
  /** 无法发言或流失败 → 调用方回落本地台词池 */
  onFallback?: (reason: string) => void;
};

export function usePetSay(opts: Options) {
  const [say, setSay] = useState<SayState>(null);
  /** 已接受的最大 sessionId（单调递增，永不重置）：小于等于它的事件一律丢弃 */
  const activeRef = useRef(0);
  const reasoningSeenRef = useRef(false);
  const optsRef = useRef(opts);
  optsRef.current = opts;

  useEffect(() => {
    let disposed = false;
    const subs: Array<Promise<UnlistenFn>> = [];
    const on = <T>(name: string, fn: (p: T) => void) => {
      subs.push(
        listen<T>(name, (e) => {
          if (!disposed) fn(e.payload);
        }),
      );
    };

    on<StartPayload>('pet-say-start', (p) => {
      if (p.sessionId <= activeRef.current) return;
      activeRef.current = p.sessionId;
      reasoningSeenRef.current = false;
      setSay((cur) => ({
        sessionId: p.sessionId,
        streaming: true,
        text: cur?.text ?? '',
        thinking: cur?.thinking ?? '',
      }));
    });

    on<DeltaPayload>('pet-say-delta', (p) => {
      if (p.sessionId !== activeRef.current) return;
      if (p.kind === 'reasoning') {
        if (!reasoningSeenRef.current) {
          reasoningSeenRef.current = true;
          optsRef.current.onReasoningFirst?.();
        }
        setSay((cur) => (cur ? { ...cur, thinking: cur.thinking + p.delta } : cur));
      } else {
        setSay((cur) => (cur ? { ...cur, text: cur.text + p.delta } : cur));
      }
    });

    on<DonePayload>('pet-say-done', (p) => {
      if (p.sessionId !== activeRef.current) return;
      setSay({
        sessionId: p.sessionId,
        streaming: false,
        text: p.text ?? '',
        thinking: p.reasoning ?? '',
      });
    });

    on<ErrorPayload>('pet-say-error', (p) => {
      if (p.sessionId !== activeRef.current) return;
      setSay(null);
      optsRef.current.onFallback?.(p.code || 'error');
    });

    return () => {
      disposed = true;
      subs.forEach((s) => void s.then((un) => un()));
    };
  }, []);

  /**
   * 发起一次发言。true = 已受理（气泡进入流式态）；false = 无法发言（已回调 onFallback）。
   *
   * P7：`opts` 可带多轮历史与档位（mode）。翻成 Rust 侧参数名 `history` / `mode`。
   */
  const startSay = useCallback(
    async (
      prompt: string,
      context?: string | null,
      trigger: 'user' | 'auto' = 'user',
      opts?: { history?: SayTurn[]; mode?: 'chat' | 'monologue' },
    ) => {
      try {
        const id = await api.pet.say(
          prompt,
          context,
          trigger,
          opts?.history ?? null,
          opts?.mode ?? 'monologue',
        );
        // 若 pet-say-start 事件先于 invoke 返回到达（sessionId 已被接受），
        // 这里不要再覆盖状态，否则会把已收到的 delta 清空
        if (id > activeRef.current) {
          activeRef.current = id;
          reasoningSeenRef.current = false;
          setSay({ sessionId: id, streaming: true, text: '', thinking: '' });
        }
        return true;
      } catch (e) {
        setSay(null);
        optsRef.current.onFallback?.(String(e));
        return false;
      }
    },
    [],
  );

  /** 结束/清空气泡（气泡消失计时到点时调用） */
  const cancelSay = useCallback(() => setSay(null), []);

  return { say, startSay, cancelSay };
}
