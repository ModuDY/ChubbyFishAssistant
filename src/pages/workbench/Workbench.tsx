import { useEffect, useMemo, useState } from 'react';
import { RotateCcw, Settings2, Eye, EyeOff } from 'lucide-react';
import PageHeader from '../../components/PageHeader';
import { useAppStore } from '../../store';
import { cn } from '../../lib/cn';
import type { WorkbenchWidgetKey } from '../../types';
import { WIDGETS, WIDGET_MAP, SPAN_CLASS } from './widgets/registry';
import { useTodosEmpty } from './widgets/TaskTodosWidget';

/**
 * 工作台（P0）：跨应用固定入口，默认首屏。
 *
 * 三层结构：
 * 1. `registry.tsx` —— 区块注册表（key / 标题 / 组件 / 默认占列）
 * 2. `settings.workbench_layout`（kv `workbench_layout`）—— 用户调过的顺序与显隐
 * 3. 本文件 —— 按"布局 ∪ 注册表"求并集渲染
 *
 * 渲染取并集而非直接用布局文档：布局可能来自旧版本（少了新区块）
 * 或超前版本（多了不认识的 key），以注册表为真值、布局只覆盖 span/hidden。
 */
export default function Workbench() {
  const layout = useAppStore((s) => s.workbenchLayout);
  const layoutError = useAppStore((s) => s.workbenchLayoutError);
  const refreshWorkbenchLayout = useAppStore((s) => s.refreshWorkbenchLayout);
  const saveWorkbenchLayout = useAppStore((s) => s.saveWorkbenchLayout);
  const resetWorkbenchLayout = useAppStore((s) => s.resetWorkbenchLayout);

  const [editing, setEditing] = useState(false);
  const todosEmpty = useTodosEmpty();

  useEffect(() => {
    void refreshWorkbenchLayout();
  }, [refreshWorkbenchLayout]);

  /**
   * 求并集后的渲染顺序：
   * - 以布局文档顺序为主（用户拖过的顺序要尊重）
   * - 追加注册表里存在、但布局文档没有的区块（版本升级新增）
   */
  const orderedKeys = useMemo<WorkbenchWidgetKey[]>(() => {
    const fromLayout = (layout?.widgets ?? []).map((w) => w.key);
    const known = fromLayout.filter((k) => WIDGET_MAP.has(k));
    const missing = WIDGETS.map((w) => w.key).filter((k) => !known.includes(k));
    return [...known, ...missing];
  }, [layout]);

  /** key → 布局项（span / hidden），缺失时取注册表默认 */
  const layoutOf = (key: WorkbenchWidgetKey) =>
    layout?.widgets.find((w) => w.key === key) ?? {
      key,
      span: WIDGET_MAP.get(key)?.defaultSpan ?? 1,
      hidden: false,
    };

  /** 切换某区块显隐（编辑态下可用） */
  const toggleHidden = (key: WorkbenchWidgetKey) => {
    const cur = layoutOf(key);
    const widgets = orderedKeys.map((k) => {
      const w = layoutOf(k);
      return k === key ? { ...w, hidden: !cur.hidden } : w;
    });
    void saveWorkbenchLayout({ version: layout?.version ?? 1, widgets });
  };

  /** 移动区块（与前一项交换），用 ▲▼ 而非拖拽：单人自用，拖拽实现成本高且易误触 */
  const move = (key: WorkbenchWidgetKey, dir: -1 | 1) => {
    const idx = orderedKeys.indexOf(key);
    const next = idx + dir;
    if (idx < 0 || next < 0 || next >= orderedKeys.length) return;
    const keys = [...orderedKeys];
    [keys[idx], keys[next]] = [keys[next], keys[idx]];
    const widgets = keys.map((k) => layoutOf(k));
    void saveWorkbenchLayout({ version: layout?.version ?? 1, widgets });
  };

  /** 实际渲染的区块：跳过 hidden，且待办为空时不渲染待办卡 */
  const visible = orderedKeys.filter((key) => {
    if (layoutOf(key).hidden) return false;
    if (key === 'todos' && todosEmpty) return false;
    return true;
  });

  return (
    <div className="animate-fade-in">
      <PageHeader
        title="工作台"
        desc="今天要做什么 · 一键直达 · 三秒捕获"
        actions={
          <>
            {layoutError && (
              <span className="text-xs text-rose-500" title={layoutError}>
                布局未持久化
              </span>
            )}
            <button
              onClick={() => setEditing((v) => !v)}
              className={cn('btn-outline', editing && '!bg-zinc-100 dark:!bg-zinc-800')}
            >
              <Settings2 size={15} /> {editing ? '完成' : '调整布局'}
            </button>
            <button
              onClick={() => {
                void resetWorkbenchLayout();
                setEditing(false);
              }}
              className="btn-ghost"
              title="恢复默认布局"
            >
              <RotateCcw size={15} />
            </button>
          </>
        }
      />

      <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
        {visible.map((key) => {
          const def = WIDGET_MAP.get(key);
          if (!def) return null;
          const { Component } = def;
          const Icon = def.icon;
          const span = layoutOf(key).span;
          return (
            <section
              key={key}
              className={cn('card p-4', SPAN_CLASS[span])}
              aria-label={def.title}
            >
              <div className="mb-3 flex items-center gap-2">
                <Icon size={15} className="text-slate-400 dark:text-zinc-500" />
                <h2 className="text-sm font-semibold text-slate-800 dark:text-zinc-100">
                  {def.title}
                </h2>
                {def.hint && (
                  <span className="truncate text-[11px] text-slate-400 dark:text-zinc-500">
                    {def.hint}
                  </span>
                )}
                {editing && (
                  <div className="ml-auto flex items-center gap-1">
                    <button
                      onClick={() => move(key, -1)}
                      className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-600 dark:hover:bg-zinc-800"
                      title="上移"
                    >
                      ▲
                    </button>
                    <button
                      onClick={() => move(key, 1)}
                      className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-600 dark:hover:bg-zinc-800"
                      title="下移"
                    >
                      ▼
                    </button>
                    <button
                      onClick={() => toggleHidden(key)}
                      className="rounded p-1 text-slate-400 hover:bg-slate-100 hover:text-slate-600 dark:hover:bg-zinc-800"
                      title="隐藏"
                    >
                      <EyeOff size={13} />
                    </button>
                  </div>
                )}
              </div>
              <Component />
            </section>
          );
        })}
      </div>

      {/* 编辑态：显示被隐藏的区块，否则用户无法恢复 */}
      {editing && orderedKeys.some((k) => layoutOf(k).hidden) && (
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <span className="text-xs text-slate-400 dark:text-zinc-500">已隐藏：</span>
          {orderedKeys
            .filter((k) => layoutOf(k).hidden)
            .map((k) => (
              <button
                key={k}
                onClick={() => toggleHidden(k)}
                className="chip bg-slate-100 text-slate-600 hover:bg-slate-200 dark:bg-zinc-800 dark:text-zinc-300"
              >
                <Eye size={12} /> {WIDGET_MAP.get(k)?.title ?? k}
              </button>
            ))}
        </div>
      )}
    </div>
  );
}
