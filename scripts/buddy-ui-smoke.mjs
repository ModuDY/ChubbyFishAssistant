import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createServer } from 'vite';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.BUDDY_PLAYWRIGHT_MODULE || 'playwright');
const failures = [];
const results = [];
const output = join(tmpdir(), 'aiwork-buddy-ui-smoke');
await mkdir(output, { recursive: true });

const harness = `
import React from 'react';
import ReactDOM from 'react-dom/client';
import '/src/index.css';
import { mockIPC, mockWindows } from '/node_modules/@tauri-apps/api/mocks.js';
import { emit } from '/node_modules/@tauri-apps/api/event.js';
const account = (suffix, balance) => ({
  id: 'wb-test-' + suffix, uid: 'synthetic-' + suffix, nickname: '测试账号 ' + suffix,
  phone_masked: '', edition_type: 'pro', access_token_expires_at: null,
  refresh_token_expires_at: null, auth_saved_at: null, needs_relogin: false,
  relogin_reason: '', group_id: '', note: '', credits_balance: balance,
  credits_fetched_at: null, credits_expire_at: null, is_current: false,
  has_credential: true, has_snapshot: true, has_snapshot_codebuddy: false,
  is_current_workbuddy: false, is_current_codebuddy: false
});
let accounts = [account('A', 37), account('B', 2600)];
let result = { ok: true, cached: false, accounts: accounts.map(item => ({
  user_id: item.id, name: item.nickname, ok: true, balance: item.credits_balance,
  source: 'synthetic', packages: [{ name: '测试积分包', code: item.id,
    remaining: item.credits_balance, total: null, used: null, end_time: null, expire_ts: null }]
})) };
let held = false;
const pending = [];
const calls = [];
const unexpected = [];
mockIPC(async (command, args) => {
  calls.push({command, args});
  switch (command) {
    case 'workbuddy_accounts_list': return structuredClone(accounts);
    case 'workbuddy_credits_fetch': {
      const snapshot = structuredClone(result);
      if (held) return new Promise(resolve => pending.push(() => resolve(snapshot)));
      return snapshot;
    }
    case 'workbuddy_groups_list':
    case 'shared_groups_list':
    case 'tags_list':
    case 'workbuddy_checkin_results': return [];
    case 'account_tags_map': return {};
    case 'workbuddy_cli_status': return { active_account_id: null, env_token_present: false };
    case 'workbuddy_chatdata_info': return { backed: false };
    case 'workbuddy_env_check':
    case 'codebuddy_env_check': return { installed: false, running: false };
    case 'workbuddy_account_remove': {
      if (!args.userId.startsWith('wb-test-')) throw new Error('non-synthetic deletion blocked');
      accounts = accounts.filter(item => item.id !== args.userId);
      result.accounts = result.accounts.filter(item => item.user_id !== args.userId);
      return null;
    }
    default: unexpected.push(command); throw new Error('unmocked command: ' + command);
  }
}, {shouldMockEvents: true});
mockWindows('main');
const { useAppStore } = await import('/src/store.ts');
useAppStore.setState({pushToast: (kind, msg) => useAppStore.setState(state => ({
  toasts: [...state.toasts, {id: crypto.randomUUID(), kind, msg}]
}))});
const { default: Accounts } = await import('/src/pages/buddy/BuddyAccounts.tsx');
const { default: Overview } = await import('/src/pages/buddy/BuddyOverview.tsx');
const { default: Toaster } = await import('/src/components/Toaster.tsx');
const root = ReactDOM.createRoot(document.getElementById('root'));
const mount = name => root.render(React.createElement(React.Fragment, {},
  React.createElement(name === 'overview' ? Overview : Accounts, {key: name}), React.createElement(Toaster)));
window.smoke = {
  calls, unexpected, mount, pending,
  hold: () => { held = true; },
  release: () => { held = false; pending.splice(0).forEach(resolve => resolve()); },
  event: async balance => {
    result = {...result, cached: false, refreshing: false};
    accounts[0].credits_balance = balance;
    result.accounts[0].balance = balance;
    result.accounts[0].packages[0].remaining = balance;
    await emit('wb-credits-updated', structuredClone(result));
  },
  partial: () => { result.accounts[1].ok = false; result.accounts[1].balance = null; },
  stale: () => { result = {...result, cached: true, refreshing: true}; },
  failed: () => { result.accounts.forEach(item => { item.ok = false; }); },
  reset: () => useAppStore.setState({toasts: []})
};
mount(new URLSearchParams(location.search).get('view') || 'accounts');
`;

const server = await createServer({
  cacheDir: join(output, 'vite-cache'),
  server: { port: 0, host: '127.0.0.1', strictPort: false, open: false, hmr: false },
  optimizeDeps: { include: ['react', 'react-dom/client'] },
  plugins: [{ name: 'buddy-smoke-only', configureServer(vite) {
    vite.middlewares.use(async (request, response, next) => {
      const path = request.url?.split('?')[0];
      if (path === '/__buddy_smoke__') {
        const html = await vite.transformIndexHtml(request.url,
          '<!doctype html><html><head><meta charset="UTF-8"></head><body><div id="root"></div><script type="module" src="/__buddy_harness__.js"></script></body></html>');
        response.setHeader('Content-Type', 'text/html; charset=utf-8');
        response.end(html);
      } else next();
    });
  }, resolveId(id) {
    if (id === '/__buddy_harness__.js') return '\0buddy-harness';
  }, load(id) {
    if (id === '\0buddy-harness') return harness;
  }}],
});
let browser;
try {
  await server.listen();
  const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
  browser = await chromium.launch({headless: true, channel: 'msedge'});
  const context = await browser.newContext({viewport: {width: 1500, height: 1100}});
  context.setDefaultTimeout(8000);
  await context.route('**/*', route => new URL(route.request().url()).origin === origin
    ? route.continue() : route.abort());
  async function scenario(name, action, view = 'accounts') {
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') console.error(message.text()); });
    try {
      await page.goto(`${origin}/__buddy_smoke__?view=${view}`);
      await page.waitForFunction(() => window.smoke);
      await action(page);
      assert.deepEqual(await page.evaluate(() => window.smoke.unexpected), []);
      assert.deepEqual(errors, []);
      results.push({name, status: 'passed'});
    } catch (error) {
      failures.push(name);
      results.push({name, status: 'failed', message: error.message, errors});
      await page.screenshot({path: join(output, `${name}.png`), fullPage: true});
    } finally { await page.close(); }
  }
  const refreshButton = page => page.getByRole('button', {name: '刷新', exact: true});
  const waitIdle = page => page.waitForFunction(() => window.smoke.calls.some(call => call.command === 'workbuddy_credits_fetch') && [...document.querySelectorAll('button')]
    .some(button => button.textContent.trim() === '刷新' && !button.disabled));
  await scenario('balances-and-unknown', async page => {
    await waitIdle(page);
    assert.match(await page.locator('body').innerText(), /37\.00/);
    assert.match(await page.locator('body').innerText(), /2600\.00/);
    await page.getByRole('button', {name: '1 个包'}).first().click();
    assert.match(await page.locator('body').innerText(), /已用 未知/);
  });
  await scenario('manual-single-request', async page => {
    await waitIdle(page);
    await page.evaluate(() => window.smoke.hold());
    await refreshButton(page).click();
    await page.waitForFunction(() => window.smoke.pending.length === 1);
    assert.equal(await refreshButton(page).isDisabled(), true);
    const count = await page.evaluate(() => window.smoke.calls.filter(call => call.command === 'workbuddy_credits_fetch').length);
    await refreshButton(page).evaluate(button => { button.click(); button.click(); });
    assert.equal(await page.evaluate(() => window.smoke.calls.filter(call => call.command === 'workbuddy_credits_fetch').length), count);
    await page.evaluate(() => window.smoke.release());
    await waitIdle(page);
    assert.match(await page.locator('body').innerText(), /积分已更新：2 个账号/);
  });
  await scenario('partial-failure', async page => {
    await waitIdle(page);
    await page.evaluate(() => window.smoke.partial());
    await refreshButton(page).click();
    await waitIdle(page);
    const body = await page.locator('body').innerText();
    assert.match(body, /成功 1 个，失败 1 个/);
    assert.match(body, /1 个账号积分查询失败/);
  });
  await scenario('slow-background-event', async page => {
    await waitIdle(page);
    await page.evaluate(() => window.smoke.stale());
    await refreshButton(page).click();
    await waitIdle(page);
    assert.match(await page.locator('body').innerText(), /正在后台更新/);
    await page.waitForTimeout(16000);
    await page.evaluate(() => window.smoke.event(456));
    await page.getByRole('button', {name: '1 个包'}).first().click();
    assert.match(await page.locator('body').innerText(), /456\.00 \/ 未知/);
    assert.equal(await page.getByRole('status').count(), 0);
    assert.equal(await page.evaluate(() => window.smoke.calls.filter(call => call.command === 'workbuddy_credits_fetch').length), 2);
  });
  await scenario('overview-all-failed', async page => {
    await waitIdle(page);
    await page.evaluate(() => window.smoke.failed());
    await refreshButton(page).click();
    await waitIdle(page);
    const total = page.getByText('可用总积分', {exact: true}).locator('..');
    assert.match(await total.innerText(), /—/);
    assert.doesNotMatch(await page.locator('body').innerText(), /Top 2/);
  }, 'overview');
  for (const view of ['accounts', 'overview']) {
    await scenario(`${view}-late-response`, async page => {
      await waitIdle(page);
      await page.evaluate(() => window.smoke.hold());
      await refreshButton(page).click();
      await page.waitForFunction(() => window.smoke.pending.length === 1);
      await page.evaluate(() => window.smoke.event(123));
      await page.waitForTimeout(150);
      await page.evaluate(() => window.smoke.release());
      await waitIdle(page);
      if (view === 'accounts') {
        await page.getByRole('button', {name: '1 个包'}).first().click();
        assert.match(await page.locator('body').innerText(), /123\.00 \/ 未知/);
      } else {
        assert.match(await page.locator('body').innerText(), /2723\.00/);
      }
    }, view);
  }
  await scenario('delete-synthetic-only', async page => {
    await waitIdle(page);
    await page.getByTitle('删除账号', {exact: true}).first().click();
    const checkbox = page.getByRole('checkbox');
    assert.equal(await checkbox.isChecked(), false);
    await checkbox.check();
    await page.getByRole('button', {name: '确认删除'}).click();
    await page.getByText('账号已删除', {exact: true}).waitFor();
    await waitIdle(page);
    const deletion = await page.evaluate(() => window.smoke.calls.find(call => call.command === 'workbuddy_account_remove'));
    assert.deepEqual(deletion.args, {userId: 'wb-test-A', deleteSnapshot: true});
    assert.equal(await page.getByText('测试账号 A', {exact: true}).count(), 0);
    assert.equal(await page.getByText('测试账号 B', {exact: true}).count(), 1);
  });
  const report = JSON.stringify({results, screenshots: output}, null, 2);
  await writeFile(join(output, 'results.json'), report);
  console.log(report);
  if (failures.length) process.exitCode = 1;
} finally {
  await browser?.close();
  await server.close();
}
