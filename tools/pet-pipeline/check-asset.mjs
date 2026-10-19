#!/usr/bin/env node
/**
 * 桌宠素材校验器（零依赖，纯 JS 解析 WebM/EBML 头）
 *
 * 用法：
 *   node tools/pet-pipeline/check-asset.mjs                 # 校验 src/assets/pet/videos/*.webm
 *   node tools/pet-pipeline/check-asset.mjs <dir|file...>   # 校验指定目录或文件
 *   node tools/pet-pipeline/check-asset.mjs --json          # 机器可读输出（CI 用）
 *
 * 校验项（对齐设计方案 §9 P4-A 的产物门槛）：
 *   1. 容器 = EBML/WebM，视频轨编码 = VP9（VP8 仅告警）
 *   2. 分辨率恰为 640×360
 *   3. 帧率 ≈ 24fps（DefaultDuration 反算，容差 ±0.5）
 *   4. 视频轨带 AlphaMode=1（VP9/VP8 alpha 轨）
 *   5. 时长 > 0.5s
 *   6. manifest.json 与目录双向对齐（登记了但没文件 / 有文件但没登记）
 *
 * 未覆盖（需要解码，故意不在这里做）：
 *   - 「透明像素占比 > 5%」和「脚底对齐」要逐帧解码，纯 JS 做不到；
 *     这两项在移植 4 步管线时由 build-asset.mjs 调 ffmpeg 做（见设计方案 §9 P4-A）。
 *
 * 退出码：0 = 全部通过；1 = 有不合格项（可直接用于 CI / pre-commit）。
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, extname, join, resolve, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const DEFAULT_DIR = resolve(ROOT, 'src/assets/pet/videos');
const EXPECT = { width: 640, height: 360, fps: 24, fpsTolerance: 0.5, minDurationSec: 0.5 };

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

/* -------------------------------- 校验逻辑 -------------------------------- */

function checkOne(file) {
  const problems = [];
  const warnings = [];
  let info;
  try {
    info = probe(file);
  } catch (e) {
    return { file, ok: false, problems: [`parse error: ${e.message}`], warnings, info: null };
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

  return { file, ok: problems.length === 0, problems, warnings, info };
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
function checkManifest(files) {
  const problems = [];
  const warnings = [];
  const manifestPath = resolve(DEFAULT_DIR, '../manifest.json');
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
  } catch (e) {
    return { problems: [`manifest parse error: ${e.message}`], warnings };
  }
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
const targets = args.filter((a) => !a.startsWith('--'));
const files = listWebm(targets.length ? targets : [DEFAULT_DIR]);

if (files.length === 0) {
  console.error('no .webm found');
  process.exit(1);
}

const results = files.map(checkOne);
const isDefaultDir = targets.length === 0;
const manifestCheck = isDefaultDir ? checkManifest(files) : { problems: [], warnings: [] };

if (asJson) {
  console.log(
    JSON.stringify(
      {
        expected: EXPECT,
        files: results.map((r) => ({
          file: basename(r.file),
          ok: r.ok,
          problems: r.problems,
          warnings: r.warnings,
          info: r.info,
        })),
        manifest: manifestCheck,
        failed: results.filter((r) => !r.ok).length,
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
  }
  for (const p of manifestCheck.problems) console.log(`[FAIL] manifest: ${p}`);
  for (const w of manifestCheck.warnings) console.log(`[WARN] manifest: ${w}`);
}

const failed = results.filter((r) => !r.ok).length + manifestCheck.problems.length;
if (!asJson) {
  console.log(
    `\n${files.length - results.filter((r) => !r.ok).length}/${files.length} passed, ${failed} failed`,
  );
  if (failed === 0) {
    console.log('note: 像素级检查（透明占比 / 脚底对齐）需解码，留给 build-asset.mjs 用 ffmpeg 做');
  }
}
process.exit(failed === 0 ? 0 : 1);
