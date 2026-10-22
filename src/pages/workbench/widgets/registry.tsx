import { type ComponentType } from 'react';
import {
  Activity,
  ListChecks,
  Zap,
  Clock,
  PenLine,
} from 'lucide-react';
import type { WorkbenchWidgetKey } from '../../../types';
import TaskStatusWidget from './TaskStatusWidget';
import TaskTodosWidget from './TaskTodosWidget';
import TaskActionsWidget from './TaskActionsWidget';
import TaskRecentWidget from './TaskRecentWidget';
import TaskCaptureWidget from './TaskCaptureWidget';

/** widget 注册项：工作台只按 key 渲染，不关心组件从哪来 */
export interface WidgetDef {
  key: WorkbenchWidgetKey;
  /** 区块标题 */
  title: string;
  /** 标题右侧的说明（一句话讲清这个区块干什么） */
  hint?: string;
  icon: ComponentType<{ size?: number | string; className?: string }>;
  Component: ComponentType;
  /** 默认占列数（1~3），布局文档缺失时用它 */
  defaultSpan: 1 | 2 | 3;
}

/**
 * 区块注册表：顺序即默认优先级（越靠前越"每天必看"）。
 *
 * 增删区块只改这里 + `types.ts::WorkbenchWidgetKey` 白名单 +
 * Rust 侧 `commands/workbench.rs::WIDGET_KEYS`（三处必须同步，
 * 否则老布局文档里会出现前端没有的 key）。
 */
export const WIDGETS: WidgetDef[] = [
  {
    key: 'status',
    title: '今日状态',
    hint: '签到进度 / 网关 / 告警',
    icon: Activity,
    Component: TaskStatusWidget,
    defaultSpan: 3,
  },
  {
    key: 'todos',
    title: '待办',
    hint: '只列今天确实要动手的事',
    icon: ListChecks,
    Component: TaskTodosWidget,
    defaultSpan: 2,
  },
  {
    key: 'actions',
    title: '一键动作',
    hint: '常用操作直达',
    icon: Zap,
    Component: TaskActionsWidget,
    defaultSpan: 1,
  },
  {
    key: 'recent',
    title: '最近',
    hint: '最近打开与常用',
    icon: Clock,
    Component: TaskRecentWidget,
    defaultSpan: 2,
  },
  {
    key: 'capture',
    title: '快速捕获',
    hint: '? 问知识库 · / 走命令 · 其余存笔记',
    icon: PenLine,
    Component: TaskCaptureWidget,
    defaultSpan: 1,
  },
];

/** key → 注册项（渲染 O(1) 查找） */
export const WIDGET_MAP = new Map(WIDGETS.map((w) => [w.key, w]));

/**
 * 栅格列数映射。Tailwind JIT 只扫描源码里的**字面量类名**，
 * 因此不能用 `col-span-${span}` 拼接——那样构建后类名不存在，宽度全失效。
 */
export const SPAN_CLASS: Record<1 | 2 | 3, string> = {
  1: 'lg:col-span-1',
  2: 'lg:col-span-2',
  3: 'lg:col-span-3',
};
