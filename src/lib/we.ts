/**
 * 本机 Wallpaper Engine 壁纸库：扫描结果 + 给 webwallgl 的资源源。
 *
 * 为什么不用 `httpSource`（引擎自带的那个）：
 * 它要求把工程目录挂成一个 HTTP 服务（`{base}/scene.pkg`、`{base}/project.json`…）。
 * 我们已经有 Tauri IPC，可以直接按名读文件 —— 少一个监听端口、少一层 CORS、
 * 也不用把用户硬盘上的路径暴露成 URL。所以自己实现 `Source` 接口。
 */

import { invoke } from '@tauri-apps/api/core';
import type { Source } from 'webwallgl';

/**
 * 引擎的 `Source.sceneDir()` 返回形态。
 * 库的公开类型入口没有导出这个别名，这里按 `types.d.ts` 的定义本地声明一份
 * （结构完全一致，只是避免依赖未导出的名字）。
 */
interface SceneDirAssets {
  readonly entry: string;
  read(name: string, signal?: AbortSignal): Promise<Uint8Array | null>;
  bytes?(): number;
}

export interface WeWallpaper {
  id: string;
  title: string;
  /** scene / video / web / application */
  kind: string;
  /** 工程目录绝对路径 */
  dir: string;
  entry: string | null;
  preview: string | null;
  workshop: boolean;
}

export interface WeScanResult {
  installDir: string | null;
  libraryRoots: string[];
  items: WeWallpaper[];
}

/** 扫描本机壁纸库（结果由调用方缓存，不要每次渲染都调） */
export function scanWe(): Promise<WeScanResult> {
  return invoke<WeScanResult>('we_scan');
}

/** 读工程内单个文件；不存在时抛错 */
async function readFile(dir: string, name: string): Promise<Uint8Array> {
  // we_read 返回 tauri::ipc::Response ⇒ JS 侧收到 ArrayBuffer
  const buf = await invoke<ArrayBuffer>('we_read', { dir, name });
  return new Uint8Array(buf);
}

/**
 * 给 webwallgl 的 Source：优先按"散装工程"读（本机官方工程与多数工坊场景都是散装的），
 * 拿不到入口 json 时才回退 scene.pkg。
 *
 * 注意 `scenePkg()` 抛错在引擎里等价于"该场景不可用"，所以这里只在真的没有 pkg 时
 * 才抛，并且抛之前先让 sceneDir 有机会返回非 null。
 */
export function weSource(dir: string, projectFile: string | null): Source {
  const cache = new Map<string, Uint8Array>();

  const read = async (name: string): Promise<Uint8Array | null> => {
    const hit = cache.get(name);
    if (hit) return hit;
    try {
      const bytes = await readFile(dir, name);
      // 贴图动辄数 MB，缓存住避免装配期重复 IPC
      if (bytes.byteLength < 24 * 1024 * 1024) cache.set(name, bytes);
      return bytes;
    } catch {
      return null;
    }
  };

  const entry = projectFile ?? 'scene.json';

  return {
    async project() {
      const bytes = await read('project.json');
      if (!bytes) return null;
      try {
        return JSON.parse(new TextDecoder().decode(bytes));
      } catch {
        return null;
      }
    },
    async sceneDir(): Promise<SceneDirAssets | null> {
      // 只有入口是 json 才是散装工程；mp4 / html 走别的分支
      if (!entry.toLowerCase().endsWith('.json')) return null;
      const head = await read(entry);
      if (!head) return null;
      return {
        entry,
        read: (name: string) => read(name),
      };
    },
    async scenePkg() {
      const bytes = await read('scene.pkg');
      if (!bytes) throw new Error('该壁纸没有 scene.pkg，也没有可用的散装入口');
      return bytes;
    },
  };
}

/** 预览图 → object URL（同一张只转一次；调用方负责在合适时机释放） */
const previewUrls = new Map<string, string>();

export async function wePreviewUrl(w: WeWallpaper): Promise<string | null> {
  if (!w.preview) return null;
  const key = `${w.dir}::${w.preview}`;
  const hit = previewUrls.get(key);
  if (hit) return hit;
  try {
    const bytes = await readFile(w.dir, w.preview);
    const mime = w.preview.toLowerCase().endsWith('.png')
      ? 'image/png'
      : w.preview.toLowerCase().endsWith('.gif')
        ? 'image/gif'
        : 'image/jpeg';
    const url = URL.createObjectURL(new Blob([bytes.slice().buffer as ArrayBuffer], { type: mime }));
    previewUrls.set(key, url);
    return url;
  } catch {
    return null;
  }
}

/** 视频类壁纸：一次性取整段 mp4 生成 object URL（本机文件，读盘即可） */
export async function weVideoUrl(w: WeWallpaper): Promise<string | null> {
  if (!w.entry) return null;
  const key = `${w.dir}::${w.entry}::video`;
  const hit = previewUrls.get(key);
  if (hit) return hit;
  try {
    const bytes = await readFile(w.dir, w.entry);
    const url = URL.createObjectURL(new Blob([bytes.slice().buffer as ArrayBuffer], { type: 'video/mp4' }));
    previewUrls.set(key, url);
    return url;
  } catch {
    return null;
  }
}
