#!/usr/bin/env node
/**
 * 桌宠素材批量下载器（上游 `PC2005-cloud/dsh-pet` → 本仓库）
 *
 * 用途：把上游 `dsh-pet/assets/webm/` 里尚未入库的动作一次性下载到
 * `src/assets/pet/videos/<ascii-id>.webm`。上游素材**已是成品**（640×360 / 24fps /
 * VP9+alpha），无需跑 tools/pet-pipeline 的四步加工管线，落盘 + 登记 manifest 即可。
 *
 * 用法：
 *   node tools/pet-pipeline/fetch-webm.mjs               # 下载缺失的素材（幂等，已存在且大小相符则跳过）
 *   node tools/pet-pipeline/fetch-webm.mjs --list        # 只列目录与下载状态，不下载
 *   node tools/pet-pipeline/fetch-webm.mjs --emit         # 打印可直接粘进 manifest.json 的 actions 行
 *   node tools/pet-pipeline/fetch-webm.mjs --uploaded     # 同时拉取上游目录做「清单一致性」核对
 *
 * 设计要点：
 * - `CATALOG` 是**唯一事实来源**：中文原名 → ASCII id / group / loop。id 也是落盘文件名
 *   （`<id>.webm`）。出现未登记的上游文件时**报错退出**，避免漏挂后无法察觉。
 * - 下载按 `download_url` 走 raw.githubusercontent，逐条比对字节数，失败退避重试 3 次。
 * - 本脚本是**开发期工具**：不进安装包、不参与 vite build。
 */
import { existsSync, mkdirSync, statSync, writeFileSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const PIPELINE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(PIPELINE, '..', '..');
const OUT_DIR = join(ROOT, 'src', 'assets', 'pet', 'videos');
const MANIFEST = join(ROOT, 'src', 'assets', 'pet', 'manifest.json');
const API = 'https://api.github.com/repos/PC2005-cloud/dsh-pet/contents/dsh-pet/assets/webm';

/**
 * 上游中文原名 → { id, group, loop }。
 * group 取值为：idle / work / balance / click / mumble / drag / turn / jump / festival / life / talent / sfx。
 * 规则：`<group>-<ascii-slug>`，全小写 kebab；**新增动作一律 loop:false**
 * （瞬态只播一遍；loop:true 永不触发 onEnded，只能靠 12s 兜底硬切），
 * 例外是「展示类」两个（整体换装试色 / 小幅度原地360度旋转展示）——它们可循环，但不进随机池。
 */
const CATALOG = [
  // ── 已入库的 31 个业务动作（不重复下载，仅登记以完成清单一致性核对）──
  { name: '待机呼吸休闲', id: 'idle-breathe', group: 'idle', loop: true },
  { name: '原地漂浮踏步', id: 'idle-float', group: 'idle', loop: true },
  { name: '原地小憩沉眠', id: 'idle-nap', group: 'idle', loop: true },
  { name: '哈欠连天', id: 'idle-yawn', group: 'idle', loop: false },
  { name: '东张西望', id: 'idle-look', group: 'idle', loop: false },
  { name: '打瞌睡被惊醒', id: 'idle-wake', group: 'idle', loop: false },
  { name: '超大伸懒腰', id: 'idle-stretch', group: 'idle', loop: false },
  { name: '原地重力下蹲压缩', id: 'idle-squat', group: 'idle', loop: false },
  { name: '工作状态-忙碌点按', id: 'work-busy', group: 'work', loop: true },
  { name: '工作状态-思考冒泡', id: 'work-think', group: 'work', loop: true },
  { name: '工作状态-原地踱步张望', id: 'work-pace', group: 'work', loop: true },
  { name: '工作状态-雀跃庆祝', id: 'work-celebrate', group: 'work', loop: false },
  { name: '工作状态-垂头叹气冒汗', id: 'work-sad', group: 'work', loop: false },
  { name: '工作状态-清点归档', id: 'work-archive', group: 'work', loop: false },
  { name: '余额-钱袋满溢', id: 'balance-full', group: 'balance', loop: false },
  { name: '余额-数金皱眉', id: 'balance-count', group: 'balance', loop: false },
  { name: '余额-钱袋如常', id: 'balance-normal', group: 'balance', loop: false },
  { name: '余额-金袋叮当', id: 'balance-jingle', group: 'balance', loop: false },
  { name: '余额-分文不剩', id: 'balance-empty', group: 'balance', loop: false },
  { name: '余额-袋空如洗', id: 'balance-broke', group: 'balance', loop: false },
  { name: '点击回应-元气挥手', id: 'click-wave', group: 'click', loop: false },
  { name: '点击回应-开心跃动', id: 'click-happy', group: 'click', loop: false },
  { name: '点击回应-傲娇生气', id: 'click-tsundere', group: 'click', loop: false },
  { name: '点击回应-害羞惊讶', id: 'click-shy', group: 'click', loop: false },
  { name: '点击回应-挠痒咯咯笑', id: 'click-tickle', group: 'click', loop: false },
  { name: '碎碎念-发呆碎碎念', id: 'mumble-daze', group: 'mumble', loop: false },
  { name: '碎碎念-对屏碎碎念', id: 'mumble-screen', group: 'mumble', loop: false },
  { name: '深度思考碎碎念', id: 'mumble-deep', group: 'mumble', loop: false },
  { name: '被鼠标拖拽悬空反馈', id: 'drag-dangle', group: 'drag', loop: false },
  { name: '原地左转奔跑', id: 'turn-run', group: 'turn', loop: false },
  { name: '原地跳跃抓碎头顶物品', id: 'jump-grab', group: 'jump', loop: false },

  // ── 节日 / 时令（festival）：按日期触发 ──
  { name: '写福字', id: 'festival-spring-fu', group: 'festival', loop: false },
  { name: '吃年糕', id: 'festival-spring-niangao', group: 'festival', loop: false },
  { name: '收红包', id: 'festival-spring-redpacket', group: 'festival', loop: false },
  { name: '放烟花', id: 'festival-spring-fireworks', group: 'festival', loop: false },
  { name: '舞狮头', id: 'festival-spring-lion', group: 'festival', loop: false },
  { name: '吃汤圆', id: 'festival-lantern-tangyuan', group: 'festival', loop: false },
  { name: '放孔明灯', id: 'festival-lantern-sky', group: 'festival', loop: false },
  { name: '吃青团', id: 'festival-qingming-qingtuan', group: 'festival', loop: false },
  { name: '放风筝', id: 'festival-qingming-kite', group: 'festival', loop: false },
  { name: '吃粽子', id: 'festival-dragonboat-zongzi', group: 'festival', loop: false },
  { name: '穿针乞巧', id: 'festival-qixi', group: 'festival', loop: false },
  { name: '放河灯', id: 'festival-ghost-riverlamp', group: 'festival', loop: false },
  { name: '中秋赏月吃月饼', id: 'festival-midautumn', group: 'festival', loop: false },
  { name: '吃重阳糕', id: 'festival-doubleninth-cake', group: 'festival', loop: false },
  { name: '插茱萸赏菊', id: 'festival-doubleninth-chrysanthemum', group: 'festival', loop: false },
  { name: '吃腊八粥', id: 'festival-laba-porridge', group: 'festival', loop: false },
  { name: '吃饺子', id: 'festival-winter-solstice-dumpling', group: 'festival', loop: false },
  { name: '讨糖南瓜灯', id: 'festival-halloween', group: 'festival', loop: false },
  { name: '装点圣诞树', id: 'festival-christmas', group: 'festival', loop: false },

  // ── 饮食起居 / 日常（life）──
  { name: '吃早餐', id: 'life-breakfast', group: 'life', loop: false },
  { name: '吃午餐', id: 'life-lunch', group: 'life', loop: false },
  { name: '吃晚餐', id: 'life-dinner', group: 'life', loop: false },
  { name: '吃白饭', id: 'life-rice', group: 'life', loop: false },
  { name: '大口吃零食', id: 'life-snack-big', group: 'life', loop: false },
  { name: '偷吃零食被抓住', id: 'life-snack-caught', group: 'life', loop: false },
  { name: '吃冰淇淋融化', id: 'life-icecream-melt', group: 'life', loop: false },
  { name: '吃糖葫芦', id: 'life-tanghulu', group: 'life', loop: false },
  { name: '吃西瓜', id: 'life-watermelon', group: 'life', loop: false },
  { name: '吃大闸蟹', id: 'life-crab', group: 'life', loop: false },
  { name: '吃长寿面', id: 'life-longevity-noodles', group: 'life', loop: false },
  { name: '涮火锅', id: 'life-hotpot', group: 'life', loop: false },
  { name: '晨间刷牙', id: 'life-brushing', group: 'life', loop: false },
  { name: '照镜子', id: 'life-mirror', group: 'life', loop: false },
  { name: '撸猫', id: 'life-cat', group: 'life', loop: false },
  { name: '摇扇纳凉', id: 'life-fan', group: 'life', loop: false },
  { name: '堆雪人', id: 'life-snowman', group: 'life', loop: false },
  { name: '荡秋千', id: 'life-swing', group: 'life', loop: false },
  { name: '骑木马', id: 'life-hobbyhorse', group: 'life', loop: false },
  { name: '踢毽子', id: 'life-jianzi', group: 'life', loop: false },
  { name: '抽陀螺', id: 'life-spinning-top', group: 'life', loop: false },
  { name: '拆礼物', id: 'life-gift', group: 'life', loop: false },
  { name: '下五子棋', id: 'life-gomoku', group: 'life', loop: false },
  { name: '原地专心玩魔方', id: 'life-rubiks', group: 'life', loop: false },
  { name: '原地蹲下玩玩具汽车', id: 'life-toy-car', group: 'life', loop: false },
  { name: '玩游戏气急败坏', id: 'life-game-rage', group: 'life', loop: false },
  { name: '玩水枪', id: 'life-watergun', group: 'life', loop: false },
  { name: '吹气球', id: 'life-balloon', group: 'life', loop: false },
  { name: '被吓一跳', id: 'life-scared', group: 'life', loop: false },

  // ── 才艺 / 玩物 / 表演（talent）──
  { name: '三球抛接', id: 'talent-juggling', group: 'talent', loop: false },
  { name: '优雅女仆舞', id: 'talent-maid-dance', group: 'talent', loop: false },
  { name: '可爱宅舞', id: 'talent-otaku-dance', group: 'talent', loop: false },
  { name: '女仆屈膝礼仪', id: 'talent-maid-curtsy', group: 'talent', loop: false },
  { name: '小提琴演奏', id: 'talent-violin', group: 'talent', loop: false },
  { name: '吹笛子', id: 'talent-flute', group: 'talent', loop: false },
  { name: '悠闲哼歌', id: 'talent-humming', group: 'talent', loop: false },
  { name: '扑克魔术', id: 'talent-card-magic', group: 'talent', loop: false },
  { name: '变鸽子', id: 'talent-dove-magic', group: 'talent', loop: false },
  { name: '凭空生花', id: 'talent-flower-magic', group: 'talent', loop: false },
  { name: '写代码', id: 'talent-coding', group: 'talent', loop: false },
  { name: '原地敲击桌面互动', id: 'talent-desk-tap', group: 'talent', loop: false },
  { name: '轻快摇摆舞', id: 'talent-swing-dance', group: 'talent', loop: false },
  { name: '轻快记录', id: 'talent-quick-note', group: 'talent', loop: false },
  { name: '螃蟹走路', id: 'talent-crab-walk', group: 'talent', loop: false },
  { name: '吃Token', id: 'talent-eat-token', group: 'talent', loop: false },

  // ── 特效 / 奇观（sfx）──
  { name: '动物环绕', id: 'sfx-animals-ring', group: 'sfx', loop: false },
  { name: '用鲸鱼尾巴拍打地面', id: 'sfx-tail-slap', group: 'sfx', loop: false },
  { name: '萌化小幽灵', id: 'sfx-cute-ghost', group: 'sfx', loop: false },
  { name: '蓝鲸现世', id: 'sfx-whale-rise', group: 'sfx', loop: false },
  { name: '蝴蝶蜜蜂环绕头顶开花', id: 'sfx-butterfly-bee-bloom', group: 'sfx', loop: false },
  { name: '被落叶淹没', id: 'sfx-autumn-leaves', group: 'sfx', loop: false },
  { name: '鲸鱼吐泡泡特效', id: 'sfx-bubbles', group: 'sfx', loop: false },
  // 展示类：可循环，但**不进随机池**（由右键手动点播）
  { name: '小幅度原地360度旋转展示', id: 'sfx-spin-showcase', group: 'sfx', loop: true },
  { name: '整体换装试色', id: 'sfx-outfit-showcase', group: 'sfx', loop: true },

  // ── 碎碎念（mumble，沿用既有 group）──
  { name: '碎碎念-擦桌碎碎念', id: 'mumble-table-wipe', group: 'mumble', loop: false },
  { name: '是啊，吃什么', id: 'mumble-what-to-eat', group: 'mumble', loop: false },
];

/* --------------------------------- 工具 --------------------------------- */

const log = (m) => process.stdout.write(`${m}\n`);
const fail = (m) => {
  process.stderr.write(`\n[fetch-webm] ✗ ${m}\n`);
  process.exit(1);
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 拉上游目录清单（106 条，含 download_url / size / sha） */
async function fetchUpstreamIndex() {
  const res = await fetch(API, {
    headers: { 'User-Agent': 'chubbyfish-pet-fetch', Accept: 'application/vnd.github+json' },
  });
  if (!res.ok) fail(`拉取上游目录失败：HTTP ${res.status}（可能是 GitHub API 限流，稍后重试）`);
  const list = await res.json();
  if (!Array.isArray(list) || list.length === 0) fail('上游目录返回为空');
  const byName = new Map();
  for (const item of list) {
    if (!item.name?.endsWith('.webm')) continue;
    // 上游可能在中文文件名末尾带 .LFS 后缀等，这里只认 webm
    byName.set(item.name.replace(/\.webm$/, ''), item);
  }
  return byName;
}

/** 下载单条（含退避重试），返回落地状态 */
async function downloadOne(entry, item) {
  const out = join(OUT_DIR, `${entry.id}.webm`);
  const expectBytes = item.size;

  if (existsSync(out)) {
    const have = statSync(out).size;
    if (have === expectBytes) return { entry, status: 'skip' };
    log(`  ⟳ ${entry.name} 已有文件但大小不符（${have} ≠ ${expectBytes}），重新下载`);
  }

  let lastErr = '';
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const res = await fetch(item.download_url, { headers: { 'User-Agent': 'chubbyfish-pet-fetch' } });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const buf = Buffer.from(await res.arrayBuffer());
      if (buf.length !== expectBytes) {
        throw new Error(`字节数不符 ${buf.length} ≠ ${expectBytes}`);
      }
      writeFileSync(out, buf);
      return { entry, status: 'ok', bytes: buf.length };
    } catch (e) {
      lastErr = e.message;
      if (attempt < 3) await sleep(800 * attempt);
    }
  }
  return { entry, status: 'fail', error: lastErr };
}

/** 打印可粘进 manifest.json 的行（与现有单行风格一致） */
function emitManifestLines() {
  for (const e of CATALOG) {
    log(`    { "id": "${e.id}", "label": "${e.name}", "group": "${e.group}", "loop": ${e.loop} },`);
  }
}

/* --------------------------------- 入口 --------------------------------- */

const args = process.argv.slice(2);
const listOnly = args.includes('--list');
const emitOnly = args.includes('--emit');
const checkUpstream = args.includes('--uploaded');

if (emitOnly) {
  emitManifestLines();
  process.exit(0);
}

// 校验 id 唯一（防手滑写重复）
const idSet = new Set();
for (const e of CATALOG) {
  if (idSet.has(e.id)) fail(`CATALOG 里 id 重复：${e.id}`);
  idSet.add(e.id);
  if (!/^[a-z0-9]+(-[a-z0-9]+)*$/.test(e.id)) fail(`id 不是小写 kebab：${e.id}`);
}

mkdirSync(OUT_DIR, { recursive: true });

const index = await fetchUpstreamIndex();
log(`上游素材：${index.size} 个；本目录登记：${CATALOG.size ?? CATALOG.length} 个`);

// 一致性核对：上游有但未登记 → 报错（防漏挂）
const unmapped = [...index.keys()].filter((n) => !CATALOG.some((e) => e.name === n));
if (unmapped.length) {
  fail(
    `上游有 ${unmapped.length} 个素材未在 CATALOG 登记，请先补映射表（不可静默跳过）：\n  ` +
      unmapped.join('\n  '),
  );
}

// 已入库的 31 个业务动作不在 CATALOG 内，核对是否存在反向遗漏（CATALOG 条目在上游缺失）
const missingUpstream = CATALOG.filter((e) => !index.has(e.name));
if (missingUpstream.length) {
  fail(`CATALOG 里有 ${missingUpstream.length} 条在上游找不到同名文件：\n  ` + missingUpstream.map((e) => e.name).join('\n  '));
}

log(`上游素材 ${index.size} 个 ↔ CATALOG ${CATALOG.length} 条：清单一致`);

const pending = CATALOG.filter((e) => !existsSync(join(OUT_DIR, `${e.id}.webm`)));
log(`已落盘 ${CATALOG.length - pending.length} 个；待下载 ${pending.length} 个${listOnly ? '（--list 模式，不实际下载）' : ''}`);

if (listOnly) {
  for (const e of CATALOG) {
    const p = join(OUT_DIR, `${e.id}.webm`);
    const st = existsSync(p) ? '✓ 已有' : '· 缺失';
    log(`  ${st}  ${e.id.padEnd(38)} ${e.name}`);
  }
  process.exit(0);
}

let ok = 0;
let skip = 0;
const failures = [];
for (const e of CATALOG) {
  const item = index.get(e.name);
  const r = await downloadOne(e, item);
  if (r.status === 'ok') {
    ok += 1;
    log(`  ✓ ${e.id}  ← ${e.name}（${(r.bytes / 1024).toFixed(0)} KB）`);
  } else if (r.status === 'skip') {
    skip += 1;
  } else {
    failures.push(r);
    log(`  ✗ ${e.id}  ← ${e.name}：${r.error}`);
  }
  await sleep(120); // 轻微限速，避免被 raw.githubusercontent 拒绝
}

log('');
log(`完成：新下载 ${ok} 个 / 跳过 ${skip} 个 / 失败 ${failures.length} 个`);
if (failures.length) {
  fail(`以下 ${failures.length} 条下载失败，请重跑：\n  ` + failures.map((f) => f.entry.name).join('\n  '));
}
log('');
log('后续步骤：');
log('  1) node tools/pet-pipeline/fetch-webm.mjs --emit   → 把输出行粘进 src/assets/pet/manifest.json 的 actions');
log('  2) 同步 manifest 的 source.selected: 106 与 source.note');
log('  3) node tools/pet-pipeline/check-asset.mjs --pixels  → 期望 106/106 passed');
log('  4) node tools/pet-pipeline/fetch-webm.mjs --uploaded 可再次核对清单一致性');
