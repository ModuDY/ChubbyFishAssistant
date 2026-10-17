#!/usr/bin/env node
/**
 * 启动器新鲜度检查（供 启动助手.cmd 调用）。
 *
 * 判断「已编译的 debug exe 是否落后于当前源码」，避免改了代码却仍拉起旧程序、
 * 误以为新功能没生效。输出单个词（供 cmd 的 for /f 读取）：
 *   missing  exe 不存在，需要先编译
 *   stale    源码/前端产物比 exe 新，需要重新编译
 *   fresh    确认为最新，直接启动
 *
 * 用法：node scripts/launcher_check.mjs <exe 路径>
 */
import { statSync, readdirSync, existsSync } from 'node:fs';
import { join, resolve } from 'node:path';

const exePath = process.argv[2];
if (!exePath) {
  console.error('usage: node scripts/launcher_check.mjs <exe-path>');
  process.exit(2);
}

const root = resolve(process.cwd());

// 参与新鲜度判断的源：Rust 源码 / 前端源码 / 构建配置 / 打包资源
const SOURCE_DIRS = ['src', 'src-tauri/src', 'build-assets', 'scripts'];
const SOURCE_FILES = [
  'index.html',
  'package.json',
  'vite.config.ts',
  'tsconfig.json',
  'tailwind.config.js',
  'postcss.config.js',
  'src-tauri/tauri.conf.json',
  'src-tauri/Cargo.toml',
  'src-tauri/build.rs',
];
const SKIP_DIRS = new Set(['node_modules', 'target', 'dist', '.git', '.codebuddy']);

const mtimeOf = (p) => {
  try {
    return statSync(p).mtimeMs;
  } catch {
    return 0;
  }
};

/** 递归取目录内最新文件时间（跳过产物目录）。 */
function newestIn(dir) {
  let newest = 0;
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return 0;
  }
  for (const entry of entries) {
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue;
      newest = Math.max(newest, newestIn(join(dir, entry.name)));
    } else if (entry.isFile()) {
      newest = Math.max(newest, mtimeOf(join(dir, entry.name)));
    }
  }
  return newest;
}

function main() {
  if (!existsSync(exePath)) {
    console.log('missing');
    return;
  }

  const exeMtime = mtimeOf(exePath);
  let newest = 0;
  for (const dir of SOURCE_DIRS) newest = Math.max(newest, newestIn(join(root, dir)));
  for (const file of SOURCE_FILES) newest = Math.max(newest, mtimeOf(join(root, file)));

  // 前端产物缺失同样视为需要重新编译（exe 内嵌 dist 资源）
  const distIndex = join(root, 'dist', 'index.html');
  if (!existsSync(distIndex)) {
    console.log('stale');
    return;
  }

  console.log(newest > exeMtime ? 'stale' : 'fresh');
}

main();
