import { DatabaseSync } from 'node:sqlite';
import { createReadStream } from 'node:fs';
import { readdir } from 'node:fs/promises';
import { createInterface } from 'node:readline';
import { join } from 'node:path';
import { redact, splitLogEntries, proxyHeader } from './buddy-session-audit-support.mjs';

const root = process.env.AIWORK_BUDDY_AUDIT_DIR;
if (!root) throw new Error('Explicit AIWORK_BUDDY_AUDIT_DIR is required');
const day = process.env.AIWORK_BUDDY_AUDIT_DAY;
if (!/^\d{4}-\d{2}-\d{2}$/.test(day || '')) throw new Error('Explicit AIWORK_BUDDY_AUDIT_DAY (YYYY-MM-DD) is required');
const nextDay = new Date(Date.parse(`${day}T00:00:00Z`) + 86400000).toISOString().slice(0, 10);
const database = new DatabaseSync(join(root, 'data', 'aiwork.sqlite'), {readOnly: true});
const aliases = new Map();
const timestamp = value => typeof value === 'number' && value > 0
  ? new Date(value > 1e12 ? value : value * 1000).toISOString() : null;
const accountRows = database.prepare('SELECT pk, data FROM wb_accounts ORDER BY pk').all();
const accounts = accountRows.map((row, index) => {
  const account = JSON.parse(row.data);
  const alias = `account#${index + 1}`;
  for (const value of [row.pk, account.uid, account.nickname, account.phone_masked]) {
    if (typeof value === 'string' && value.length > 2) aliases.set(value, alias);
  }
  const tokenRow = database.prepare('SELECT data, updated_at FROM wb_tokens WHERE pk = ?').get(row.pk);
  const token = tokenRow ? JSON.parse(tokenRow.data) : {};
  return {alias, needs_relogin: account.needs_relogin, reason: account.relogin_reason,
    access_expires: timestamp(account.access_token_expires_at), refresh_expires: timestamp(account.refresh_token_expires_at),
    saved_at: timestamp(account.auth_saved_at), token_updated_at: tokenRow?.updated_at,
    token_access_expires: timestamp(token.expires_at_ms ?? token.expiresAtMs),
    token_refresh_expires: timestamp(token.refresh_expires_at_ms ?? token.refreshExpiresAtMs),
    metadata_uid_matches_pool: !token.uid ? 'missing' : token.uid === account.uid};
});
function safe(value) {
  return redact(value, aliases);
}
console.log(JSON.stringify({day, accounts: accounts.map(account => ({...account, reason: safe(account.reason ?? '')}))}, null, 2));
const stateRows = database.prepare("SELECT key, content FROM kv WHERE key IN ('scheduler_state', 'app_settings', 'workbuddy_settings')").all();
for (const row of stateRows) {
  const value = JSON.parse(row.content);
  const selected = row.key === 'scheduler_state'
    ? Object.fromEntries(Object.entries(value.tasks || {}).filter(([key]) => key.startsWith('wb-'))
      .map(([key, entry]) => [key, {...entry, last_run_ts: timestamp(entry.last_run_ts)}]))
    : Object.fromEntries(Object.entries(value).filter(([key]) => /wb_.*(?:renew|checkin|growth)|auto_checkin/.test(key)));
  console.log('CONFIG', row.key, safe(JSON.stringify(selected)));
}
database.close();
const logs = join(root, 'logs');
const files = await readdir(logs);
for (const name of ['app.log', 'checkin.log', 'switcher.log', `api_${day}.log`]) {
  if (!files.includes(name)) continue;
  const matches = [];
  const afternoon = [];
  const failuresByHour = new Map();
  let firstTimestamp = null;
  let lastTimestamp = null;
  const input = createInterface({input: createReadStream(join(logs, name)), crlfDelay: Infinity});
  for await (const line of input) {
    const normalized = name.startsWith('api_') ? line.replace(/^\[(\d{2}:\d{2}:\d{2})\]/, `[${day} $1]`) : line;
    for (const entry of splitLogEntries(normalized)) {
      firstTimestamp ??= entry.time;
      lastTimestamp = entry.time;
      if (![day, nextDay].includes(entry.time.slice(0, 10))) continue;
      if (!/(?:buddy|\bwb\b|wb-|WorkBuddy|CodeBuddy)/i.test(entry.message)
        || !/(?:401|403|429|refresh|续期|失效|登录|失败|renew|vault|保险库|growth|成长|checkin|签到)/i.test(entry.message)) continue;
      const event = {time: entry.time, message: safe(entry.message).slice(0, 420)};
      matches.push(event);
      if (entry.time >= `${day} 12:00:00` && entry.time < `${day} 19:00:00` && afternoon.length < 60) afternoon.push(event);
      if (/401|403|429|失效|refresh.*失败/i.test(entry.message)) {
        const account = event.message.match(/account#\d+|\[other-account\]/)?.[0] || 'unspecified';
        const key = `${entry.time.slice(0, 13)} ${account}`;
        const existing = failuresByHour.get(key);
        failuresByHour.set(key, existing ? {...existing, last: entry.time, count: existing.count + 1}
          : {first: entry.time, last: entry.time, count: 1, example: event.message});
      }
    }
  }
  console.log('TIMELINE', name, JSON.stringify({firstTimestamp, lastTimestamp, relevant: matches.length,
    first: matches.slice(0, 12), afternoon, last: matches.slice(-8), failuresByHour: [...failuresByHour]}));
}
const counts = new Map();
const coverage = [];
const failures = [];
const refreshes = [];
for (const name of files.filter(name => name.startsWith(`proxy_req_${day}.log`))) {
  let record = null;
  let section = '';
  let headers = 0;
  let buddyHeaders = 0;
  let firstTime = null;
  let lastTime = null;
  const finish = () => {
    if (!record || !record.relevant) return;
    const key = `${record.time.slice(0, 13)} ${record.path} ${record.status}`;
    counts.set(key, (counts.get(key) || 0) + 1);
    let message;
    let code;
    try {
      const body = JSON.parse(record.body);
      const candidateCode = body.code ?? body.error?.code ?? body.Response?.Error?.Code;
      if (['string', 'number'].includes(typeof candidateCode)) code = safe(candidateCode).slice(0, 100);
      const candidate = body.message ?? body.msg ?? body.error_description ?? body.error?.message ?? body.Response?.Error?.Message;
      if (typeof candidate === 'string') message = safe(candidate).slice(0, 300);
    } catch {}
    const entry = {time: record.time, path: record.path, status: record.status, account: record.account, code, message};
    if (record.status >= 400 && failures.length < 65) failures.push(entry);
    if (/refresh/i.test(record.path) && refreshes.length < 80) refreshes.push(entry);
  };
  const input = createInterface({input: createReadStream(join(logs, name)), crlfDelay: Infinity});
  for await (const line of input) {
    const header = proxyHeader(line);
    if (header) {
      finish();
      headers += 1;
      if (header.buddy) buddyHeaders += 1;
      firstTime ??= header.time;
      lastTime = header.time;
      const path = header.endpoint;
      record = {time: header.time, path: safe(path), relevant: header.buddy
        && /(?:refresh|auth|billing|login|growth)/i.test(path), status: null, body: ''};
      section = '';
    } else if (record?.relevant) {
      if (line.startsWith('--- Request Headers')) section = 'request';
      else if (line.startsWith('--- Response:')) { record.status = Number(line.match(/Response: (\d+)/)?.[1]); section = 'response'; }
      else if (line.startsWith('--- Response Body')) section = 'body';
      else if (line.startsWith('---')) section = '';
      else if (section === 'body' && record.body.length < 280000) record.body += line;
      else if (section === 'request' && /^\s*x-user-id:/i.test(line)) {
        record.account = aliases.get(line.split(':').slice(1).join(':').trim()) || 'unmapped';
      }
    }
  }
  finish();
  coverage.push({name, headers, buddyHeaders, firstTime, lastTime});
}
console.log('PROXY_COVERAGE', JSON.stringify(coverage));
console.log('PROXY_COUNTS', JSON.stringify([...counts].sort()));
console.log('PROXY_FAILURES', JSON.stringify(failures));
console.log('PROXY_REFRESH', JSON.stringify(refreshes));
