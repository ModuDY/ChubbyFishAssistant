/**
 * 右键菜单的展示模型（纯函数，便于单测）
 *
 * 两件事从 `PetRoot` 里搬出来：
 * 1. **文案规范化**：manifest 的 `label` 自带分组前缀（`余额-钱袋满溢`），
 *    菜单里已经有分组标签，再显示一遍就成了「积分 › 余额-钱袋满溢」。
 *    这里只在展示层剥离，不改 `manifest.json`（保留上游可追溯性）。
 * 2. **点播面板的筛选模型**：分组聚合、搜索、最近使用。
 */
import type { PetAction } from './actions';

/** 动作分组的中文标题（与 manifest 的 group 字段对应；未知 group 回落显示原值） */
export const GROUP_LABELS: Record<string, string> = {
  idle: '待机',
  work: '工作',
  balance: '积分',
  click: '点击回应',
  mumble: '碎碎念',
  drag: '拖拽',
  turn: '转向',
  jump: '跳跃',
  festival: '节日',
  life: '饮食起居',
  talent: '才艺玩物',
  sfx: '特效',
};

/** 分类 chips 的展示顺序：按使用频度排，业务组在前、彩蛋组在后 */
export const GROUP_ORDER = [
  'idle',
  'work',
  'balance',
  'click',
  'mumble',
  'life',
  'talent',
  'festival',
  'sfx',
  'drag',
  'turn',
  'jump',
] as const;

/** manifest label 里重复的分组前缀（只剥这四类，其余文案保持原样） */
const LABEL_PREFIX = /^(余额|工作状态|点击回应|碎碎念)[-－]\s*/;

/** 展示用短标签：去掉分组前缀 */
export function shortLabel(label: string): string {
  const stripped = label.replace(LABEL_PREFIX, '').trim();
  return stripped || label;
}

export type ActionGroup = {
  key: string;
  label: string;
  items: PetAction[];
};

/** 按分组聚合（只收有素材的，避免点播到空动作；未知分组排在已知分组之后） */
export function groupActions(list: readonly PetAction[]): ActionGroup[] {
  const map = new Map<string, PetAction[]>();
  for (const action of list) {
    if (!action.url) continue;
    const bucket = map.get(action.group);
    if (bucket) bucket.push(action);
    else map.set(action.group, [action]);
  }
  const order = [...GROUP_ORDER];
  const keys = [...map.keys()].sort((a, b) => {
    const ia = order.indexOf(a as (typeof GROUP_ORDER)[number]);
    const ib = order.indexOf(b as (typeof GROUP_ORDER)[number]);
    if (ia === -1 && ib === -1) return a.localeCompare(b);
    if (ia === -1) return 1;
    if (ib === -1) return -1;
    return ia - ib;
  });
  return keys.map((key) => ({
    key,
    label: GROUP_LABELS[key] ?? key,
    items: map.get(key) ?? [],
  }));
}

/** 搜索：中文子串匹配展示名与原始名（输入「余额」仍能命中，不牺牲可追溯性） */
export function filterActions(list: readonly PetAction[], query: string): PetAction[] {
  const q = query.trim().toLowerCase();
  if (!q) return [...list];
  return list.filter(
    (a) => shortLabel(a.label).toLowerCase().includes(q) || a.label.toLowerCase().includes(q),
  );
}

/** 最近使用：按记录顺序取出仍存在且有素材的动作 */
export function recentActions(list: readonly PetAction[], ids: readonly string[]): PetAction[] {
  const byId = new Map(list.map((a) => [a.id, a]));
  const out: PetAction[] = [];
  for (const id of ids) {
    const action = byId.get(id);
    if (action?.url) out.push(action);
  }
  return out;
}

/** 记录一次点播：去重后置顶，只保留最近 max 条（内存态，不落盘） */
export function pushRecentId(ids: readonly string[], id: string, max = 6): string[] {
  return [id, ...ids.filter((x) => x !== id)].slice(0, max);
}
