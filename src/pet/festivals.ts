/**
 * 节日日历（节日动作的日期触发依据）
 *
 * 两级规则：
 * - `SOLAR_RULES`：**公历固定**节日，月/日常年不变（元旦、劳动节、儿童节、国庆、万圣、圣诞）；
 * - `LUNAR_BY_YEAR`：**农历节日 + 节气**，每年公历日期不同，故按年预生成静态表。
 *
 * ## 关于农历表的来源与维护（重要）
 *
 * 本项目**刻意不引入任何农历/第三方运行时依赖**（与"零新增依赖"的既有倾向一致），
 * 改为内置 2026–2030 五年的静态映射表。
 *
 * 数据来源：以 `lunar-javascript`（开发期一次性换算工具，**未成为项目依赖**）批量换算，
 * 并于 2026-10-20 对 2026 年数据做了独立交叉验证——
 * 春节 02-17 与 IMCCE（法国天文与历算研究所）《Calendrier chinois pour 2026》一致；
 * 春节假期 02-15~02-23、中秋 09-25 与国务院办公厅《关于 2026 年部分节假日安排的通知》一致。
 *
 * 表内每年固定 10 条：春节、元宵、清明、端午、七夕、中元、中秋、重阳、腊八、冬至。
 * 其中「清明 / 冬至」是**节气**（按太阳黄经定，非农历月日），换算方式与农历月日不同，已一并预生成。
 *
 * **到期维护**：2030 年之后本表不再命中农历类节日（`getTodaysFestivalPlan` 会静默回退到公历规则，
 * 开发期会 `console.warn` 提示）。续写方式：用同一工具重算下一批年份并追加进 `LUNAR_BY_YEAR`。
 */

/** 节日规则：命中当天后可播放的动作（`actions` 按优先序取第一个有素材的） */
export type FestivalRule = {
  /** 稳定标识（用于「当天已播过」的去重键） */
  id: string;
  /** 中文名（可作气泡文案） */
  label: string;
  /** 公历月（1–12） */
  month: number;
  /** 公历日（1–31） */
  day: number;
  /** 候选动作 id（manifest 里的 id） */
  actions: string[];
  /** 备注（数据来源 / 特殊说明） */
  note?: string;
};

/** 公历固定节日（月/日不变） */
export const SOLAR_RULES: FestivalRule[] = [
  { id: 'new-year', label: '元旦', month: 1, day: 1, actions: ['festival-spring-fireworks', 'jump-grab'] },
  { id: 'valentine', label: '情人节', month: 2, day: 14, actions: ['talent-flower-magic', 'sfx-butterfly-bee-bloom'] },
  { id: 'labor', label: '劳动节', month: 5, day: 1, actions: ['talent-coding', 'life-fan'] },
  { id: 'children', label: '儿童节', month: 6, day: 1, actions: ['life-balloon', 'life-hobbyhorse'] },
  { id: 'national', label: '国庆节', month: 10, day: 1, actions: ['festival-spring-fireworks', 'festival-spring-lion'] },
  { id: 'halloween', label: '万圣节', month: 10, day: 31, actions: ['festival-halloween'] },
  { id: 'christmas', label: '圣诞节', month: 12, day: 25, actions: ['festival-christmas'] },
];

/**
 * 农历节日 / 节气 → 公历日期（2026–2030）
 *
 * 每行格式：`'MM-DD key'`，key 见下方 `LUNAR_META`。
 * 每年固定 10 条，顺序无关（运行时会逐条比对月/日）。
 */
const LUNAR_BY_YEAR: Record<number, string[]> = {
  2026: ['01-26 laba', '02-17 spring', '03-03 lantern', '04-05 qingming', '06-19 dragonboat', '08-19 qixi', '08-27 ghost', '09-25 midautumn', '10-18 doubleninth', '12-22 winter'],
  2027: ['01-15 laba', '02-06 spring', '02-20 lantern', '04-05 qingming', '06-09 dragonboat', '08-08 qixi', '08-16 ghost', '09-15 midautumn', '10-08 doubleninth', '12-22 winter'],
  2028: ['01-04 laba', '01-26 spring', '02-09 lantern', '04-04 qingming', '05-28 dragonboat', '08-26 qixi', '09-03 ghost', '10-03 midautumn', '10-26 doubleninth', '12-21 winter'],
  2029: ['01-22 laba', '02-13 spring', '02-27 lantern', '04-04 qingming', '06-16 dragonboat', '08-16 qixi', '08-24 ghost', '09-22 midautumn', '10-16 doubleninth', '12-21 winter'],
  2030: ['01-11 laba', '02-03 spring', '02-17 lantern', '04-05 qingming', '06-05 dragonboat', '08-05 qixi', '08-13 ghost', '09-12 midautumn', '10-05 doubleninth', '12-22 winter'],
};

/** key → 元信息（标签 / 可播动作）。`actions` 为空表示该节日暂无对应素材，命中时只冒泡不播动作 */
const LUNAR_META: Record<string, { label: string; actions: string[] }> = {
  spring: { label: '春节', actions: ['festival-spring-fireworks', 'festival-spring-redpacket', 'festival-spring-niangao', 'festival-spring-fu', 'festival-spring-lion'] },
  lantern: { label: '元宵节', actions: ['festival-lantern-tangyuan', 'festival-lantern-sky'] },
  qingming: { label: '清明节', actions: ['festival-qingming-kite', 'festival-qingming-qingtuan'] },
  dragonboat: { label: '端午节', actions: ['festival-dragonboat-zongzi'] },
  qixi: { label: '七夕', actions: ['festival-qixi', 'talent-flower-magic'] },
  ghost: { label: '中元节', actions: ['festival-ghost-riverlamp', 'sfx-cute-ghost'] },
  midautumn: { label: '中秋节', actions: ['festival-midautumn'] },
  doubleninth: { label: '重阳节', actions: ['festival-doubleninth-chrysanthemum', 'festival-doubleninth-cake'] },
  laba: { label: '腊八节', actions: ['festival-laba-porridge'] },
  winter: { label: '冬至', actions: ['festival-winter-solstice-dumpling'] },
};

/** 今天命中的节日计划 */
export type FestivalPlan = {
  /** 当日唯一键：`YYYY-MM-DD#<ruleId>`，用于「当天只播一次」去重 */
  key: string;
  label: string;
  actions: string[];
};

/** 本地日期键（用本地时区，避免 UTC 偏移导致「节日错一天」） */
export function localDateKey(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  return `${y}-${m}-${day}`;
}

/**
 * 取指定日期命中的节日。
 *
 * 优先级：**农历节日 / 节气 > 公历固定节日**（同日冲突时以传统节日为准，如 2026 年中秋不冲突，
 * 但个别年份会出现「国庆撞中秋」之类，此时按传统节日播）。
 *
 * 农历表未覆盖该年份时静默回退到公历规则；开发期 `console.warn` 提醒续表。
 * @param now 当前时间（测试可注入）
 */
export function getTodaysFestivalPlan(now: Date = new Date()): FestivalPlan | null {
  const year = now.getFullYear();
  const month = now.getMonth() + 1;
  const day = now.getDate();
  const dateKey = localDateKey(now);

  const rows = LUNAR_BY_YEAR[year];
  if (!rows) {
    if (import.meta.env?.DEV) {
      console.warn(
        `[pet] 节日表未覆盖 ${year} 年（当前仅 2026–2030），农历类节日将不会触发；` +
          '请按 src/pet/festivals.ts 头部说明续写 LUNAR_BY_YEAR。',
      );
    }
  } else {
    const mmdd = `${String(month).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
    const hit = rows.find((r) => r.startsWith(mmdd));
    if (hit) {
      const key = hit.split(' ')[1];
      const meta = LUNAR_META[key];
      if (meta) {
        return { key: `${dateKey}#${key}`, label: meta.label, actions: meta.actions };
      }
    }
  }

  const solar = SOLAR_RULES.find((r) => r.month === month && r.day === day);
  if (solar) {
    return { key: `${dateKey}#${solar.id}`, label: solar.label, actions: solar.actions };
  }

  return null;
}
