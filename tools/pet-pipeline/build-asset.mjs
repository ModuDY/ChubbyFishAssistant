#!/usr/bin/env node
/**
 * 桌宠素材处理链 · 薄封装（设计 §9 P4-A）
 *
 * 把「绿幕 mp4（路线 A）/ PR 手工抠像 mov（路线 B）」加工成符合本仓库播放规格的
 * 透明 WebM，**校验通过后**才落盘到 src/assets/pet/videos/。
 *
 * 它只做「准备输入 → 依序调上游 4 步 → 调 check-asset 校验 → 落盘」，
 * 真正的转码逻辑全在 vendor/ 的上游脚本里（逐字保留，未改动）。
 *
 * 用法：
 *   node tools/pet-pipeline/build-asset.mjs --src <file> [选项]
 *
 * 选项：
 *   --src <path>        源视频（.mp4 走路线 A；.mov 走路线 B）。必填
 *   --alias <id>        落盘文件名（ASCII，如 idle-breathe）。源名非 ASCII 时必填
 *   --label <中文名>    仅打印用（便于后续登记 manifest 的 label）
 *   --route auto|pr     抠像路线；缺省按扩展名推断（.mov→pr，其余→auto）
 *   --watermark         先跑 step01 去水印（需自备 video/watermark_mask_v5.mkv）
 *   --keep              保留中间产物（video/ pr/ step01..04），便于排查
 *
 * 前置依赖见同目录 README.md（Python + numpy/scipy + 带 libvpx 的便携 ffmpeg）。
 */
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from 'node:fs';
import { basename, dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const PIPELINE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(PIPELINE, '..', '..');
const VENDOR = join(PIPELINE, 'vendor');
const CHECK_ASSET = join(PIPELINE, 'check-asset.mjs');
const FFMPEG = join(PIPELINE, '.tools', 'ffmpeg-9.0.1-essentials_build', 'bin', 'ffmpeg.exe');
const VIDEOS_OUT = resolve(ROOT, 'src', 'assets', 'pet', 'videos');

/** 上游脚本按固定目录工作；ROOT = 脚本上一级 = tools/pet-pipeline/ */
const WORK_DIRS = ['video', 'pr', 'step01', 'step02', 'step03', 'step04'].map((d) => join(PIPELINE, d));

/* ------------------------------- 输出与退出 ------------------------------- */

const log = (msg) => process.stdout.write(`${msg}\n`);
const fail = (msg) => {
  process.stderr.write(`\n[build-asset] ✗ ${msg}\n`);
  process.exit(1);
};

/* --------------------------------- 参数 --------------------------------- */

function parseArgs(argv) {
  const out = { route: null, watermark: false, keep: false };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === '--watermark') out.watermark = true;
    else if (a === '--keep') out.keep = true;
    else if (a.startsWith('--')) out[a.slice(2)] = argv[++i];
    else fail(`无法识别的参数: ${a}`);
  }
  return out;
}

/* ------------------------------- 前置检查 ------------------------------- */

/** 找一个可用的 python（Windows 多为 `python`，部分环境为 `python3`） */
function findPython() {
  for (const exe of ['python', 'python3']) {
    const r = spawnSync(exe, ['--version'], { encoding: 'utf8' });
    if (!r.error && r.status === 0) return exe;
  }
  return null;
}

function preflight(python) {
  const py = spawnSync(python, ['-c', 'import numpy, scipy'], { encoding: 'utf8' });
  if (py.error || py.status !== 0) {
    fail(
      `Python 缺依赖 numpy/scipy（${python} -c "import numpy, scipy" 失败）。\n` +
        `  安装：${python} -m pip install numpy scipy`,
    );
  }
  if (!existsSync(FFMPEG)) {
    fail(
      `缺便携 ffmpeg（上游脚本硬编码此路径，必须用带 libvpx-vp9 的构建）：\n` +
        `  期望：${FFMPEG}\n` +
        `  该目录结构 tools/pet-pipeline/.tools/ffmpeg-9.0.1-essentials_build/bin/ 不能改（见 README）。`,
    );
  }
}

/* -------------------------------- 工作区 -------------------------------- */

function cleanWorkDirs() {
  for (const d of WORK_DIRS) {
    if (existsSync(d)) rmSync(d, { recursive: true, force: true });
  }
}

/** 把一个文件复制成 `<alias><ext>`（上游按文件名派生后续产物名，故统一用 ASCII alias） */
function stageSource(src, ext, alias) {
  const dir = ext.toLowerCase() === '.mov' ? join(PIPELINE, 'pr') : join(PIPELINE, 'video');
  mkdirSync(dir, { recursive: true });
  const dst = join(dir, `${alias}${ext}`);
  copyFileSync(src, dst);
  return dst;
}

function runStep(script, label) {
  log(`  ▸ ${label}`);
  const r = spawnSync(pythonCmd, [join(VENDOR, script)], { cwd: PIPELINE, stdio: 'inherit' });
  if (r.error || r.status !== 0) fail(`${label} 失败（${script}，退出码 ${r.status ?? 'N/A'}）`);
}

let pythonCmd = 'python'; // 由 main 覆盖

/* --------------------------------- 主流程 -------------------------------- */

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (!args.src) fail('缺少 --src（源视频路径）。用法见 tools/pet-pipeline/README.md');

  const src = resolve(args.src);
  if (!existsSync(src) || !statSync(src).isFile()) fail(`源文件不存在或不是文件: ${src}`);

  const ext = extname(src).toLowerCase();
  const route = args.route ?? (ext === '.mov' ? 'pr' : 'auto');
  if (route !== 'auto' && route !== 'pr') fail(`--route 只能是 auto | pr（收到 ${route}）`);

  const stem = basename(src, ext);
  const alias = args.alias ?? (/^[A-Za-z0-9._-]+$/.test(stem) ? stem : null);
  if (!alias) fail(`源名「${stem}」含非 ASCII 字符，请用 --alias 指定落盘文件名（如 idle-breathe）`);
  if (!/^[A-Za-z0-9._-]+$/.test(alias)) fail(`--alias 只能是 ASCII 字母/数字/._-（收到 ${alias}）`);

  log(`[build-asset] 源=${src}`);
  log(`[build-asset] 路线=${route}  落盘名=${alias}${args.label ? `  中文名=${args.label}` : ''}`);

  // 1) 前置检查
  const python = findPython();
  if (!python) fail('未找到 python / python3（素材链依赖 Python 3.9+）。');
  pythonCmd = python;
  preflight(python);
  log(`[build-asset] 前置检查通过（${python} + 便携 ffmpeg）`);

  // 2) 清工作区 + 备料
  cleanWorkDirs();
  stageSource(src, ext, alias);

  // 3) 依序调步
  log('[build-asset] 开始处理链：');
  if (args.watermark) runStep('watermark_step01.py', 'step01 水印填充');
  if (route === 'pr') runStep('pr_import_step02.py', 'step02 路线B（PR 手工抠像导入）');
  else runStep('chroma_step02.py', 'step02 路线A（HSV 自动抠像）');
  runStep('normalize_step03.py', 'step03 归一化 2160×1215');
  runStep('encode_thumbs.py', 'step04 转码 640×360 VP9 alpha');

  // 4) 定位产物并校验
  const produced = join(PIPELINE, 'step04', `${alias}.webm`);
  if (!existsSync(produced)) {
    const listing = existsSync(join(PIPELINE, 'step04'))
      ? readdirSync(join(PIPELINE, 'step04')).join(', ')
      : '(空)';
    fail(`未找到产物 ${produced}；step04 实际内容: ${listing}`);
  }
  log('[build-asset] 校验产物（check-asset.mjs）：');
  const check = spawnSync(process.execPath, [CHECK_ASSET, produced], { stdio: 'inherit' });
  if (check.error || check.status !== 0) {
    fail('产物未通过校验，已中止落盘（不写入 src/assets/pet/videos/）。');
  }

  // 5) 落盘
  mkdirSync(VIDEOS_OUT, { recursive: true });
  const finalPath = join(VIDEOS_OUT, `${alias}.webm`);
  copyFileSync(produced, finalPath);
  log(`[build-asset] ✓ 已落盘: ${finalPath}`);

  // 6) 收尾
  if (!args.keep) cleanWorkDirs();
  else log('[build-asset] 已保留中间产物（--keep）');

  log('');
  log('[build-asset] 后续手工步骤：');
  log(`  1) 在 src/assets/pet/manifest.json 的 actions 增一条：`);
  log(`     { "id": "${alias}", "label": "${args.label ?? alias}", "group": "<idle|work|balance|click|mumble|drag|turn|jump>", "loop": <true|false>, "file": "${alias}.webm" }`);
  log(`  2) 若是新角色/替换上游素材，同步更新 src/assets/pet/THIRD_PARTY_NOTICES.md 与 README 致谢`);
  log(`  3) 跑一次全量校验：node tools/pet-pipeline/check-asset.mjs`);
}

main();
