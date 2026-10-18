import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import ErrorBoundary from './components/ErrorBoundary';
import './index.css';

/**
 * 窗口分流：桌宠窗口复用同一份 index.html，靠查询参数分流。
 * 这样**不需要新增 Vite 构建入口**（vite.config.ts 未配多入口，新增会牵动
 * beforeBuildCommand / frontendDist）。
 *
 * - `?pet=1`            → 正式桌宠页（PetRoot）
 * - `?pet=1&probe=1`    → 渲染路线诊断页（ProbeRoot，排查 alpha / 性能用）
 *
 * 用动态 import 而非顶层 import：主窗口不会把桌宠代码打进首屏包。
 */
const params = new URLSearchParams(window.location.search);
const isPetWindow = params.get('pet') === '1';
const isProbe = params.get('probe') === '1';

const root = ReactDOM.createRoot(document.getElementById('root') as HTMLElement);

const renderStandalone = (Root: React.ComponentType) => {
  root.render(
    <React.StrictMode>
      <Root />
    </React.StrictMode>,
  );
};

if (isPetWindow) {
  if (isProbe) {
    void import('./pet/ProbeRoot').then((m) => renderStandalone(m.default));
  } else {
    void import('./pet/PetRoot').then((m) => renderStandalone(m.default));
  }
} else {
  root.render(
    <React.StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </React.StrictMode>,
  );
}
