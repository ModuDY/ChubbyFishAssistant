import test from 'node:test';
import assert from 'node:assert/strict';
import { redact, splitLogEntries, proxyHeader } from './buddy-session-audit-support.mjs';

test('redacts camelCase, snake_case and unknown account identifiers', () => {
  const output = redact('{"accessToken":"secret-a","refresh_token":"secret-b"} wb-abcdef123456 1234567890123456 13912345678 someone@example.com');
  for (const sensitive of ['secret-a', 'secret-b', 'wb-abcdef123456', '1234567890123456', '13912345678', 'someone@example.com']) {
    assert.ok(!output.includes(sensitive));
  }
});

test('uses anonymous aliases and preserves scheduler task keys', () => {
  assert.equal(redact('wb-abcdef123456 uid-known wb-renew', new Map([['wb-abcdef123456', 'account#1'], ['uid-known', 'account#1']])), 'account#1 account#1 wb-renew');
});

test('splits concatenated application log timestamps', () => {
  assert.deepEqual(splitLogEntries('[2026-10-17 12:00:00] wb: first[2026-10-17 12:00:01] wb: second'), [
    { time: '2026-10-17 12:00:00', message: 'wb: first' },
    { time: '2026-10-17 12:00:01', message: 'wb: second' },
  ]);
  assert.equal(splitLogEntries('\uFEFF[2026-10-17 12:00:00] wb: first')[0].time, '2026-10-17 12:00:00');
});

test('proxy coverage distinguishes missing Buddy capture from no failures', () => {
  assert.equal(proxyHeader('[2026-10-17 12:00:00] POST api.trae.cn/example').buddy, false);
  assert.equal(proxyHeader('[2026-10-17 12:00:00] POST copilot.tencent.com/auth/refresh').buddy, true);
  assert.equal(proxyHeader('[2026-10-17 12:00:00] POST www.workbuddy.ai/auth/refresh').buddy, true);
  assert.equal(proxyHeader('[2026-10-17 12:00:00] POST fakeworkbuddy.ai/auth/refresh').buddy, false);
  assert.equal(proxyHeader('Authorization: Bearer secret'), null);
});
