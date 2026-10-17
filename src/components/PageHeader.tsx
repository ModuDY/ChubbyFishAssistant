import { type ReactNode } from 'react';

export default function PageHeader({
  title,
  desc,
  actions,
  leftExtra,
  stackActions = false,
}: {
  title: string;
  desc?: string;
  actions?: ReactNode;
  /** 左侧区域附加元素（紧跟在标题/描述文字后面） */
  leftExtra?: ReactNode;
  /** 操作按钮较多时：整体换到标题下方独立一行，避免挤在标题右侧被压成多行 */
  stackActions?: boolean;
}) {
  return (
    <div className="mb-5 flex flex-wrap items-end justify-between gap-x-4 gap-y-3">
      <div className="flex shrink-0 items-center gap-3">
        <div className="h-7 w-1 rounded-full bg-zinc-800 dark:bg-zinc-200" />
        <div>
          <h1 className="text-xl font-semibold tracking-tight text-slate-800 dark:text-zinc-100">{title}</h1>
          {(desc || leftExtra) && (
            <div className="mt-1 flex items-center gap-3">
              {desc && <p className="text-sm text-slate-500 dark:text-zinc-400">{desc}</p>}
              {leftExtra}
            </div>
          )}
        </div>
      </div>
      {actions && (
        <div
          className={
            stackActions
              ? 'flex basis-full flex-wrap items-center justify-end gap-2'
              : 'flex min-w-0 flex-1 flex-wrap items-center justify-end gap-2'
          }
        >
          {actions}
        </div>
      )}
    </div>
  );
}
