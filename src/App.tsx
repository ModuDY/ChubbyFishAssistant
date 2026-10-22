import { useEffect } from 'react';
import TitleBar from './components/TitleBar';
import Sidebar from './components/Sidebar';
import TopBar from './components/TopBar';
import Toaster from './components/Toaster';
import StorageHealthBanner from './components/StorageHealthBanner';
import WallpaperLayer from './components/WallpaperLayer';
import EntrySplash from './components/EntrySplash';
import EffectsHost from './components/EffectsHost';
import { useAppStore } from './store';
import { resolveTheme } from './lib/themes';
import Workbench from './pages/workbench/Workbench';
import Dashboard from './pages/Dashboard';
import Accounts from './pages/Accounts';
import Checkin from './pages/Checkin';
import Logs from './pages/Logs';
import Settings from './pages/Settings';
import ApiService from './pages/ApiService';
import DoubaoOverview from './pages/doubao/DoubaoOverview';
import DoubaoAccounts from './pages/doubao/DoubaoAccounts';
import DoubaoSettings from './pages/doubao/DoubaoSettings';
import BuddyOverview from './pages/buddy/BuddyOverview';
import BuddyAccounts from './pages/buddy/BuddyAccounts';
import BuddyCheckin from './pages/buddy/BuddyCheckin';
// 积分看板（credits-dashboard-plan.md；平台拆分）：同一组件按 platform 渲染
// Trae 页（credits 视图）、Buddy 页（buddy-credits 视图）、Qoder 页（qoder-credits 视图，替换旧 QoderCredits.tsx）
import CreditsDashboard from './pages/dashboard/Dashboard';
import BuddyApiService from './pages/buddy/BuddyApiService';
import BuddySettings from './pages/buddy/BuddySettings';
import QoderOverview from './pages/qoder/QoderOverview';
import QoderAccounts from './pages/qoder/QoderAccounts';
import QoderCheckin from './pages/qoder/QoderCheckin';
import QoderApiService from './pages/qoder/QoderApiService';
import QoderSettings from './pages/qoder/QoderSettings';
import ApiManagerModal from './components/api/ApiManagerModal';
import TraeWorkMigrationGuide from './components/TraeWorkMigrationGuide';

function renderView(view: string) {
  switch (view) {
    // 工作台：跨应用固定入口（默认首屏）。注意本 switch 的 default 回落
    // Dashboard，新增 ViewKey 忘了加 case 会**静默显示概览页**而不是报错。
    case 'workbench':
      return <Workbench />;
    case 'dashboard':
      return <Dashboard />;
    case 'accounts':
      return <Accounts />;
    case 'checkin':
      return <Checkin />;
    case 'credits':
      // key 强制按平台重挂载：两视图共用同一组件类型，缺 key 时 React 复用实例仅更新 props，
      // 挂载数据加载 useEffect 不重跑，切换平台后另一平台数据全空
      return <CreditsDashboard key="trae" platform="trae" />;
    case 'logs':
      return <Logs />;
    case 'api-service':
      return <ApiService />;
    case 'settings':
      return <Settings />;
    case 'doubao-overview':
      return <DoubaoOverview />;
    case 'doubao-accounts':
      return <DoubaoAccounts />;
    case 'doubao-settings':
      return <DoubaoSettings />;
    case 'buddy-overview':
      return <BuddyOverview />;
    case 'buddy-accounts':
      return <BuddyAccounts />;
    case 'buddy-checkin':
      return <BuddyCheckin />;
    case 'buddy-credits':
      return <CreditsDashboard key="buddy" platform="buddy" />;
    case 'buddy-api-service':
      return <BuddyApiService />;
    case 'buddy-settings':
      return <BuddySettings />;
    case 'qoder-overview':
      return <QoderOverview />;
    case 'qoder-accounts':
      return <QoderAccounts />;
    case 'qoder-checkin':
      return <QoderCheckin />;
    case 'qoder-credits':
      return <CreditsDashboard key="qoder" platform="qoder" />;
    case 'qoder-api-service':
      return <QoderApiService key="qoder-api-service" />;
    case 'qoder-settings':
      return <QoderSettings />;
    default:
      return <Dashboard />;
  }
}

export default function App() {
  const view = useAppStore((s) => s.view);
  const setView = useAppStore((s) => s.setView);
  const init = useAppStore((s) => s.init);
  const ready = useAppStore((s) => s.ready);
  const initError = useAppStore((s) => s.initError);
  const settings = useAppStore((s) => s.settings);

  useEffect(() => {
    void init().catch((err) => {
      console.error('初始化失败:', err);
    });
  }, [init]);

  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const apply = () => {
      const theme = useAppStore.getState().settings?.theme;
      const t = resolveTheme(theme, mq.matches);
      document.documentElement.classList.toggle('dark', t.dark);
      document.documentElement.dataset.theme = t.id;
    };
    apply();
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [settings?.theme]);

  // 壁纸层必须挂在最外层且始终在场（关闭时组件自身返回 null），
  // 这样登录/加载态也已经能看到场景，不会"先白一下再变壁纸"
  const shell = !ready ? (
    <div className="app-shell flex h-full items-center justify-center bg-slate-100 dark:bg-zinc-950">
      {/* boot-veil：视频壁纸开屏时隐藏这枚转圈，避免和底部进度提示重复（见 index.css） */}
      <div className="boot-veil flex flex-col items-center gap-3">
        {initError ? (
          <>
            <p role="alert" className="max-w-lg break-words px-4 text-sm text-red-600">初始化失败：{initError}</p>
            <button className="btn-primary" onClick={() => void init().catch(() => {})}>重试</button>
          </>
        ) : (
          <>
            <div className="h-8 w-8 animate-spin rounded-full border-3 border-zinc-400 border-t-transparent" />
            <span className="text-sm text-slate-500">正在加载…</span>
          </>
        )}
      </div>
    </div>
  ) : (
    <div className="app-shell flex h-full flex-col bg-slate-100 text-slate-800 dark:bg-zinc-950 dark:text-zinc-100">
      <TitleBar />
      <StorageHealthBanner />
      <div className="flex min-h-0 flex-1">
        <Sidebar view={view} onNav={setView} />
        <main className="relative flex min-w-0 flex-1 flex-col">
          <TopBar />
          {/* app-content：壁纸开启时由样式表在此铺一层轻纱，保证正文对比度 */}
          <div className="app-content relative min-h-0 flex-1 overflow-auto p-5">{renderView(view)}</div>
        </main>
      </div>
      <Toaster />
      {/* 全局 API 管理弹窗（任意 activeApp 视图均可打开，unified-api-gateway-design §5.2） */}
      <ApiManagerModal />
      {/* Trae 双应用合并升级引导（首启有迁移报告时弹出，关闭落标记后不再展示） */}
      <TraeWorkMigrationGuide />
    </div>
  );

  return (
    <>
      <WallpaperLayer />
      {shell}
      <EffectsHost />
      <EntrySplash ready={ready} />
    </>
  );
}
