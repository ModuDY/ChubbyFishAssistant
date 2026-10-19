/**
 * 动作素材注册表
 *
 * 为什么需要这一层：Vite **只打包被 import 到的资源**。
 * 实测：只显式 import 4 个 webm 时，产物里就只有 4 个（其余 27 个被忽略）。
 * 所以必须用 `import.meta.glob` 批量引入，否则会出现"素材放进去了但打包后 404"。
 *
 * 新增素材的正确姿势：丢文件进 `videos/` + 在 `manifest.json` 加一条，**不用改任何代码**。
 */
import { convertFileSrc } from '@tauri-apps/api/core';
import { api } from '../lib/tauri';
import manifest from '../assets/pet/manifest.json';

const files = import.meta.glob<string>('../assets/pet/videos/*.webm', {
  eager: true,
  query: '?url',
  import: 'default',
});

/** 文件名 → 打包后 URL（Vite 会给文件名加 hash） */
const byFile = new Map<string, string>();
for (const [path, url] of Object.entries(files)) {
  const name = path.split('/').pop();
  if (name) byFile.set(name, url);
}

export type PetAction = {
  id: string;
  /** 上游原始中文名，用于追溯与致谢 */
  label: string;
  group: 'idle' | 'work' | 'balance' | 'click' | 'mumble' | 'drag' | 'turn' | 'jump';
  loop: boolean;
  url: string;
};

type ManifestAction = {
  id: string;
  label: string;
  group: PetAction['group'];
  loop: boolean;
  file: string;
};

const manifestActions = manifest.actions as ManifestAction[];

export const ACTION_LIST: PetAction[] = manifestActions.map((a) => ({
  id: a.id,
  label: a.label,
  group: a.group,
  loop: a.loop,
  url: byFile.get(a.file) ?? '',
}));

export const ACTIONS: Record<string, PetAction> = Object.fromEntries(
  ACTION_LIST.map((a) => [a.id, a]),
);

/** manifest 已登记但打包产物里找不到文件的动作——开发期自检用，生产应为空 */
export const MISSING_ACTIONS: string[] = ACTION_LIST.filter((a) => !a.url).map((a) => a.id);

export function actionUrl(id: string): string {
  return ACTIONS[id]?.url ?? '';
}

/** 内置动作表快照：切素材包时以它为基线，包里没有的动作自动回落内置 */
const BUILTIN_LIST = ACTION_LIST.slice();

/** 原地改写 ACTIONS，保持对象引用不变（petState / PetSprite 都直接读它） */
function replaceActions(list: PetAction[]) {
  for (const key of Object.keys(ACTIONS)) delete ACTIONS[key];
  for (const a of list) ACTIONS[a.id] = a;
}

/**
 * 应用素材包（P4-3）：按动作 id 覆盖内置，包里缺的动作继续用内置素材。
 *
 * 为什么不在打包期解决：Vite 只打包被 import 的资源，用户目录里的 webm
 * 只能用 Tauri 的 asset 协议在运行时读取（convertFileSrc）。
 *
 * @returns 实际生效的包 id（加载失败会回落 'builtin'）
 */
export async function applyPetPack(packId: string | null | undefined): Promise<string> {
  const id = packId && packId !== 'builtin' ? packId : '';
  if (!id) {
    replaceActions(BUILTIN_LIST);
    return 'builtin';
  }
  try {
    const manifest = await api.pet.packManifest(id);
    const merged = new Map(BUILTIN_LIST.map((a) => [a.id, a]));
    for (const a of manifest.actions) {
      merged.set(a.id, {
        id: a.id,
        label: a.label,
        group: a.group as PetAction['group'],
        loop: a.loop,
        url: convertFileSrc(a.path),
      });
    }
    replaceActions([...merged.values()]);
    return id;
  } catch {
    replaceActions(BUILTIN_LIST);
    return 'builtin';
  }
}

/** 画布规格（与素材保持一致，改素材前先看这里） */
export const CANVAS = manifest.canvas;
