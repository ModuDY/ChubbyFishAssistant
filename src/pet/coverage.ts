/**
 * 动作覆盖审计（防"吃灰"）
 *
 * 背景：106 个动作入库后，实际被自动播到的只有 30 来个。2026-10-20 排查出两类问题：
 * 1. **接线漏了**：`PetRoot` 的空闲彩蛋计算出动作却忘了传给 `playTransient`，
 *    导致分桶池里 50 个动作一次都没播过；
 * 2. **候选只取第一个**：节日类 `actions.find()` 让 10 个节日动作永远排在队尾。
 *
 * 光靠人工核对挡不住回归（池子还会继续加动作）。这里把「所有自动触发入口」摊开成
 * 一张表，`coverage.test.ts` 拿它与 `manifest.json` 做差集：
 * **任何一个 manifest 里的动作，都必须能在某条自动入口里出现**，
 * 否则测试失败并列出"吃灰"名单。这样下次加动作忘了接线会立刻被 CI 拦住。
 *
 * 只看"自动入口"——右键菜单的手动点播（`pet_playAction` / `pet_playRandom`）能播任意动作，
 * 若无限制地把它们算作"覆盖"，这张表就永远绿灯、失去意义。
 */
import { CORE_POOLS, SHOWCASE_ACTIONS, TRANSIENT_POOLS } from './pools';
import { FLAVOR_BUCKETS, QUIET_ACTIONS, SLOT_ACTIONS, TIME_SLOT_ALL } from './flavor';
import { ALL_FESTIVAL_ACTIONS } from './festivals';
import { INTERACTIONS, LANDING_ACTIONS } from './interaction';

/** 入口描述（审计报告用：某个动作是"靠哪条路径被播到"的） */
export type CoverageEntry = {
  /** 入口名（报告里显示） */
  source: string;
  /** 该入口可能播出的动作 id */
  actions: readonly string[];
};

/**
 * 全部**自动**触发入口。
 *
 * 新增触发路径时请在这里登记一条（否则新动作会以"吃灰"身份被单测拦下，
 * 这是刻意的：宁可测试失败，也不要悄悄上线一个永远看不到的动作）。
 */
export function coverageEntries(): CoverageEntry[] {
  const entries: CoverageEntry[] = [];

  // 核心态循环（idle / sleepy / thinking / working / sad）
  for (const [state, pool] of Object.entries(CORE_POOLS)) {
    entries.push({ source: `core:${state}`, actions: pool });
  }

  // 瞬态池（业务事件 + 用户互动）
  for (const [kind, pool] of Object.entries(TRANSIENT_POOLS)) {
    entries.push({ source: `transient:${kind}`, actions: pool });
  }

  // 空闲随机彩蛋（分桶轮转）
  FLAVOR_BUCKETS.forEach((bucket, i) => {
    entries.push({ source: `flavor:bucket${i}`, actions: bucket });
  });

  // 深夜安静池（23~5 点只从这里取）
  entries.push({ source: 'flavor:quiet', actions: QUIET_ACTIONS });

  // 时段（早 / 午 / 晚）
  for (const slot of TIME_SLOT_ALL) {
    entries.push({ source: `slot:${slot}`, actions: SLOT_ACTIONS[slot] });
  }

  // 节日（当天轮播全部候选）
  entries.push({ source: 'festival:all', actions: ALL_FESTIVAL_ACTIONS });

  // 拖拽落地
  entries.push({ source: 'drag:landing', actions: LANDING_ACTIONS });
  entries.push({ source: 'drag:dangle', actions: ['drag-dangle'] });

  // 启动彩蛋 / 打盹过渡（写死在 PetRoot 的两个显式点播，也必须登记）
  entries.push({ source: 'boot:stretch', actions: ['idle-stretch'] });
  entries.push({ source: 'sleep:yawn', actions: ['idle-yawn'] });
  entries.push({ source: 'sleep:wake', actions: ['idle-wake'] });

  return entries;
}

/**
 * 只接受**手动点播**的动作白名单（它们没有自动入口，属于设计意图而非遗漏）。
 *
 * 目前只有两个 `loop:true` 展示类：循环动作进瞬态会被兜底时长硬切，
 * 所以刻意不进任何随机池（见 `flavor.ts` 头部说明）。
 */
export function manualOnlyActions(): string[] {
  return [...SHOWCASE_ACTIONS];
}

/** 自动入口覆盖到的全部动作（去重） */
export function coveredActions(): Set<string> {
  const set = new Set<string>();
  for (const e of coverageEntries()) for (const id of e.actions) set.add(id);
  return set;
}

/**
 * 计算"吃灰"动作：manifest 里有、但既没有自动入口也不在白名单里的动作。
 *
 * @param manifestIds manifest 里的全部动作 id（由调用方传入，避免本模块依赖 actions.ts）
 */
export function uncoveredActions(manifestIds: readonly string[]): string[] {
  const covered = coveredActions();
  const allowedManual = new Set(manualOnlyActions());
  return manifestIds.filter((id) => !covered.has(id) && !allowedManual.has(id));
}

/**
 * 动作 → 可能播到它的入口名列表（排障用：想知道"这个动作什么时候会出现"）。
 * 未被覆盖时返回空数组。
 */
export function sourcesForAction(id: string): string[] {
  return coverageEntries()
    .filter((e) => e.actions.includes(id))
    .map((e) => e.source);
}

/** 主动互动入口（右键菜单三项）——同样必须指向真实存在的池子 */
export function interactionKinds(): string[] {
  return INTERACTIONS.map((i) => i.kind);
}
