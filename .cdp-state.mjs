// 读取打包版的外观状态与开屏形态（临时脚本）
const list = await (await fetch('http://127.0.0.1:9223/json/list')).json();
const page = list.find((t) => t.type === 'page' && !t.url.includes('pet=1'));
console.log('URL:', page?.url);
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
ws.addEventListener('message', (ev) => {
  const m = JSON.parse(ev.data);
  if (m.id && pending.has(m.id)) {
    pending.get(m.id)(m);
    pending.delete(m.id);
  }
});
const send = (method, params = {}) =>
  new Promise((r) => {
    const mid = ++id;
    pending.set(mid, r);
    ws.send(JSON.stringify({ id: mid, method, params }));
  });
const evalJs = async (e) => {
  const r = await send('Runtime.evaluate', { expression: e, returnByValue: true, awaitPromise: true });
  if (r.result?.exceptionDetails) return 'EX: ' + JSON.stringify(r.result.exceptionDetails).slice(0, 200);
  return r.result?.result?.value;
};
await new Promise((r) => ws.addEventListener('open', r, { once: true }));

console.log('cf-appearance-v2:', await evalJs(`localStorage.getItem('cf-appearance-v2')`));
console.log('cf-appearance-v1:', await evalJs(`localStorage.getItem('cf-appearance-v1')`));
console.log(
  '当前状态:',
  await evalJs(`JSON.stringify({
    kind: document.querySelector('.wp-layer')?.dataset.kind,
    scene: document.querySelector('.wp-layer')?.dataset.scene,
    video: !!document.querySelector('video.wp-video'),
    html: document.documentElement.dataset,
  }, null, 1)`),
);
ws.close();
