import type { WorkbenchLayout } from '../types';

/** 工作台 slice（P0）：只持有布局状态，不持有待办数据。
 *
 * 待办由 `lib/workbenchTodos.ts` 的纯函数从既有 store 数据实时推导，
 * 放进 store 会造成"数据一份、推导结果又一份"的双写不一致（改了账号忘刷待办）。
 */
export interface WorkbenchSlice {
  /** 布局：null = 尚未加载（首屏用注册表默认顺序渲染，避免闪空） */
  workbenchLayout: WorkbenchLayout | null;
  /** 布局加载失败标记：true 时页面提示"布局未保存，本次改动不持久化" */
  workbenchLayoutError: string | null;
  refreshWorkbenchLayout: () => Promise<void>;
  saveWorkbenchLayout: (layout: WorkbenchLayout) => Promise<void>;
  resetWorkbenchLayout: () => Promise<void>;
}
