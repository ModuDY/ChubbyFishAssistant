import type { StateCreator } from 'zustand';
import { api } from '../lib/tauri';
import type { AppState } from '../store';
import type { WorkbenchLayout } from '../types';
import type { WorkbenchSlice } from './workbenchTypes';

export const createWorkbenchSlice: StateCreator<AppState, [], [], WorkbenchSlice> = (set, get) => ({
  workbenchLayout: null,
  workbenchLayoutError: null,

  refreshWorkbenchLayout: async () => {
    try {
      const layout = await api.workbench.layoutGet();
      set({ workbenchLayout: layout, workbenchLayoutError: null });
    } catch (e) {
      // 布局读失败不阻断工作台渲染（页面按默认顺序展示）；
      // 但要记录下来，避免用户改了布局"看起来成功、实际没存"。
      set({ workbenchLayoutError: String(e) });
    }
  },

  saveWorkbenchLayout: async (layout: WorkbenchLayout) => {
    // 乐观更新：先改本地再落库，拖拽/隐藏操作无延迟感
    const prev = get().workbenchLayout;
    set({ workbenchLayout: layout });
    try {
      await api.workbench.layoutSet(layout);
      set({ workbenchLayoutError: null });
    } catch (e) {
      // 落库失败回滚，否则下次启动布局悄悄丢失
      set({ workbenchLayout: prev, workbenchLayoutError: String(e) });
      get().pushToast('error', `工作台布局保存失败：${String(e)}`);
    }
  },

  resetWorkbenchLayout: async () => {
    try {
      const layout = await api.workbench.layoutReset();
      set({ workbenchLayout: layout, workbenchLayoutError: null });
      get().pushToast('success', '布局已恢复默认');
    } catch (e) {
      get().pushToast('error', `恢复默认布局失败：${String(e)}`);
    }
  },
});
