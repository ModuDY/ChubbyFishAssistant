import { describe, expect, it } from 'vitest';

import manifest from '../assets/pet/manifest.json';
import {
  coveredActions,
  coverageEntries,
  manualOnlyActions,
  sourcesForAction,
  uncoveredActions,
} from './coverage';
import { FOOD_ACTIONS, PLAY_ACTIONS, SFX_ACTIONS, SHOWCASE_ACTIONS, TALENT_ACTIONS } from './pools';
import { FLAVOR_BUCKETS, SLOT_ACTIONS } from './flavor';
import { ALL_FESTIVAL_ACTIONS, nextFestivalAction } from './festivals';
import { LANDING_ACTIONS, pickLandingAction, STARTLE_SPEED } from './interaction';

type ManifestAction = { id: string; group: string; loop: boolean };
const ids = (manifest.actions as ManifestAction[]).map((a) => a.id);
const byId = new Map((manifest.actions as ManifestAction[]).map((a) => [a.id, a]));

/**
 * 核心回归防线（2026-10-20）：106 个动作入库后，曾有 50 个因为没有接线而永远不会自动播。
 * 这条测试把「吃灰」变成编译期以外的硬失败——新增动作忘了接线会直接红。
 */
describe('动作覆盖（防吃灰）', () => {
  it('manifest 里没有任何动作被漏掉', () => {
    expect(uncoveredActions(ids)).toEqual([]);
  });

  it('手动点播白名单只含 loop 展示类（其余必须有自动入口）', () => {
    expect(manualOnlyActions()).toEqual(['sfx-spin-showcase', 'sfx-outfit-showcase']);
    for (const id of manualOnlyActions()) {
      expect(byId.get(id)?.loop).toBe(true);
    }
  });

  it('自动入口引用的动作 id 全部存在于 manifest（防拼写错）', () => {
    const unknown = [...coveredActions()].filter((id) => !byId.has(id));
    expect(unknown).toEqual([]);
  });

  it('自动入口不收录节日外的「非自动」池子重复登记', () => {
    const sources = coverageEntries().map((e) => e.source);
    expect(new Set(sources).size).toBe(sources.length);
  });
});

describe('分类池卫生', () => {
  it('吃喝 / 玩乐 / 才艺 / 特效池互不重叠', () => {
    const all = [FOOD_ACTIONS, PLAY_ACTIONS, TALENT_ACTIONS, SFX_ACTIONS].flat();
    expect(new Set(all).size).toBe(all.length);
  });

  it('每个分类池内的动作都属于对应的 group', () => {
    for (const id of FOOD_ACTIONS.concat(PLAY_ACTIONS)) expect(byId.get(id)?.group).toBe('life');
    for (const id of TALENT_ACTIONS) expect(byId.get(id)?.group).toBe('talent');
    for (const id of SFX_ACTIONS) expect(byId.get(id)?.group).toBe('sfx');
  });

  it('展示类不进任何随机池 / 瞬态池 / 落地池', () => {
    const autoPools = [
      ...FLAVOR_BUCKETS.flat(),
      ...Object.values(SLOT_ACTIONS).flat(),
      ...LANDING_ACTIONS,
    ];
    for (const id of SHOWCASE_ACTIONS) expect(autoPools).not.toContain(id);
  });
});

describe('节日轮播（原实现只播第一个候选）', () => {
  const spring = [
    'festival-spring-fireworks',
    'festival-spring-redpacket',
    'festival-spring-niangao',
    'festival-spring-fu',
    'festival-spring-lion',
  ];

  it('依次返回未播过的候选', () => {
    expect(nextFestivalAction(spring, [])).toBe(spring[0]);
    expect(nextFestivalAction(spring, [spring[0]])).toBe(spring[1]);
    expect(nextFestivalAction(spring, spring.slice(0, 4))).toBe(spring[4]);
  });

  it('全部播完返回 null（当天收工）', () => {
    expect(nextFestivalAction(spring, spring)).toBeNull();
  });

  it('跳过没有素材的候选', () => {
    const hasAsset = (id: string) => id !== spring[0];
    expect(nextFestivalAction(spring, [], hasAsset)).toBe(spring[1]);
  });

  it('19 个节日动作都能通过某条节日规则被播到', () => {
    const festivalIds = ids.filter((id) => id.startsWith('festival-'));
    for (const id of festivalIds) expect(ALL_FESTIVAL_ACTIONS).toContain(id);
  });
});

describe('拖拽落地分流', () => {
  it('低速时只走常规落地候选', () => {
    const picks = new Set(Array.from({ length: 20 }, (_, i) => pickLandingAction(0, () => i / 20)));
    for (const id of picks) expect(LANDING_ACTIONS).toContain(id);
  });

  it('高速甩动有概率触发 life-scared', () => {
    // 随机源恒 0 < STARTLE_CHANCE → 必定走惊吓分支
    expect(pickLandingAction(STARTLE_SPEED + 1, () => 0)).toBe('life-scared');
    // 随机源恒 1 → 不触发，仍走常规落地
    expect(LANDING_ACTIONS).toContain(pickLandingAction(STARTLE_SPEED + 1, () => 0.999));
  });
});

describe('sourcesForAction —— 排障入口', () => {
  it('能报出一个动作的所有触发路径', () => {
    expect(sourcesForAction('festival-spring-fireworks').length).toBeGreaterThan(0);
    // 吃西瓜既在"喂点吃的"互动池里，也在空闲随机池的"吃的"桶里
    expect(sourcesForAction('life-watermelon')).toEqual(
      expect.arrayContaining(['transient:feed', 'flavor:bucket1']),
    );
    expect(sourcesForAction('work-busy')).toEqual(expect.arrayContaining(['core:working']));
  });

  it('展示类没有任何自动入口', () => {
    for (const id of SHOWCASE_ACTIONS) expect(sourcesForAction(id)).toEqual([]);
  });
});
