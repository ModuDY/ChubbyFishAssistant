/**
 * 网关活动 → 桌宠信号（P5，设计 §6.5）
 *
 * 网关（api_server）侧只在「并发 0→1」时下发一次 `gateway-activity{kind:'active'}`，
 * 其余为上游健康 / 失败边沿（fail / switch / degrade / no-healthy / milestone）。
 *
 * 「一段活跃结束」不在 Rust 侧判定：请求之间有秒级空档，并发会频繁 0→1→0，
 * 按 1→0 直接判定会把一次 IDE 任务拆成几十个「干完」。因此结束改由**前端 5s 静默窗**
 * 判定——窗口内没有新的 active 才算这一轮真的干完。
 *
 * 与 `usePetEvents` 的分工：本 hook 只处理 `gateway-activity`，产出同样的 `PetSignal`，
 * 交给 `PetRoot` 复用既有状态机（不新增状态、不改任何业务逻辑）。
 */
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useEffect, useRef } from 'react';
import { formatAmount } from './lines';
import type { PetSignal } from './usePetEvents';

type GatewayActivity = {
  kind?: 'active' | 'idle' | 'fail' | 'switch' | 'degrade' | 'no-healthy' | 'milestone';
  /** 本次事件涉及的资源池（trae / buddy / qoder / doubao / custom） */
  pool?: string;
  /** 跨池回退的来源池 */
  from?: string;
  /** degrade 原因：circuit_breaker / hard_credit */
  reason?: string;
  model?: string;
  /** milestone：累计 token */
  tokens?: number;
};

/** 一段活跃的静默窗（ms）：窗口内无新 active 才判定「干完了」 */
const IDLE_WINDOW_MS = 5_000;
/** 同一类非核心信号的节流（ms），与 usePetEvents 的 2.5s 口径一致 */
const THROTTLE_MS = 2_500;

const POOL_LABEL: Record<string, string> = {
  trae: 'Trae',
  buddy: 'Buddy',
  qoder: 'Qoder',
  doubao: '豆包',
  custom: '自定义',
};

function poolName(p?: string): string {
  return p ? POOL_LABEL[p] ?? p : '上游';
}

type Options = {
  /**
   * 业务流（签到 / 切换 / 积分）是否正在进行。
   * 返回 true 时，网关一段活跃结束**不**回落核心态、也不叠庆祝动作——
   * 避免把业务流正在展示的 `working` 或失败态 `sad` 打断。
   */
  isBusinessBusy?: () => boolean;
};

/**
 * 订阅网关活动事件。
 * @param onSignal 与业务事件共用的信号回调（同一状态机）
 * @param enabled  设置项 `pet_gateway_link`；关闭时不订阅、零事件处理
 * @param opts     护栏：业务流进行中时避让核心态回落
 */
export function usePetGatewayEvents(
  onSignal: (signal: PetSignal) => void,
  enabled: boolean,
  opts?: Options,
): void {
  const handlerRef = useRef(onSignal);
  handlerRef.current = onSignal;
  const optsRef = useRef(opts);
  optsRef.current = opts;

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    const subs: Array<Promise<UnlistenFn>> = [];
    /** 本 burst 内的 active 次数（≈ 请求波数）；静默窗到点即结算并归零 */
    let burstCount = 0;
    let idleTimer: number | null = null;
    const lastAt = new Map<string, number>();

    const clearIdleTimer = () => {
      if (idleTimer !== null) {
        window.clearTimeout(idleTimer);
        idleTimer = null;
      }
    };

    /** 下发信号；传 throttleKey 时按 2.5s 去重（核心态重置类不带 key，永不被吞） */
    const emit = (signal: PetSignal, throttleKey?: string) => {
      if (disposed) return;
      if (throttleKey) {
        const now = Date.now();
        const prev = lastAt.get(throttleKey) ?? 0;
        if (now - prev < THROTTLE_MS) return;
        lastAt.set(throttleKey, now);
      }
      handlerRef.current(signal);
    };

    subs.push(
      listen<GatewayActivity>('gateway-activity', (event) => {
        if (disposed) return;
        const p = event.payload ?? {};
        switch (p.kind) {
          case 'active': {
            burstCount += 1;
            // 进 working（不节流：每次活跃开始都要把核心态抬起来）
            emit({ type: 'working' });
            // 重置静默窗：窗口内再来 active 就仍是同一轮
            clearIdleTimer();
            idleTimer = window.setTimeout(() => {
              idleTimer = null;
              const n = burstCount;
              burstCount = 0;
              // 护栏（设计 §6.5.6）：业务流进行中（或正处于 sad 停留）时不强降核心态、
              // 不叠庆祝动作——由业务流自己的 done/exit 回落，也避免清掉 sad 的 9s 停留
              if (optsRef.current?.isBusinessBusy?.()) return;
              // 先回落核心态（working → idle），再叠一发瞬态庆祝
              emit({ type: 'idle' });
              emit({ type: 'happy', text: n > 1 ? `一口气接了 ${n} 波活` : '这波干完了' });
            }, IDLE_WINDOW_MS);
            break;
          }
          case 'fail':
            emit({ type: 'sad', text: `${poolName(p.pool)}这次没答上来` }, 'fail');
            break;
          case 'switch':
            emit(
              { type: 'turn', text: `${poolName(p.from)}顶不住，换到 ${poolName(p.pool)}` },
              'switch',
            );
            break;
          case 'degrade':
            emit(
              {
                type: 'sad',
                text:
                  p.reason === 'hard_credit' ? '上游额度见底了' : '上游连连出错，先晾一会儿',
              },
              'degrade',
            );
            break;
          case 'no-healthy':
            emit({ type: 'broke', text: '这会儿没有账号能用了' }, 'no-healthy');
            break;
          case 'milestone':
            emit(
              {
                type: (p.tokens ?? 0) >= 5_000_000 ? 'jump' : 'rich',
                text: `累计吃掉 ${formatAmount(p.tokens ?? 0)} token`,
              },
              'milestone',
            );
            break;
          default:
            break;
        }
      }),
    );

    return () => {
      disposed = true;
      clearIdleTimer();
      for (const sub of subs) void sub.then((un) => un()).catch(() => {});
    };
  }, [enabled]);
}
