/**
 * 全局 API 管理 · 用量统计（unified-api-gateway-design §5.2/§5.3）
 * days（Trae 桶）/ wb_days（Buddy 桶）/ custom_days（自定义模型桶）/ qoder_days（Qoder 桶）
 * 四桶聚合，参数化资源池筛选（全部/Trae/Buddy/Qoder/自定义）；含模型分布 Top5（按所选池聚合）。
 * 数据源：api_usage_stats / api_wb_usage_stats / api_custom_usage_stats / api_qoder_usage_stats
 * （落盘数据，服务未运行也可查看，口径与键名不变 §9.6）。
 *
 * 视图分层（自上而下）：① 工具条（范围/资源池/刷新）② 概览指标 ③ 按日趋势（请求/Token/耗时
 * 三口径切换）④ 模型维度（分布 Top5 + 延迟分位 Top5）⑤ 按 API Key 统计（各 Key 请求/Token/
 * 成功率 + 该 Key 的模型下钻明细）。
 */
import { Fragment, useCallback, useEffect, useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { BarChart3, ChevronDown, ChevronRight, KeyRound, RefreshCw } from 'lucide-react';
import {
  Bar,
  BarChart,
  CartesianGrid,
  Legend,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';
import { Badge, StatCard } from '../ui';
import { api } from '../../lib/tauri';
import { fmtTokens } from '../../lib/format';
import { cn } from '../../lib/cn';
import type {
  ApiKeyEntry,
  UsageCounterView,
  UsageDayView,
  UsageKeyTokenView,
  UsageKeyUsageView,
  UsageModelLatencyView,
} from '../../types';

/** 资源池筛选（§5.2：全部/Trae/Buddy/Qoder/自定义模型） */
export type PoolFilter = 'all' | 'trae' | 'buddy' | 'qoder' | 'custom';

const FILTER_LABELS: Record<PoolFilter, string> = {
  all: '全部',
  trae: 'Trae',
  buddy: 'Buddy',
  qoder: 'Qoder',
  custom: '自定义',
};

/** 趋势图口径 */
type Metric = 'requests' | 'tokens' | 'latency';

const DAY_OPTIONS: { key: number; label: string }[] = [
  { key: 7, label: '7 天' },
  { key: 14, label: '14 天' },
  { key: 30, label: '30 天' },
];

const METRIC_OPTIONS: { key: Metric; label: string }[] = [
  { key: 'requests', label: '请求' },
  { key: 'tokens', label: 'Token' },
  { key: 'latency', label: '耗时' },
];

/** 同名计数器合并（模型/账号/Key 维度通用） */
function mergeCounters(a: UsageCounterView[], b: UsageCounterView[]): UsageCounterView[] {
  const map = new Map<string, UsageCounterView>();
  for (const c of [...a, ...b]) {
    const e = map.get(c.name);
    if (e) {
      e.requests += c.requests;
      e.ok += c.ok;
      e.errors += c.errors;
      e.credits += c.credits;
    } else {
      map.set(c.name, { ...c });
    }
  }
  return [...map.values()];
}

/** 按 Key 的 token 用量合并 */
function mergeKeyTokens(a: UsageKeyTokenView[], b: UsageKeyTokenView[]): UsageKeyTokenView[] {
  const map = new Map<string, UsageKeyTokenView>();
  for (const t of [...a, ...b]) {
    const e = map.get(t.name);
    if (e) {
      e.prompt_tokens += t.prompt_tokens;
      e.completion_tokens += t.completion_tokens;
    } else {
      map.set(t.name, { ...t });
    }
  }
  return [...map.values()];
}

/** 按 Key 的完整视图合并（请求/成功失败/token + 模型明细） */
function mergeKeyUsage(a: UsageKeyUsageView[] = [], b: UsageKeyUsageView[] = []): UsageKeyUsageView[] {
  const map = new Map<string, UsageKeyUsageView>();
  for (const k of [...a, ...b]) {
    const e = map.get(k.name);
    if (!e) {
      map.set(k.name, {
        name: k.name,
        requests: k.requests,
        ok: k.ok,
        errors: k.errors,
        prompt_tokens: k.prompt_tokens,
        completion_tokens: k.completion_tokens,
        models: k.models.map((m) => ({ ...m })),
      });
      continue;
    }
    e.requests += k.requests;
    e.ok += k.ok;
    e.errors += k.errors;
    e.prompt_tokens += k.prompt_tokens;
    e.completion_tokens += k.completion_tokens;
    for (const m of k.models) {
      const ex = e.models.find((x) => x.model === m.model);
      if (ex) {
        ex.requests += m.requests;
        ex.ok += m.ok;
        ex.errors += m.errors;
        ex.prompt_tokens += m.prompt_tokens;
        ex.completion_tokens += m.completion_tokens;
      } else {
        e.models.push({ ...m });
      }
    }
  }
  return [...map.values()];
}

/** 两条日记录合并（avg_duration 按请求数加权；TTFT 按样本数加权） */
function mergeDay(a: UsageDayView, b: UsageDayView): UsageDayView {
  const total = a.total_requests + b.total_requests;
  const ttfbSamples = (a.ttfb_samples ?? 0) + (b.ttfb_samples ?? 0);
  const avgTtfb =
    ttfbSamples > 0
      ? Math.round(
          ((a.avg_ttfb_ms ?? 0) * (a.ttfb_samples ?? 0) +
            (b.avg_ttfb_ms ?? 0) * (b.ttfb_samples ?? 0)) /
            ttfbSamples,
        )
      : undefined;
  return {
    date: a.date,
    total_requests: total,
    ok: a.ok + b.ok,
    errors: a.errors + b.errors,
    stream_requests: a.stream_requests + b.stream_requests,
    prompt_tokens: a.prompt_tokens + b.prompt_tokens,
    completion_tokens: a.completion_tokens + b.completion_tokens,
    credits: a.credits + b.credits,
    original_credits: a.original_credits + b.original_credits,
    avg_duration_ms:
      total > 0
        ? Math.round(
            (a.avg_duration_ms * a.total_requests + b.avg_duration_ms * b.total_requests) / total,
          )
        : 0,
    p95_duration_ms: maxOpt(a.p95_duration_ms, b.p95_duration_ms),
    max_duration_ms: maxOpt(a.max_duration_ms, b.max_duration_ms),
    avg_ttfb_ms: avgTtfb,
    p95_ttfb_ms: maxOpt(a.p95_ttfb_ms, b.p95_ttfb_ms),
    ttfb_samples: ttfbSamples,
    models: mergeCounters(a.models, b.models),
    accounts: mergeCounters(a.accounts, b.accounts),
    keys: mergeCounters(a.keys, b.keys),
    key_tokens: mergeKeyTokens(a.key_tokens, b.key_tokens),
    key_usage: mergeKeyUsage(a.key_usage, b.key_usage),
    model_latency: mergeModelLatency(a.model_latency, b.model_latency),
  };
}

/** 可选数值取较大者（P95/最大值跨桶合并取保守值） */
function maxOpt(a?: number, b?: number): number | undefined {
  if (a == null && b == null) return undefined;
  return Math.max(a ?? 0, b ?? 0);
}

/** 多日可选 P95 取最大值（跨天聚合取保守值） */
function maxOptList(values: (number | undefined)[]): number | undefined {
  const vs = values.filter((v): v is number => v != null);
  return vs.length > 0 ? Math.max(...vs) : undefined;
}

/** 按模型的延迟分位合并（F-76①） */
function mergeModelLatency(
  a: UsageModelLatencyView[] = [],
  b: UsageModelLatencyView[] = [],
): UsageModelLatencyView[] {
  const map = new Map<string, UsageModelLatencyView>();
  for (const m of [...a, ...b]) {
    const e = map.get(m.model);
    if (e) {
      e.samples += m.samples;
      e.p50_duration_ms = maxOpt(e.p50_duration_ms, m.p50_duration_ms);
      e.p95_duration_ms = maxOpt(e.p95_duration_ms, m.p95_duration_ms);
      e.max_duration_ms = maxOpt(e.max_duration_ms, m.max_duration_ms);
      e.avg_ttfb_ms = maxOpt(e.avg_ttfb_ms, m.avg_ttfb_ms);
      e.p95_ttfb_ms = maxOpt(e.p95_ttfb_ms, m.p95_ttfb_ms);
    } else {
      map.set(m.model, { ...m });
    }
  }
  return [...map.values()].sort((x, y) => y.samples - x.samples);
}

/** 多桶按日合并（同日相加），按日期升序 */
function mergeBuckets(...buckets: UsageDayView[][]): UsageDayView[] {
  const map = new Map<string, UsageDayView>();
  for (const bucket of buckets) {
    for (const d of bucket) {
      const cur = map.get(d.date);
      map.set(d.date, cur ? mergeDay(cur, d) : { ...d });
    }
  }
  return [...map.values()].sort((x, y) => x.date.localeCompare(y.date));
}

/** 分段控制器（时间范围 / 资源池 / 趋势口径共用） */
function Segmented<T extends string | number>({
  options,
  value,
  onChange,
  ariaLabel,
}: {
  options: readonly { key: T; label: string }[];
  value: T;
  onChange: (v: T) => void;
  ariaLabel: string;
}) {
  return (
    <div
      role="group"
      aria-label={ariaLabel}
      className="inline-flex items-center gap-0.5 rounded-lg bg-slate-100/80 p-0.5 dark:bg-zinc-800/70"
    >
      {options.map((o) => (
        <button
          key={o.key}
          type="button"
          onClick={() => onChange(o.key)}
          className={cn(
            'rounded-md px-2.5 py-1 text-xs font-medium transition',
            value === o.key
              ? 'bg-white text-slate-800 shadow-sm dark:bg-zinc-700 dark:text-zinc-100'
              : 'text-slate-500 hover:text-slate-700 dark:text-zinc-400 dark:hover:text-zinc-200',
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** 分区卡片（统一标题/说明/右侧操作） */
function SectionCard({
  title,
  desc,
  right,
  children,
}: {
  title: string;
  desc?: string;
  right?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="card min-w-0 p-4">
      <div className="mb-3 flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0">
          <h4 className="text-sm font-semibold text-slate-800 dark:text-zinc-100">{title}</h4>
          {desc && <p className="mt-0.5 text-xs text-slate-400 dark:text-zinc-500">{desc}</p>}
        </div>
        {right}
      </div>
      {children}
    </section>
  );
}

/** 占比条 */
function ShareBar({ pct, tone = 'bg-brand-500', className }: { pct: number; tone?: string; className?: string }) {
  return (
    <div
      className={cn(
        'h-1.5 w-full overflow-hidden rounded-full bg-slate-100 dark:bg-zinc-800',
        className,
      )}
    >
      <div
        className={cn('h-full rounded-full transition-[width] duration-300', tone)}
        style={{ width: `${Math.min(100, Math.max(0, pct))}%` }}
      />
    </div>
  );
}

/** 单个 Key 的聚合中间态（模型维度用 Map 便于累加） */
interface KeyAgg {
  id: string;
  requests: number;
  ok: number;
  errors: number;
  prompt: number;
  completion: number;
  models: Map<string, { requests: number; ok: number; errors: number; prompt: number; completion: number }>;
}

/** 单个 Key 的聚合结果（渲染用） */
interface KeyRow {
  id: string;
  name: string;
  requests: number;
  ok: number;
  errors: number;
  prompt: number;
  completion: number;
  models: { model: string; requests: number; ok: number; errors: number; prompt: number; completion: number }[];
}

export default function UsageStatsPanel() {
  const [days, setDays] = useState<number>(14);
  const [filter, setFilter] = useState<PoolFilter>('all');
  const [metric, setMetric] = useState<Metric>('requests');
  const [traeUsage, setTraeUsage] = useState<UsageDayView[]>([]);
  const [buddyUsage, setBuddyUsage] = useState<UsageDayView[]>([]);
  const [customUsage, setCustomUsage] = useState<UsageDayView[]>([]);
  const [qoderUsage, setQoderUsage] = useState<UsageDayView[]>([]);
  /** API Key 条目（id → 名称映射；Key 用量按 id 记账） */
  const [apiKeys, setApiKeys] = useState<ApiKeyEntry[]>([]);
  const [loading, setLoading] = useState(false);
  /** 展开模型明细的 Key 集合 */
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  const load = async (d: number) => {
    setLoading(true);
    try {
      const [trae, buddy, custom, qoder] = await Promise.all([
        api.apiServer.usageStats(d).catch(() => [] as UsageDayView[]),
        api.apiServer.wbUsageStats(d).catch(() => [] as UsageDayView[]),
        api.apiServer.customUsageStats(d).catch(() => [] as UsageDayView[]),
        api.apiServer.qoderUsageStats(d).catch(() => [] as UsageDayView[]),
      ]);
      setTraeUsage(trae);
      setBuddyUsage(buddy);
      setCustomUsage(custom);
      setQoderUsage(qoder);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void load(days);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [days]);

  // Key 名称映射：历史用量按 Key id 记账，删除 Key 后仍可显示（回落「已删除」）
  useEffect(() => {
    api.apiServer
      .keysList()
      .then((v) => setApiKeys(v.keys))
      .catch(() => setApiKeys([]));
  }, []);

  // 按筛选聚合多桶（"全部"时相加，§8.2）
  const usage = useMemo(() => {
    if (filter === 'trae') return traeUsage;
    if (filter === 'buddy') return buddyUsage;
    if (filter === 'qoder') return qoderUsage;
    if (filter === 'custom') return customUsage;
    return mergeBuckets(traeUsage, buddyUsage, qoderUsage, customUsage);
  }, [filter, traeUsage, buddyUsage, qoderUsage, customUsage]);

  // 汇总（跨天聚合）+ 图表数据（与原页面统计逻辑一致，纯搬移）
  const summary = useMemo(() => {
    const models = new Map<string, { requests: number; ok: number; errors: number }>();
    const modelLatency = new Map<string, UsageModelLatencyView>();
    const t = usage.reduce(
      (acc, d) => {
        acc.requests += d.total_requests;
        acc.ok += d.ok;
        acc.errors += d.errors;
        acc.prompt += d.prompt_tokens;
        acc.completion += d.completion_tokens;
        acc.weightedDuration += d.avg_duration_ms * d.total_requests;
        acc.ttfbSamples += d.ttfb_samples ?? 0;
        acc.weightedTtfb += (d.avg_ttfb_ms ?? 0) * (d.ttfb_samples ?? 0);
        for (const m of d.models) {
          const e = models.get(m.name) ?? { requests: 0, ok: 0, errors: 0 };
          e.requests += m.requests;
          e.ok += m.ok;
          e.errors += m.errors;
          models.set(m.name, e);
        }
        for (const m of d.model_latency ?? []) {
          const e = modelLatency.get(m.model);
          if (e) {
            e.samples += m.samples;
            e.p95_duration_ms = maxOpt(e.p95_duration_ms, m.p95_duration_ms);
            e.max_duration_ms = maxOpt(e.max_duration_ms, m.max_duration_ms);
            e.avg_ttfb_ms = maxOpt(e.avg_ttfb_ms, m.avg_ttfb_ms);
            e.p95_ttfb_ms = maxOpt(e.p95_ttfb_ms, m.p95_ttfb_ms);
          } else {
            modelLatency.set(m.model, { ...m });
          }
        }
        return acc;
      },
      {
        requests: 0,
        ok: 0,
        errors: 0,
        prompt: 0,
        completion: 0,
        weightedDuration: 0,
        ttfbSamples: 0,
        weightedTtfb: 0,
      },
    );
    const topModels = [...models.entries()]
      .sort((a, b) => b[1].requests - a[1].requests)
      .slice(0, 5)
      .map(([name, v]) => ({ name, ...v }));
    const latencyModels = [...modelLatency.values()]
      .sort((a, b) => b.samples - a.samples)
      .slice(0, 5);
    return {
      ...t,
      topModels,
      latencyModels,
      successRate: t.requests > 0 ? ((t.ok / t.requests) * 100).toFixed(1) : '—',
      avgDuration: t.requests > 0 ? Math.round(t.weightedDuration / t.requests) : 0,
      avgTtfb: t.ttfbSamples > 0 ? Math.round(t.weightedTtfb / t.ttfbSamples) : undefined,
      p95Ttfb: maxOptList(usage.map((d) => d.p95_ttfb_ms)),
      p95Duration: maxOptList(usage.map((d) => d.p95_duration_ms)),
    };
  }, [usage]);

  // ---------- 按 API Key 统计 ----------
  const keyNames = useMemo(() => {
    const m = new Map<string, string>();
    for (const k of apiKeys) m.set(k.id, k.name);
    return m;
  }, [apiKeys]);

  /** Key id → 展示名（匿名流量单独标注，已删除 Key 保留历史并可识别） */
  const labelOf = useCallback(
    (id: string) => {
      if (id === 'anonymous') return '匿名请求（未鉴权）';
      return keyNames.get(id) ?? `已删除的 Key · ${id.slice(0, 8)}`;
    },
    [keyNames],
  );

  const keyRows = useMemo<KeyRow[]>(() => {
    const acc = new Map<string, KeyAgg>();
    const rowOf = (id: string): KeyAgg => {
      let r = acc.get(id);
      if (!r) {
        r = { id, requests: 0, ok: 0, errors: 0, prompt: 0, completion: 0, models: new Map() };
        acc.set(id, r);
      }
      return r;
    };
    for (const d of usage) {
      const detail = d.key_usage ?? [];
      if (detail.length > 0) {
        for (const k of detail) {
          const r = rowOf(k.name);
          r.requests += k.requests;
          r.ok += k.ok;
          r.errors += k.errors;
          r.prompt += k.prompt_tokens;
          r.completion += k.completion_tokens;
          for (const m of k.models) {
            const e =
              r.models.get(m.model) ??
              { requests: 0, ok: 0, errors: 0, prompt: 0, completion: 0 };
            e.requests += m.requests;
            e.ok += m.ok;
            e.errors += m.errors;
            e.prompt += m.prompt_tokens;
            e.completion += m.completion_tokens;
            r.models.set(m.model, e);
          }
        }
        continue;
      }
      // 兼容旧结构（无 key_usage）：计数与 token 分表合并
      for (const k of d.keys ?? []) {
        const r = rowOf(k.name);
        r.requests += k.requests;
        r.ok += k.ok;
        r.errors += k.errors;
      }
      for (const t of d.key_tokens ?? []) {
        const r = rowOf(t.name);
        r.prompt += t.prompt_tokens;
        r.completion += t.completion_tokens;
      }
    }
    return [...acc.values()]
      .map((r) => ({
        id: r.id,
        name: labelOf(r.id),
        requests: r.requests,
        ok: r.ok,
        errors: r.errors,
        prompt: r.prompt,
        completion: r.completion,
        models: [...r.models.entries()]
          .map(([model, s]) => ({ model, ...s }))
          .sort((a, b) => b.requests - a.requests || a.model.localeCompare(b.model)),
      }))
      .sort(
        (a, b) =>
          b.requests - a.requests ||
          b.prompt + b.completion - (a.prompt + a.completion) ||
          a.name.localeCompare(b.name),
      );
  }, [usage, labelOf]);

  const keyTotals = useMemo(
    () =>
      keyRows.reduce(
        (acc, r) => ({
          requests: acc.requests + r.requests,
          prompt: acc.prompt + r.prompt,
          completion: acc.completion + r.completion,
        }),
        { requests: 0, prompt: 0, completion: 0 },
      ),
    [keyRows],
  );

  const allExpanded = keyRows.length > 0 && keyRows.every((r) => expanded.has(r.id));
  const toggleKeyRow = (id: string) =>
    setExpanded((cur) => {
      const next = new Set(cur);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const toggleAllRows = () =>
    setExpanded(allExpanded ? new Set() : new Set(keyRows.map((r) => r.id)));

  /** 有请求但无模型明细 = 升级前的历史数据（模型维度不可回溯补算） */
  const missingModelDetail = keyRows.some((r) => r.requests > 0 && r.models.length === 0);

  // ---------- 趋势图 ----------
  const chartData = useMemo(
    () =>
      usage.map((d) => ({
        date: d.date.slice(5),
        成功: d.ok,
        失败: d.errors,
        输入: d.prompt_tokens,
        输出: d.completion_tokens,
        平均耗时: d.avg_duration_ms,
        'P95 耗时': d.p95_duration_ms ?? 0,
      })),
    [usage],
  );

  const series =
    metric === 'requests'
      ? [
          { key: '成功', color: '#10b981' },
          { key: '失败', color: '#f43f5e' },
        ]
      : metric === 'tokens'
        ? [
            { key: '输入', color: '#38bdf8' },
            { key: '输出', color: '#a78bfa' },
          ]
        : [
            { key: '平均耗时', color: '#f59e0b' },
            { key: 'P95 耗时', color: '#fb7185' },
          ];
  /** 耗时口径并列对比（堆叠会把分钟级与毫秒级混加，失真） */
  const stacked = metric !== 'latency';
  const unitSuffix = metric === 'tokens' ? ' tok' : metric === 'latency' ? ' ms' : ' 次';

  const filterHint =
    filter === 'all'
      ? 'Trae + Buddy + Qoder + 自定义 四桶聚合'
      : filter === 'custom'
        ? '仅自定义模型桶'
        : `仅 ${FILTER_LABELS[filter]} 桶`;

  return (
    <div className="space-y-3">
      {/* ① 工具条：范围 / 资源池 / 刷新 */}
      <div className="card flex flex-wrap items-center justify-between gap-2 px-4 py-3">
        <div className="flex min-w-0 items-center gap-2">
          <BarChart3 size={16} className="shrink-0 text-brand-500" />
          <div className="min-w-0">
            <h3 className="text-sm font-semibold text-slate-800 dark:text-zinc-100">用量统计</h3>
            <p className="truncate text-[11px] text-slate-400 dark:text-zinc-500">
              按日落盘 · 独立于服务运行状态 · {filterHint}
            </p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Segmented ariaLabel="统计时间范围" options={DAY_OPTIONS} value={days} onChange={setDays} />
          <Segmented
            ariaLabel="资源池筛选"
            options={(Object.keys(FILTER_LABELS) as PoolFilter[]).map((f) => ({
              key: f,
              label: FILTER_LABELS[f],
            }))}
            value={filter}
            onChange={setFilter}
          />
          <button
            className="btn-ghost flex items-center gap-1 !px-2 !py-1.5 text-xs"
            onClick={() => void load(days)}
            disabled={loading}
            title="重新读取落盘用量数据"
          >
            <RefreshCw size={13} className={loading ? 'animate-spin' : ''} />
            刷新
          </button>
        </div>
      </div>

      {/* ② 概览指标（5 块单行铺开） */}
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 xl:grid-cols-5">
        <StatCard
          label="总请求数"
          value={summary.requests}
          tone="brand"
          hint={`近 ${days} 天 · ${FILTER_LABELS[filter]}`}
        />
        <StatCard
          label="成功率"
          value={summary.successRate === '—' ? '—' : `${summary.successRate}%`}
          tone="green"
          hint={`失败 ${summary.errors} 次`}
        />
        <StatCard
          label="Token 消耗"
          value={fmtTokens(summary.prompt + summary.completion)}
          tone="amber"
          hint={`输入 ${fmtTokens(summary.prompt)} / 输出 ${fmtTokens(summary.completion)}`}
        />
        <StatCard
          label="平均耗时"
          value={summary.requests > 0 ? `${summary.avgDuration}ms` : '—'}
          tone="blue"
          hint={summary.p95Duration != null ? `按请求加权 · P95 ${summary.p95Duration}ms` : '按请求加权'}
        />
        <StatCard
          label="首字延迟 TTFT"
          value={summary.avgTtfb != null ? `${summary.avgTtfb}ms` : '—'}
          tone="violet"
          hint={
            summary.avgTtfb != null
              ? `P95 ${summary.p95Ttfb ?? '—'}ms · ${summary.ttfbSamples} 样本（流式成功）`
              : '暂无流式样本'
          }
        />
      </div>

      {/* ③ 按日趋势：请求 / Token / 耗时 三口径 */}
      <SectionCard
        title="按日趋势"
        desc={`近 ${days} 天 · ${filterHint}`}
        right={
          <Segmented ariaLabel="趋势口径" options={METRIC_OPTIONS} value={metric} onChange={setMetric} />
        }
      >
        {summary.requests > 0 ? (
          <div className="h-56 text-slate-500 dark:text-zinc-400">
            <ResponsiveContainer width="100%" height="100%">
              <BarChart data={chartData} margin={{ top: 4, right: 8, bottom: 0, left: -8 }}>
                <CartesianGrid strokeDasharray="3 3" stroke="currentColor" opacity={0.15} vertical={false} />
                <XAxis dataKey="date" tick={{ fill: 'currentColor', fontSize: 11 }} tickLine={false} />
                <YAxis
                  allowDecimals={false}
                  tick={{ fill: 'currentColor', fontSize: 11 }}
                  tickLine={false}
                  tickFormatter={
                    metric === 'tokens' ? (v: number) => fmtTokens(Number(v)) : undefined
                  }
                />
                <Tooltip
                  cursor={{ fill: 'rgba(120,120,120,0.08)' }}
                  contentStyle={{
                    borderRadius: 8,
                    border: '1px solid rgba(120,120,120,0.25)',
                    fontSize: 12,
                  }}
                  formatter={(v) => `${v}${unitSuffix}`}
                />
                <Legend wrapperStyle={{ fontSize: 12 }} />
                {series.map((s) => (
                  <Bar
                    key={s.key}
                    dataKey={s.key}
                    stackId={stacked ? 's' : undefined}
                    fill={s.color}
                    radius={[3, 3, 0, 0]}
                  />
                ))}
              </BarChart>
            </ResponsiveContainer>
          </div>
        ) : (
          <p className="py-8 text-center text-sm text-slate-400">
            暂无请求数据 — 发起一次 API 调用后这里会展示按日趋势（{filterHint}）
          </p>
        )}
      </SectionCard>

      {/* ④ 模型维度：分布 + 延迟分位 */}
      <div className="grid gap-3 xl:grid-cols-2">
        <SectionCard title="模型分布" desc={`近 ${days} 天 Top 5 · ${FILTER_LABELS[filter]}`}>
          {summary.topModels.length === 0 ? (
            <p className="py-6 text-center text-sm text-slate-400">暂无模型维度数据</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-slate-200 text-left text-xs text-slate-500 dark:border-zinc-700 dark:text-zinc-400">
                    <th className="pb-2 pr-3 font-medium">模型</th>
                    <th className="pb-2 pr-3 font-medium">请求数</th>
                    <th className="pb-2 pr-3 font-medium">成功</th>
                    <th className="pb-2 pr-3 font-medium">失败</th>
                    <th className="pb-2 font-medium">占比</th>
                  </tr>
                </thead>
                <tbody>
                  {summary.topModels.map((m) => {
                    const pct = summary.requests > 0 ? (m.requests / summary.requests) * 100 : 0;
                    return (
                      <tr key={m.name} className="border-b border-slate-100 last:border-0 dark:border-zinc-800">
                        <td className="max-w-[11rem] truncate py-2 pr-3 font-mono text-xs font-medium text-slate-700 dark:text-zinc-200" title={m.name}>
                          {m.name}
                        </td>
                        <td className="py-2 pr-3 tabular-nums text-slate-600 dark:text-zinc-300">{m.requests}</td>
                        <td className="py-2 pr-3 tabular-nums text-emerald-600 dark:text-emerald-400">{m.ok}</td>
                        <td className="py-2 pr-3 tabular-nums text-rose-600 dark:text-rose-400">{m.errors}</td>
                        <td className="w-32 py-2">
                          <div className="flex items-center gap-2">
                            <ShareBar pct={pct} />
                            <span className="w-11 shrink-0 text-right text-xs tabular-nums text-slate-400">
                              {pct.toFixed(1)}%
                            </span>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </SectionCard>

        <SectionCard title="模型延迟" desc={`近 ${days} 天 Top 5 · P50/P95 总耗时 + 首字延迟`}>
          {summary.latencyModels.length === 0 ? (
            <p className="py-6 text-center text-sm text-slate-400">暂无延迟样本</p>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-slate-200 text-left text-xs text-slate-500 dark:border-zinc-700 dark:text-zinc-400">
                    <th className="pb-2 pr-3 font-medium">模型</th>
                    <th className="pb-2 pr-3 font-medium">样本</th>
                    <th className="pb-2 pr-3 font-medium">P50</th>
                    <th className="pb-2 pr-3 font-medium">P95</th>
                    <th className="pb-2 pr-3 font-medium">平均 TTFT</th>
                    <th className="pb-2 font-medium">P95 TTFT</th>
                  </tr>
                </thead>
                <tbody>
                  {summary.latencyModels.map((m) => (
                    <tr key={m.model} className="border-b border-slate-100 last:border-0 dark:border-zinc-800">
                      <td className="max-w-[10rem] truncate py-2 pr-3 font-mono text-xs font-medium text-slate-700 dark:text-zinc-200" title={m.model}>
                        {m.model}
                      </td>
                      <td className="py-2 pr-3 tabular-nums text-slate-600 dark:text-zinc-300">{m.samples}</td>
                      <td className="py-2 pr-3 tabular-nums text-slate-600 dark:text-zinc-300">
                        {m.p50_duration_ms != null ? `${m.p50_duration_ms}ms` : '—'}
                      </td>
                      <td className="py-2 pr-3 tabular-nums text-slate-600 dark:text-zinc-300">
                        {m.p95_duration_ms != null ? `${m.p95_duration_ms}ms` : '—'}
                      </td>
                      <td className="py-2 pr-3 tabular-nums text-violet-600 dark:text-violet-400">
                        {m.avg_ttfb_ms != null ? `${m.avg_ttfb_ms}ms` : '—'}
                      </td>
                      <td className="py-2 tabular-nums text-violet-600 dark:text-violet-400">
                        {m.p95_ttfb_ms != null ? `${m.p95_ttfb_ms}ms` : '—'}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </SectionCard>
      </div>

      {/* ⑤ 按 API Key 统计（请求/Token/成功率 + 各 Key 的模型下钻） */}
      <SectionCard
        title="按 API Key 统计"
        desc={`近 ${days} 天 · ${FILTER_LABELS[filter]} · 展开可看该 Key 分别用了哪些模型`}
        right={
          keyRows.length > 0 ? (
            <div className="flex flex-wrap items-center gap-2 text-xs text-slate-400 dark:text-zinc-500">
              <span className="flex items-center gap-1">
                <KeyRound size={12} />
                {keyRows.length} 个 Key
              </span>
              <span className="text-slate-300 dark:text-zinc-700">|</span>
              <span className="tabular-nums">{keyTotals.requests} 次请求</span>
              <span className="text-slate-300 dark:text-zinc-700">|</span>
              <span className="tabular-nums">
                {fmtTokens(keyTotals.prompt + keyTotals.completion)} tok
              </span>
              <button className="btn-ghost !px-2 !py-1 text-xs" onClick={toggleAllRows}>
                {allExpanded ? '全部收起' : '全部展开'}
              </button>
            </div>
          ) : undefined
        }
      >
        {keyRows.length === 0 ? (
          <p className="py-6 text-center text-sm text-slate-400">
            暂无 Key 维度数据 — 用任一 Key 发起调用后，这里会按 Key 汇总请求数、Token 与所用模型
          </p>
        ) : (
          <>
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-slate-200 text-left text-xs text-slate-500 dark:border-zinc-700 dark:text-zinc-400">
                    <th className="pb-2 pr-3 font-medium">API Key</th>
                    <th className="w-36 pb-2 pr-3 font-medium">请求数</th>
                    <th className="pb-2 pr-3 font-medium">成功 / 失败</th>
                    <th className="pb-2 pr-3 font-medium">成功率</th>
                    <th className="pb-2 pr-3 font-medium">Token（入 / 出）</th>
                    <th className="pb-2 font-medium">使用的模型</th>
                  </tr>
                </thead>
                <tbody>
                  {keyRows.map((r) => {
                    const open = expanded.has(r.id);
                    const pct = keyTotals.requests > 0 ? (r.requests / keyTotals.requests) * 100 : 0;
                    const total = r.prompt + r.completion;
                    const rate = r.requests > 0 ? ((r.ok / r.requests) * 100).toFixed(1) : '—';
                    return (
                      <Fragment key={r.id}>
                        <tr className="row-hover border-b border-slate-100 last:border-0 dark:border-zinc-800">
                          <td className="py-2 pr-3">
                            <div className="flex items-center gap-1.5">
                              <span className="max-w-[13rem] truncate font-medium text-slate-700 dark:text-zinc-200" title={r.name}>
                                {r.name}
                              </span>
                              {r.id === 'anonymous' && <Badge tone="amber">未鉴权</Badge>}
                            </div>
                            {r.id !== 'anonymous' && !keyNames.has(r.id) && (
                              <span className="text-[10px] text-slate-400">Key 已删除，历史用量保留</span>
                            )}
                          </td>
                          <td className="py-2 pr-3">
                            <div className="flex items-center justify-between gap-2">
                              <span className="tabular-nums text-slate-600 dark:text-zinc-300">{r.requests}</span>
                              <span className="text-[10px] tabular-nums text-slate-400">{pct.toFixed(1)}%</span>
                            </div>
                            <ShareBar pct={pct} className="mt-1" />
                          </td>
                          <td className="py-2 pr-3 tabular-nums">
                            <span className="text-emerald-600 dark:text-emerald-400">{r.ok}</span>
                            <span className="mx-0.5 text-slate-300 dark:text-zinc-600">/</span>
                            <span className={r.errors > 0 ? 'text-rose-500' : 'text-slate-400'}>{r.errors}</span>
                          </td>
                          <td className="py-2 pr-3 tabular-nums text-slate-600 dark:text-zinc-300">
                            {rate === '—' ? '—' : `${rate}%`}
                          </td>
                          <td className="py-2 pr-3">
                            <div className="tabular-nums text-slate-600 dark:text-zinc-300">{fmtTokens(total)}</div>
                            <div className="text-[10px] tabular-nums text-slate-400">
                              {fmtTokens(r.prompt)} / {fmtTokens(r.completion)}
                            </div>
                          </td>
                          <td className="py-2">
                            {r.models.length === 0 ? (
                              <span className="text-xs text-slate-400">
                                {r.requests > 0 ? '无模型明细' : '—'}
                              </span>
                            ) : (
                              <button
                                type="button"
                                className="flex items-center gap-1 text-left"
                                onClick={() => toggleKeyRow(r.id)}
                                title="展开该 Key 的模型明细（请求数 / Token / 占比）"
                              >
                                {open ? (
                                  <ChevronDown size={13} className="shrink-0 text-slate-400" />
                                ) : (
                                  <ChevronRight size={13} className="shrink-0 text-slate-400" />
                                )}
                                <span className="text-xs text-slate-600 dark:text-zinc-300">
                                  {r.models.length} 个模型
                                </span>
                                <span className="max-w-[10rem] truncate text-[11px] text-slate-400">
                                  · {r.models[0].model} {r.models[0].requests} 次
                                </span>
                              </button>
                            )}
                          </td>
                        </tr>
                        {open && (
                          <tr className="border-b border-slate-100 bg-slate-50/70 dark:border-zinc-800 dark:bg-zinc-800/30">
                            <td colSpan={6} className="px-2 py-3">
                              <div className="mb-2 flex flex-wrap items-center gap-2 text-xs text-slate-500 dark:text-zinc-400">
                                <span className="font-medium">{r.name}</span>
                                <span className="text-slate-400">
                                  的模型明细 · 近 {days} 天 · {FILTER_LABELS[filter]}
                                </span>
                              </div>
                              <table className="w-full text-xs">
                                <thead>
                                  <tr className="text-left text-[11px] text-slate-400">
                                    <th className="pb-1.5 pr-3 font-medium">模型</th>
                                    <th className="pb-1.5 pr-3 font-medium">请求数</th>
                                    <th className="pb-1.5 pr-3 font-medium">成功 / 失败</th>
                                    <th className="pb-1.5 pr-3 font-medium">输入 token</th>
                                    <th className="pb-1.5 pr-3 font-medium">输出 token</th>
                                    <th className="pb-1.5 font-medium">在该 Key 内占比</th>
                                  </tr>
                                </thead>
                                <tbody>
                                  {r.models.map((m) => {
                                    const mp = r.requests > 0 ? (m.requests / r.requests) * 100 : 0;
                                    return (
                                      <tr key={m.model} className="border-t border-slate-200/60 dark:border-zinc-700/60">
                                        <td className="max-w-[16rem] truncate py-1.5 pr-3 font-mono text-[11px] text-slate-700 dark:text-zinc-200" title={m.model}>
                                          {m.model}
                                        </td>
                                        <td className="py-1.5 pr-3 tabular-nums text-slate-600 dark:text-zinc-300">
                                          {m.requests}
                                        </td>
                                        <td className="py-1.5 pr-3 tabular-nums">
                                          <span className="text-emerald-600 dark:text-emerald-400">{m.ok}</span>
                                          <span className="mx-0.5 text-slate-300 dark:text-zinc-600">/</span>
                                          <span className={m.errors > 0 ? 'text-rose-500' : 'text-slate-400'}>
                                            {m.errors}
                                          </span>
                                        </td>
                                        <td className="py-1.5 pr-3 tabular-nums text-slate-600 dark:text-zinc-300">
                                          {fmtTokens(m.prompt)}
                                        </td>
                                        <td className="py-1.5 pr-3 tabular-nums text-slate-600 dark:text-zinc-300">
                                          {fmtTokens(m.completion)}
                                        </td>
                                        <td className="w-32 py-1.5">
                                          <div className="flex items-center gap-2">
                                            <ShareBar pct={mp} tone="bg-sky-500" />
                                            <span className="w-10 shrink-0 text-right text-[10px] tabular-nums text-slate-400">
                                              {mp.toFixed(1)}%
                                            </span>
                                          </div>
                                        </td>
                                      </tr>
                                    );
                                  })}
                                </tbody>
                              </table>
                            </td>
                          </tr>
                        )}
                      </Fragment>
                    );
                  })}
                </tbody>
              </table>
            </div>
            {missingModelDetail && (
              <p className="mt-2 text-[11px] text-slate-400">
                部分 Key 仅有请求计数、没有模型明细：Key × 模型 交叉统计自本版本起记录，更早的历史数据无法回溯补算。
              </p>
            )}
          </>
        )}
      </SectionCard>
    </div>
  );
}
