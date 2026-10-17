import type { StateCreator } from 'zustand';
import { api, setupListeners, type ProfileDoneEvent, type SaveLoginDoneEvent } from '../lib/tauri';
import type { AppState } from '../store';
import type { StartupSlice } from './startupTypes';

export const createStartupSlice: StateCreator<AppState, [], [], StartupSlice> = (set, get) => {
  let initialization: Promise<void> | null = null;
  let unsubs: Array<() => void> = [];

  const initialize = async () => {
    set({ initError: null });
    unsubs = await setupListeners({
      onProxyLog: (line) =>
        set((s) => ({ proxyLog: [line, ...s.proxyLog.slice(0, 199)] })),
      onAccountCaptured: (uid) => {
        // 事件驱动累加捕获数（后端 Arc<AtomicI64> 的实时镜像，避免轮询）
        set((s) => ({ proxy: { ...s.proxy, captured: s.proxy.captured + 1 } }));
        get().pushToast('success', `已捕获账号 ${uid}`);
        void get().refreshAccounts();
      },
      onCheckinProgress: (e) => get().applyCheckinEvent(e),
      onQoderCheckinProgress: (line) => get().applyQoderCheckinLine(line),
      onSwitchProgress: (line) =>
        set((s) => ({ switchProgress: [...s.switchProgress.slice(-49), line] })),
      // D2：订阅后端 switch-done，给用户明确的切换完成/失败信号
      onSwitchDone: (e) => {
        // 提取脚本 [fatal] 行的原文（如「目标账号 xxx 无快照，请先登录该账号并点击保存当前登录态」）
        // raw 兜底空串：后端偶发缺 payload 时避免 TypeError 把 switchingTo 永久锁死
        const raw = e.raw ?? '';
        const reason = e.success ? null : (raw.match(/\[fatal\]\s*(.+)$/)?.[1]?.trim() ?? null);
        // verify 超时（authfile 布局）：切换动作完成但登录身份未确认 → warn 而非 success，
        // 避免「切换成功」toast 掩盖客户端未登录的事实（switcher.log 实测 4/4 超时）
        const warnUnconfirmed = e.success && /未确认登录身份/.test(raw);
        const doneLine = warnUnconfirmed
          ? '[警告] 已切换，但 30 秒内未确认登录身份，请打开客户端核实'
          : e.success
            ? '[完成] 登录态切换成功'
            : `[失败] ${reason ?? '登录态切换未完成，请查看日志'}`;
        set((s) => ({
          switchingTo: null,
          switchProgress: [...s.switchProgress.slice(-49), doneLine],
        }));
        get().pushToast(
          warnUnconfirmed ? 'warn' : e.success ? 'success' : 'error',
          warnUnconfirmed
            ? '已执行切换，但 30 秒内未确认登录身份——请打开客户端核实；若未登录，请重新登录后「保存当前登录态」'
            : e.success
              ? '登录态切换完成'
              : `切换失败：${reason ?? '请查看系统日志'}`,
        );
        void get().refreshAccounts();
        void get().refreshProxy();
        // 登录中徽标（localEntitlement）刷新：切换完成后本机登录 uid 已变，
        // 立即刷新一次；客户端启动写入本机使用证据需数秒，延迟再刷一次兜底
        void get().refreshLocalEntitlement();
        setTimeout(() => void get().refreshLocalEntitlement(), 8000);
      },
      onSaveLoginProgress: (line) =>
        set((s) => ({ saveLoginProgress: [...s.saveLoginProgress.slice(-49), line] })),
      onSaveLoginDone: (e: SaveLoginDoneEvent) => {
        // 同上：raw 缺失时兜底空串，避免 savingLogin 被锁死
        const reason = e.success ? null : ((e.raw ?? '').match(/\[fatal\]\s*(.+)$/)?.[1]?.trim() ?? null);
        set((s) => ({
          savingLogin: null,
          saveLoginProgress: [
            ...s.saveLoginProgress.slice(-49),
            e.success ? '[完成] 登录态保存成功' : `[失败] ${reason ?? '登录态保存失败，请查看日志'}`,
          ],
        }));
        get().pushToast(
          e.success ? 'success' : 'error',
          e.success ? '登录态已保存，可随时切换回此账号' : `保存失败：${reason ?? '请查看系统日志'}`,
        );
        void get().refreshProfiles();
      },
      onDeviceResetProgress: (line) =>
        set((s) => ({ deviceResetProgress: [...s.deviceResetProgress.slice(-99), line] })),
      onDeviceResetDone: (e) => {
        set((s) => ({
          deviceResetActive: false,
          deviceResetProgress: [
            ...s.deviceResetProgress.slice(-99),
            e.success ? '[完成] 6 层设备标识重置成功' : '[失败] 设备标识重置未完成，请查看日志',
          ],
        }));
        get().pushToast(
          e.success ? 'success' : 'error',
          e.success ? '6 层设备标识重置完成' : '设备标识重置失败，请查看日志',
        );
      },
      onProfileProgress: (line) =>
        set((s) => ({ profileProgress: [...s.profileProgress.slice(-49), line] })),
      onProfileDone: (e: ProfileDoneEvent) => {
        set((s) => ({
          profileActive: false,
          profileProgress: [
            ...s.profileProgress.slice(-49),
            e.success ? `[完成] ${e.action === 'backup' ? '备份' : '恢复'}成功` : `[失败] ${e.action === 'backup' ? '备份' : '恢复'}失败`,
          ],
        }));
        get().pushToast(
          e.success ? 'success' : 'error',
          e.success
            ? `登录态${e.action === 'backup' ? '备份' : '恢复'}完成`
            : `登录态${e.action === 'backup' ? '备份' : '恢复'}失败`,
        );
        void get().refreshProfiles();
      },
    });
    const results = await Promise.allSettled([
      get().refreshStorageHealth(),
      get().refreshEnv(),
      get().refreshCert(),
      get().refreshProxy(),
      get().refreshApiStatus(),
      get().refreshAccounts(),
      get().refreshGroups(),
      get().refreshSettings(),
      get().refreshCreditsHistory(),
      get().refreshCreditsDaily(),
      get().refreshProfiles(),
      get().refreshLocalEntitlement(),
    ]);
    const failed = results.find((result) => result.status === 'rejected');
    if (failed?.status === 'rejected') throw failed.reason;
    set({ ready: true });

    // 启动时根据设置自动开启代理
    const s = get();
    if (!s.proxy.running && s.settings?.auto_start_proxy) {
      void s.startProxy();
    }
    // 启动时根据设置自动开启 API 网关（复刻 auto_start_proxy 模式）：
    // Rust 侧 do_start 对已运行实例返回 Err，此处判重 + 静默兜底，避免与托盘启停竞态时误报
    if (!s.apiStatus?.running && s.settings?.auto_start_api) {
      try {
        const status = await api.apiServer.start();
        set({ apiStatus: status });
      } catch {
        /* 静默失败：状态栏保持未启动，用户可手动启动 */
      }
    }
    // 待取启动通知（一次性）：后端启动期消息（如分组统一迁移结果）toast 告知用户，
    // 拉取即清不会重复弹出；失败静默（通知不阻塞启动）
    try {
      const notice = await api.misc.startupNoticeTake();
      if (notice) get().pushToast('info', notice);
    } catch {
      /* 静默失败 */
    }
    // Trae 双应用合并升级引导：有迁移报告且未关闭过 → 弹窗展示迁移流程与结果；
    // 关闭时后端落「已读」标记，之后启动不再展示。失败静默（引导不阻塞启动）
    try {
      const report = await api.misc.traeworkMigrationGuide();
      if (report) set({ traeworkGuide: report, traeworkGuideOpen: true });
    } catch {
      /* 静默失败 */
    }
  };

  return {
    ready: false,
    initError: null,
    storageHealth: null,
    refreshStorageHealth: async () => {
      try {
        set({ storageHealth: await api.misc.storageHealth() });
      } catch (error) {
        set({ storageHealth: { mode: 'unavailable', message: `无法确认数据是否正常保存：${String(error)}` } });
      }
    },
    init: () => {
      if (initialization) return initialization;
      if (get().ready) return Promise.resolve();
      initialization = Promise.resolve().then(initialize).catch((error: unknown) => {
        for (const unlisten of unsubs.splice(0).reverse()) {
          try { unlisten(); } catch {}
        }
        set({ ready: false, initError: String(error) });
        throw error;
      }).finally(() => { initialization = null; });
      return initialization;
    },
  };
};
