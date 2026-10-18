import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import ErrorBoundary from './components/ErrorBoundary';
import './index.css';

/**
 * 窗口分流：pet 窗口复用同一份 index.html，靠 `?pet=1` 分流到验证页。
 * 这样**不需要新增 Vite 构建入口**（vite.config.ts 未配多入口，新增会牵动
 * beforeBuildCommand / frontendDist）。
 *
 * 用动态 import 而非顶层 import：主窗口不会把验证页代码打进首屏包。
 */
const isPetWindow = new URLSearchParams(window.location.search).get('pet') === '1';

if (isPetWindow) {
  void import('./pet/ProbeRoot').then(({ default: ProbeRoot }) => {
    ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
      <React.StrictMode>
        <ProbeRoot />
      </React.StrictMode>,
    );
  });
} else {
  ReactDOM.createRoot(document.getElementById('root') as HTMLElement).render(
    <React.StrictMode>
      <ErrorBoundary>
        <App />
      </ErrorBoundary>
    </React.StrictMode>,
  );
}
