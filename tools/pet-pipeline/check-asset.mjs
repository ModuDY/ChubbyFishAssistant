#!/usr/bin/env node
/**
 * 桌宠素材校验器
 *
 * 用法：
 *   node tools/pet-pipeline/check-asset.mjs                 # 校验 src/assets/pet/videos/*.webm
 *   node tools/pet-pipeline/check-asset.mjs <dir|file...>   # 校验指定目录或文件
 *   node tools/pet-pipeline/check-asset.mjs --json          # 机器可读输出（CI 用）
 *   node tools/pet-pipeline/check-asset.mjs --report        # 打印像素测量明细
 *   node tools/pet-pipeline/check-asset.mjs --pixels        # 强制像素级校验（无 ffmpeg 即失败）
 *   node tools/pet-pipeline/check-asset.mjs --no-pixels     # 跳过像素级校验（仅容器级）
 *
 * 校验项（对齐设计方案 §9 P4-A 的产物门槛）：
 *   容器级（零依赖，纯 JS 解析 WebM/EBML 头）
 *     1. 容器 = EBML/WebM，视频轨编码 = VP9（VP8 仅告警）
 *     2. 分辨率恰为 640×360
 *     3. 帧率 ≈ 24fps（DefaultDuration 反算，容差 ±0.5）
 *     4. 视频轨带 AlphaMode=1（VP9/VP8 alpha 轨）
 *     5. 时长 > 0.5s
 *     6. manifest.json 与目录双向对齐（登记了但没文件 / 有文件但没登记）
 *   像素级（需 ffmpeg 解码采样帧，见 PIXEL 常量）
 *     7. 透明像素（alpha < 255）占比 > 5%（§8.3 T1 门槛；上游实测 84.5%）
 *     8. 脚底对齐：**有效脚底行**（该行可见像素 ≥ footRowMinPx，用于剔除金币/音效等细长特效）
 *        与基线 feetY 的偏差在容差内
 *     9. 水平居中：可见像素包围盒**中心均值**与画布中心的偏差在容差内
 *        （用均值而非极值：角色左右晃动是正常的，极值会误伤）
 *
 * 容差校准（2026-10-20，对内置 31 个素材实测）：脚底包络 325~343、中心均值包络 308.0~324.8；
 * 默认容差 20px 即「包络 + 余量」，可放行正常素材，同时拦住「角色飘到半空 / 沉底 / 整体偏心」这类加工事故。
 *
 * 未覆盖（故意不做）：
 *   - 「loop 动作首尾帧可衔接」只按采样帧输出**告警**（首尾帧包围盒/透明度差异），不做硬失败——
 *     有损编码下的逐像素差异没有可靠的绝对阈值，硬判定会误伤。
 *
 * ffmpeg 解析顺序：`PET_PIPELINE_FFMPEG` → `tools/pet-pipeline/.tools/.../bin/ffmpeg.exe` → PATH。
 * 默认行为：找到就跑像素级校验，找不到则**告警跳过**（容器级仍生效）；`--pixels` 时找不到即失败。
 *
 * 退出码：0 = 全部通过；1 = 有不合格项（可直接用于 CI / pre-commit）。
 */
import { spawnSync } from 'node:child_process';
import {
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const PIPELINE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(PIPELINE, '..', '..');
const DEFAULT_DIR = resolve(ROOT, 'src/assets/pet/videos');
const MANIFEST = resolve(ROOT, 'src/assets/pet/manifest.json');
const EXPECT = { width: 640, height: 360, fps: 24, fpsTolerance: 0.5, minDurationSec: 0.5 };

/** 像素级校验参数（阈值来自设计方案 §8.3 T1 实测与 manifest canvas 规格） */
const PIXEL = {
  /** 判定「可见角色像素」的 alpha 阈值：背景实测 alpha 1-50，取 128 可剔除背景噪点 */
  visibleAlpha: 128,
  /** 透明像素（alpha < 255）占比下限（§8.3 T1 门槛） */
  minTransparentRatio: 0.05,
  /** 脚底基线在播放画布上的 y（manifest canvas.feetY，距底 pad=30） */
  defaultFeetY: 330,
  /** 水平中心基线 = 画布中心 */
  defaultCenterX: EXPECT.width / 2,
  /**
   * 脚底允许偏差（px）。校准依据：内置 31 个素材实测有效脚底行包络 **325~343**
   * （基线 330，含金币/音效特效动作的极端值），取 20 留余量。
   * 仍能拦住真正的加工事故（角色飘到半空 / 沉到画面底部，偏差数十~上百 px）。
   */
  feetTolerance: 20,
  /**
   * 水平中心允许偏差（px）。校准依据：内置 31 个素材实测逐帧中心均值包络 **308.0~324.8**
   * （角色剪影天然不对称，如尾巴偏一侧），取 20 留余量。
   */
  centerTolerance: 20,
  /** 采样帧率（每帧 raw 为 640×360×4 ≈ 900KB，1fps 已足够覆盖脚底/中心漂移） */
  sampleFps: 1,
  /** 单文件最多解码帧数，防超长素材撑爆临时文件 */
  maxFrames: 30,
  /**
   * 「有效脚底行」最少可见像素数：某一行可见像素达到此数量才算角色的**身体**。
   * 为什么不用包围盒最低点：`balance-*` 类动作有金币/音效特效向下延伸（实测最低点低到 y=357），
   * 用最低点会把正常素材判成「踩空」。按行像素量过滤即可剔除这些细长特效。
   */
  footRowMinPx: 16,
};

/* ------------------------------- EBML 读取 ------------------------------- */

/** 元素 ID（保留长度标记位，EBML 规范如此） */
function readId(buf, pos) {
  const first = buf[pos];
  if (!first) return null;
  let len = 1;
  for (let i = 0; i < 4; i += 1) {
    if (first & (0x80 >> i)) {
      len = i + 1;
      break;
    }
  }
  if (pos + len > buf.length) return null;
  let value = 0;
  for (let i = 0; i < len; i += 1) value = value * 256 + buf[pos + i];
  return { id: value, len };
}

/** 元素 size（vint，去掉长度标记位）；-1 = 未知长度 */
function readSize(buf, pos) {
  const first = buf[pos];
  if (!first) return null;
  let len = 1;
  for (let i = 0; i < 8; i += 1) {
    if (first & (0x80 >> i)) {
      len = i + 1;
      break;
    }
  }
  if (pos + len > buf.length) return null;
  let value = first & (0xff >> len);
  let allOnes = value === 0xff >> len;
  for (let i = 1; i < len; i += 1) {
    value = value * 256 + buf[pos + i];
    if (buf[pos + i] !== 0xff) allOnes = false;
  }
  return { size: allOnes ? -1 : value, len };
}

function readUint(buf, start, size) {
  let v = 0;
  for (let i = 0; i < size; i += 1) v = v * 256 + buf[start + i];
  return v;
}

function readFloat(buf, start, size) {
  const dv = new DataView(buf.buffer, buf.byteOffset + start, size);
  if (size === 4) return dv.getFloat32(0);
  if (size === 8) return dv.getFloat64(0);
  return NaN;
}

const ID = {
  SEGMENT: 0x18538067,
  INFO: 0x1549a966,
  TRACKS: 0x1654ae6b,
  TRACK_ENTRY: 0xae,
  VIDEO: 0xe0,
  TIMECODE_SCALE: 0x2ad7b1,
  DURATION: 0x4489,
  TRACK_TYPE: 0x83,
  CODEC_ID: 0x86,
  DEFAULT_DURATION: 0x23e383,
  PIXEL_WIDTH: 0xb0,
  PIXEL_HEIGHT: 0xba,
  ALPHA_MODE: 0x53c0,
  CLUSTER: 0x1f43b675,
};

/** 递归遍历：只下钻到我们关心的容器，Cluster 一律跳过（可能几 MB） */
function walk(buf, start, end, out) {
  let pos = start;
  while (pos < end) {
    const id = readId(buf, pos);
    if (!id) return;
    const size = readSize(buf, pos + id.len);
    if (!size) return;
    const contentStart = pos + id.len + size.len;
    const contentEnd = size.size < 0 ? end : Math.min(end, contentStart + size.size);

    if (id.id === ID.CLUSTER) {
      if (size.size < 0) return;
      pos = contentEnd;
      continue;
    }

    const isContainer =
      id.id === ID.SEGMENT ||
      id.id === ID.INFO ||
      id.id === ID.TRACKS ||
      id.id === ID.TRACK_ENTRY ||
      id.id === ID.VIDEO;

    if (isContainer) {
      walk(buf, contentStart, contentEnd, out);
    } else {
      out.push({ id: id.id, start: contentStart, size: size.size });
    }

    if (size.size < 0) return;
    pos = contentEnd;
  }
}

/** 解析一个 WebM：返回 { codec, width, height, fps, alpha, durationSec } */
function probe(file) {
  const buf = readFileSync(file);
  const elements = [];
  walk(buf, 0, buf.length, elements);

  const take = (id) => elements.filter((e) => e.id === id);
  const timecodeScale = take(ID.TIMECODE_SCALE).length
    ? readUint(buf, take(ID.TIMECODE_SCALE)[0].start, take(ID.TIMECODE_SCALE)[0].size)
    : 1_000_000; // 默认 1ms
  const durationRaw = take(ID.DURATION)[0];
  const durationSec = durationRaw
    ? (readFloat(buf, durationRaw.start, durationRaw.size) * timecodeScale) / 1e9
    : NaN;

  // 取第一条视频轨（TrackType=1）
  const videoTrackType = elements.filter((e) => e.id === ID.TRACK_TYPE);
  const codecIds = take(ID.CODEC_ID).map((e) =>
    buf.toString('utf8', e.start, e.start + e.size).replace(/\0+$/, ''),
  );
  const isVideo = videoTrackType.length > 0 && readUint(buf, videoTrackType[0].start, videoTrackType[0].size) === 1;

  const widthEl = take(ID.PIXEL_WIDTH)[0];
  const heightEl = take(ID.PIXEL_HEIGHT)[0];
  const defaultDurationEl = take(ID.DEFAULT_DURATION)[0];
  const alphaEl = take(ID.ALPHA_MODE)[0];

  const fps = defaultDurationEl
    ? 1e9 / readUint(buf, defaultDurationEl.start, defaultDurationEl.size)
    : NaN;

  return {
    isVideo,
    codec: codecIds[0] ?? 'unknown',
    width: widthEl ? readUint(buf, widthEl.start, widthEl.size) : NaN,
    height: heightEl ? readUint(buf, heightEl.start, heightEl.size) : NaN,
    fps,
    alpha: alphaEl ? readUint(buf, alphaEl.start, alphaEl.size) === 1 : false,
    durationSec,
  };
}

/* ------------------------------ 像素级解析 ------------------------------ */

/** 找 ffmpeg：环境变量 → 仓库内便携版（build-asset 同款路径）→ PATH */
function findFfmpeg() {
  const candidates = [
    process.env.PET_PIPELINE_FFMPEG,
    join(PIPELINE, '.tools', 'ffmpeg-9.0.1-essentials_build', 'bin', 'ffmpeg.exe'),
    join(PIPELINE, '.tools', 'ffmpeg-9.0.1-essentials_build', 'bin', 'ffmpeg'),
  ].filter(Boolean);
  for (const c of candidates) {
    if (existsSync(c)) return c;
  }
  const probeRun = spawnSync('ffmpeg', ['-version'], { encoding: 'utf8' });
  if (!probeRun.error && probeRun.status === 0) return 'ffmpeg';
  return null;
}

/**
 * 解码采样帧并统计像素指标。
 *
 * 为什么落临时文件而不是读 stdout：rawvideo 单帧 900KB，spawnSync 的 maxBuffer 会截断。
 *
 * @returns {{ frames:number, transparentMin:number, zeroAvg:number,
 *             feetMin:number, feetMax:number, centerMin:number, centerMax:number,
 *             firstBottom:number, lastBottom:number, firstAlpha:number, lastAlpha:number }}
 */
function analyzePixels(file, ffmpeg) {
  const dir = mkdtempSync(join(tmpdir(), 'pet-check-'));
  const out = join(dir, 'frames.rgba');

  /** 跑一次解码；extra 为 -i 之前的解码器选择 */
  const decode = (extra) => {
    const r = spawnSync(
      ffmpeg,
      [
        '-hide_banner',
        '-loglevel',
        'error',
        ...extra,
        '-i',
        file,
        '-vf',
        `fps=${PIXEL.sampleFps}`,
        '-frames:v',
        String(PIXEL.maxFrames),
        '-f',
        'rawvideo',
        '-pix_fmt',
        'rgba',
        out,
        '-y',
      ],
      { encoding: 'utf8' },
    );
    return r;
  };

  try {
    // 必须显式用 libvpx-vp9 解码：ffmpeg 原生 vp9 解码器**不还原** WebM 的 alpha 通道，
    // 实测原生解码器下全帧 alpha=255（透明占比 0%），会误判「alpha 已丢失」。
    let r = decode(['-c:v', 'libvpx-vp9']);
    let alphaReliable = true;
    if (r.error || r.status !== 0) {
      // 兜底：ffmpeg 构建缺 libvpx 时退回默认解码器（alpha 不可靠，只做尺寸/几何类统计）
      const fb = decode([]);
      if (fb.error || fb.status !== 0) {
        throw new Error((fb.stderr || r.stderr || '').trim() || `ffmpeg 退出码 ${fb.status ?? 'N/A'}`);
      }
      r = fb;
      alphaReliable = false;
    }
    if (r.error) throw new Error(r.error.message);

    const buf = readFileSync(out);
    const frameBytes = EXPECT.width * EXPECT.height * 4;
    const frames = Math.floor(buf.length / frameBytes);
    if (frames === 0) throw new Error('解码不到任何帧');

    const totalPx = EXPECT.width * EXPECT.height;
    const per = [];
    for (let f = 0; f < frames; f += 1) {
      const base = f * frameBytes;
      let transparent = 0;
      let zero = 0;
      let alphaSum = 0;
      let left = Infinity;
      let right = -1;
      let top = Infinity;
      let bottom = -1;
      const rowCounts = new Int32Array(EXPECT.height);

      for (let y = 0; y < EXPECT.height; y += 1) {
        const row = base + y * EXPECT.width * 4;
        for (let x = 0; x < EXPECT.width; x += 1) {
          const a = buf[row + x * 4 + 3];
          alphaSum += a;
          if (a < 255) transparent += 1;
          if (a === 0) zero += 1;
          if (a >= PIXEL.visibleAlpha) {
            rowCounts[y] += 1;
            if (x < left) left = x;
            if (x > right) right = x;
            if (y < top) top = y;
            if (y > bottom) bottom = y;
          }
        }
      }

      // 有效脚底行：最后一行可见像素数达标的 y（剔除金币/音效类细长特效）
      let footRow = -1;
      for (let y = EXPECT.height - 1; y >= 0; y -= 1) {
        if (rowCounts[y] >= PIXEL.footRowMinPx) {
          footRow = y;
          break;
        }
      }

      per.push({
        transparentRatio: transparent / totalPx,
        zeroRatio: zero / totalPx,
        meanAlpha: alphaSum / totalPx,
        visible: right >= 0 ? { left, right, top, bottom } : null,
        footRow,
      });
    }

    const visibleFrames = per.filter((p) => p.visible);
    if (visibleFrames.length === 0) {
      throw new Error(`所有采样帧都没有 alpha≥${PIXEL.visibleAlpha} 的可见像素`);
    }
    const footRows = per.map((p) => p.footRow).filter((y) => y >= 0);
    if (footRows.length === 0) {
      throw new Error(`没有一行可见像素达到 ${PIXEL.footRowMinPx}px，找不到角色身体`);
    }

    const centers = visibleFrames.map((p) => (p.visible.left + p.visible.right) / 2);
    const first = per[0];
    const last = per[per.length - 1];

    return {
      frames,
      alphaReliable,
      transparentMin: Math.min(...per.map((p) => p.transparentRatio)),
      zeroAvg: per.reduce((a, p) => a + p.zeroRatio, 0) / per.length,
      feetMin: Math.min(...footRows),
      feetMax: Math.max(...footRows),
      /** 包围盒最低点（含特效），仅报告用；判定用 feetMax（有效脚底行） */
      bboxBottomMax: Math.max(...visibleFrames.map((p) => p.visible.bottom)),
      centerMin: Math.min(...centers),
      centerMax: Math.max(...centers),
      /** 逐帧中心的均值：角色会左右晃动，用均值才能区分「天生偏心」与「正常摆动」 */
      centerMean: centers.reduce((a, c) => a + c, 0) / centers.length,
      // 首尾帧对比（loop 衔接告警用，取有效脚底行与平均 alpha）
      firstBottom: first.footRow,
      lastBottom: last.footRow,
      firstAlpha: first.meanAlpha,
      lastAlpha: last.meanAlpha,
    };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/* -------------------------------- 校验逻辑 -------------------------------- */

function loadManifest() {
  try {
    return JSON.parse(readFileSync(MANIFEST, 'utf8'));
  } catch {
    return null;
  }
}

function checkOne(file, ctx) {
  const problems = [];
  const warnings = [];
  let info;
  try {
    info = probe(file);
  } catch (e) {
    return { file, ok: false, problems: [`parse error: ${e.message}`], warnings, info: null, pixels: null };
  }

  if (!info.isVideo) problems.push('no video track');
  if (!/^V_VP9$/.test(info.codec)) {
    if (/^V_VP8$/.test(info.codec)) warnings.push(`codec ${info.codec} (VP9 preferred)`);
    else problems.push(`codec ${info.codec} (expect V_VP9)`);
  }
  if (info.width !== EXPECT.width || info.height !== EXPECT.height) {
    problems.push(`size ${info.width}x${info.height} (expect ${EXPECT.width}x${EXPECT.height})`);
  }
  if (!Number.isFinite(info.fps)) warnings.push('fps unknown (no DefaultDuration)');
  else if (Math.abs(info.fps - EXPECT.fps) > EXPECT.fpsTolerance) {
    problems.push(`fps ${info.fps.toFixed(2)} (expect ${EXPECT.fps})`);
  }
  if (!info.alpha) problems.push('no AlphaMode=1 (lost alpha channel)');
  if (!Number.isFinite(info.durationSec)) warnings.push('duration unknown');
  else if (info.durationSec < EXPECT.minDurationSec) {
    problems.push(`duration ${info.durationSec.toFixed(2)}s (< ${EXPECT.minDurationSec}s)`);
  }

  // ---- 像素级校验（可选，需 ffmpeg） ----
  let pixels = null;
  if (ctx.pixels) {
    if (!ctx.ffmpeg) {
      const msg = '像素级校验跳过：未找到 ffmpeg';
      if (ctx.requirePixels) problems.push(msg);
      else warnings.push(msg);
      return { file, ok: problems.length === 0, problems, warnings, info, pixels: null };
    }
    try {
      pixels = analyzePixels(file, ctx.ffmpeg);
    } catch (e) {
      return {
        file,
        ok: false,
        problems: [...problems, `pixel decode failed: ${e.message}`],
        warnings,
        info,
        pixels: null,
      };
    }

    if (!pixels.alphaReliable) {
      warnings.push('alpha 通道统计不可靠（本次解码未走 libvpx-vp9），已跳过透明占比判定');
    } else if (pixels.transparentMin < PIXEL.minTransparentRatio) {
      problems.push(
        `透明像素占比 ${(pixels.transparentMin * 100).toFixed(1)}% (< ${PIXEL.minTransparentRatio * 100}%)——alpha 可能已丢失`,
      );
    }
    // 脚底：素材「最低点」应落到基线（跳跃类动作最低点仍在基线，故用 max 判）
    const dFeet = Math.abs(pixels.feetMax - ctx.feetY);
    if (dFeet > ctx.feetTolerance) {
      problems.push(
        `脚底最低点 y=${pixels.feetMax} 偏离基线 ${ctx.feetY} 超过 ${ctx.feetTolerance}px（实测范围 ${pixels.feetMin}~${pixels.feetMax}）`,
      );
    } else if (Math.abs(pixels.feetMin - ctx.feetY) > ctx.feetTolerance) {
      warnings.push(`脚底在部分帧离地（y=${pixels.feetMin}~${pixels.feetMax}，基线 ${ctx.feetY}）`);
    }
    // 水平居中：用逐帧中心的**均值**判定（角色会左右晃动，极值会误伤正常摆动）
    if (Math.abs(pixels.centerMean - ctx.centerX) > ctx.centerTolerance) {
      problems.push(
        `水平中心均值 ${pixels.centerMean.toFixed(1)} 偏离画布中心 ${ctx.centerX} 超过 ${ctx.centerTolerance}px（逐帧范围 ${pixels.centerMin.toFixed(1)}~${pixels.centerMax.toFixed(1)}）`,
      );
    }
    // loop 衔接：只对**登记为 loop** 的动作告警（有损编码无可靠绝对阈值，故不硬失败）
    const looping = ctx.loops?.get(basename(file));
    if (pixels.frames >= 2 && looping === true) {
      const dBottom = Math.abs(pixels.lastBottom - pixels.firstBottom);
      const dAlpha = Math.abs(pixels.lastAlpha - pixels.firstAlpha);
      if (dBottom > 12 || dAlpha > 12) {
        warnings.push(
          `首尾帧差异偏大（脚底 Δ${dBottom.toFixed(0)}px / 平均 alpha Δ${dAlpha.toFixed(1)}），loop 动作可能不衔接`,
        );
      }
    }
  }

  return { file, ok: problems.length === 0, problems, warnings, info, pixels };
}

function listWebm(targets) {
  const files = [];
  for (const t of targets) {
    const p = resolve(t);
    const st = statSync(p);
    if (st.isDirectory()) {
      for (const name of readdirSync(p)) {
        if (extname(name).toLowerCase() === '.webm') files.push(join(p, name));
      }
    } else {
      files.push(p);
    }
  }
  return files.sort();
}

/** manifest ↔ 目录双向对齐（只有校验默认目录时才有意义） */
function checkManifest(files, manifest) {
  const problems = [];
  const warnings = [];
  if (!manifest) return { problems: ['manifest 读取/解析失败'], warnings };
  const registered = new Set((manifest.actions ?? []).map((a) => a.file));
  const onDisk = new Set(files.map((f) => basename(f)));

  for (const f of registered) {
    if (!onDisk.has(f)) problems.push(`manifest 登记但缺文件: ${f}`);
  }
  for (const f of onDisk) {
    if (!registered.has(f)) warnings.push(`有文件但未登记 manifest: ${f}`);
  }
  return { problems, warnings };
}

/* --------------------------------- 入口 --------------------------------- */

const args = process.argv.slice(2);
const asJson = args.includes('--json');
const report = args.includes('--report');
const noPixels = args.includes('--no-pixels');
const forcePixels = args.includes('--pixels') || report;
const numArg = (name, fallback) => {
  const i = args.indexOf(name);
  if (i < 0) return fallback;
  const v = Number(args[i + 1]);
  return Number.isFinite(v) ? v : fallback;
};

const manifest = loadManifest();
const ctx = {
  ffmpeg: noPixels ? null : findFfmpeg(),
  pixels: !noPixels,
  requirePixels: forcePixels,
  feetY: numArg('--feet-y', manifest?.canvas?.feetY ?? PIXEL.defaultFeetY),
  centerX: numArg('--center-x', (manifest?.canvas?.width ?? EXPECT.width) / 2),
  feetTolerance: numArg('--feet-tolerance', PIXEL.feetTolerance),
  centerTolerance: numArg('--center-tolerance', PIXEL.centerTolerance),
  /** 文件名 → 是否 loop（loop 衔接告警只对这些动作生效） */
  loops: new Map((manifest?.actions ?? []).map((a) => [a.file, a.loop === true])),
};

const targets = args.filter((a) => !a.startsWith('--') && !/^\d+(\.\d+)?$/.test(a));
const files = listWebm(targets.length ? targets : [DEFAULT_DIR]);

if (files.length === 0) {
  console.error('no .webm found');
  process.exit(1);
}

const results = files.map((f) => checkOne(f, ctx));
const isDefaultDir = targets.length === 0;
const manifestCheck = isDefaultDir ? checkManifest(files, manifest) : { problems: [], warnings: [] };

const failed = results.filter((r) => !r.ok).length + manifestCheck.problems.length;

if (asJson) {
  console.log(
    JSON.stringify(
      {
        expected: EXPECT,
        pixel: { feetY: ctx.feetY, centerX: ctx.centerX, feetTolerance: ctx.feetTolerance },
        pixelsEnabled: ctx.pixels && Boolean(ctx.ffmpeg),
        files: results.map((r) => ({
          file: basename(r.file),
          ok: r.ok,
          problems: r.problems,
          warnings: r.warnings,
          info: r.info,
          pixels: r.pixels,
        })),
        manifest: manifestCheck,
        failed,
      },
      null,
      2,
    ),
  );
} else {
  for (const r of results) {
    const tag = r.ok ? 'PASS' : 'FAIL';
    const detail = r.info
      ? `${r.info.codec} ${r.info.width}x${r.info.height} ${Number.isFinite(r.info.fps) ? r.info.fps.toFixed(2) : '?'}fps alpha=${r.info.alpha} ${Number.isFinite(r.info.durationSec) ? r.info.durationSec.toFixed(2) : '?'}s`
      : '';
    console.log(`[${tag}] ${basename(r.file).padEnd(22)} ${detail}`);
    for (const p of r.problems) console.log(`       ! ${p}`);
    for (const w of r.warnings) console.log(`       ~ ${w}`);
    if (report && r.pixels) {
      const px = r.pixels;
      console.log(
        `       · 透明 ${(px.transparentMin * 100).toFixed(1)}% (alpha=0 ${(px.zeroAvg * 100).toFixed(1)}%)` +
          ` 脚底 ${px.feetMin}~${px.feetMax} 中心均值 ${px.centerMean.toFixed(1)} (${px.centerMin.toFixed(1)}~${px.centerMax.toFixed(1)})` +
          ` 首尾 Δ脚底 ${Math.abs(px.lastBottom - px.firstBottom).toFixed(0)}px Δalpha ${Math.abs(px.lastAlpha - px.firstAlpha).toFixed(1)} (${px.frames}帧)`,
      );
      if (Math.abs(px.bboxBottomMax - px.feetMax) > 10) {
        console.log(`       · 包围盒最低点 ${px.bboxBottomMax}（含特效，已按有效脚底行过滤）`);
      }
    }
  }
  for (const p of manifestCheck.problems) console.log(`[FAIL] manifest: ${p}`);
  for (const w of manifestCheck.warnings) console.log(`[WARN] manifest: ${w}`);
  console.log(
    `\n${files.length - results.filter((r) => !r.ok).length}/${files.length} passed, ${failed} failed`,
  );
  if (!ctx.pixels) console.log('note: 已按 --no-pixels 跳过像素级校验（透明占比 / 脚底 / 居中）');
  else if (!ctx.ffmpeg) console.log('note: 未找到 ffmpeg，像素级校验已跳过；装好便携 ffmpeg 后自动启用');
}
process.exit(failed === 0 ? 0 : 1);
