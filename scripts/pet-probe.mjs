// 桌宠渲染路线验证：一键以 pet 窗口覆盖配置启动 Tauri dev。
//
// 为什么不直接加到 package.json：验证完成后这条脚本与 tauri.pet.conf.json 会一起删除，
// 保持 package.json 干净；且它复用 dev-tauri.mjs 的随机端口逻辑，避免端口冲突。
//
// 用法：node scripts/pet-probe.mjs
import net from 'node:net';
import fs from 'node:fs';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const require = createRequire(import.meta.url);
const tauriCliJs = require.resolve('@tauri-apps/cli/tauri.js');

const devConfRel = 'tauri.dev.conf.json';
const petConfRel = 'tauri.pet.conf.json';

if (!fs.existsSync(path.join(root, petConfRel))) {
  console.error(`[pet-probe] 找不到 ${petConfRel}，无法启动验证窗口。`);
  process.exit(1);
}

function isPortFree(port) {
  return new Promise((resolve) => {
    const srv = net.createServer();
    srv.once('error', () => resolve(false));
    srv.once('listening', () => srv.close(() => resolve(true)));
    srv.listen(port, '127.0.0.1');
  });
}

async function pickPort() {
  for (let i = 0; i < 300; i++) {
    const p = 5000 + Math.floor(Math.random() * 1001);
    // eslint-disable-next-line no-await-in-loop
    if (await isPortFree(p)) return p;
  }
  throw new Error('5000-6000 范围内未找到空闲端口');
}

const port = await pickPort();
fs.writeFileSync(
  path.join(root, devConfRel),
  JSON.stringify({ build: { devUrl: `http://localhost:${port}` } }, null, 2),
);

console.log(`[pet-probe] 使用随机空闲端口 ${port}`);
console.log('[pet-probe] 将同时创建 main 与 pet 两个窗口（pet 为透明置顶验证窗）');

const child = spawn(
  process.execPath,
  [tauriCliJs, 'dev', '--config', devConfRel, '--config', petConfRel],
  {
    stdio: 'inherit',
    cwd: root,
    env: { ...process.env, VITE_PORT: String(port) },
  },
);

child.on('exit', (code, signal) => process.exit(code ?? (signal ? 1 : 0)));
child.on('error', (e) => {
  console.error('[pet-probe] 执行 tauri 失败:', e);
  process.exit(1);
});
