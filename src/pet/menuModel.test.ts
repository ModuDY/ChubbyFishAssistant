import { describe, expect, it } from 'vitest';

import type { PetAction } from './actions';
import {
  filterActions,
  groupActions,
  GROUP_LABELS,
  pushRecentId,
  recentActions,
  shortLabel,
} from './menuModel';

function act(id: string, label: string, group: PetAction['group'], url = 'x'): PetAction {
  return { id, label, group, loop: false, url };
}

const LIST: PetAction[] = [
  act('balance-full', '余额-钱袋满溢', 'balance'),
  act('work-busy', '工作状态-忙碌点按', 'work'),
  act('click-wave', '点击回应-元气挥手', 'click'),
  act('mumble-deep', '深度思考碎碎念', 'mumble'),
  act('life-watermelon', '吃西瓜', 'life'),
  act('life-breakfast', '吃早餐', 'life'),
  act('talent-violin', '小提琴演奏', 'talent'),
  act('idle-breathe', '待机呼吸休闲', 'idle'),
  act('missing', '缺素材的动作', 'life', ''),
];

describe('shortLabel —— 剥离重复的分组前缀', () => {
  it('去掉「余额-」「工作状态-」「点击回应-」前缀', () => {
    expect(shortLabel('余额-钱袋满溢')).toBe('钱袋满溢');
    expect(shortLabel('工作状态-忙碌点按')).toBe('忙碌点按');
    expect(shortLabel('点击回应-元气挥手')).toBe('元气挥手');
  });

  it('没有前缀的文案保持原样', () => {
    expect(shortLabel('吃西瓜')).toBe('吃西瓜');
    // 「深度思考碎碎念」不是前缀，不能被误剥
    expect(shortLabel('深度思考碎碎念')).toBe('深度思考碎碎念');
  });
});

describe('groupActions —— 分组聚合与排序', () => {
  it('按使用频度排序，且过滤无素材的动作', () => {
    const groups = groupActions(LIST);
    expect(groups.map((g) => g.key)).toEqual(['idle', 'work', 'balance', 'click', 'mumble', 'life', 'talent']);
    expect(groups.find((g) => g.key === 'life')?.items).toHaveLength(2);
  });

  it('未知分组排在已知分组之后，并用原值做标题', () => {
    const groups = groupActions([...LIST, act('x', '彩蛋', 'custom' as PetAction['group'])]);
    expect(groups[groups.length - 1].key).toBe('custom');
    expect(groups[groups.length - 1].label).toBe('custom');
    expect(GROUP_LABELS.life).toBe('饮食起居');
  });
});

describe('filterActions —— 中文子串搜索', () => {
  it('输入「吃」命中所有吃的动作', () => {
    const hit = filterActions(LIST, '吃').map((a) => a.id);
    expect(hit).toEqual(['life-watermelon', 'life-breakfast']);
  });

  it('空查询原样返回', () => {
    expect(filterActions(LIST, '   ')).toHaveLength(LIST.length);
  });

  it('输入「余额」仍能按原始名命中', () => {
    expect(filterActions(LIST, '余额').map((a) => a.id)).toEqual(['balance-full']);
  });
});

describe('最近使用', () => {
  it('去重置顶并截断到 6 条', () => {
    let ids: string[] = [];
    for (const id of ['a', 'b', 'c', 'd', 'e', 'f', 'g']) ids = pushRecentId(ids, id);
    expect(ids).toEqual(['g', 'f', 'e', 'd', 'c', 'b']);
    ids = pushRecentId(ids, 'd');
    expect(ids[0]).toBe('d');
    expect(ids).toHaveLength(6);
  });

  it('只返回仍存在且有素材的动作', () => {
    const rec = recentActions(LIST, ['missing', 'life-watermelon', 'nope']);
    expect(rec.map((a) => a.id)).toEqual(['life-watermelon']);
  });
});
