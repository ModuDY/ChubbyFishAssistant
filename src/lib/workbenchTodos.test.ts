import { describe, expect, it } from 'vitest';
import { buildTodos, parseCapture } from './workbenchTodos';
import type { AccountView, ApiServiceStatus, EnvStatus } from '../types';

/** 构造最小 AccountView（只填规则用到的字段，避免测试随类型膨胀而失效） */
function acc(over: Partial<AccountView> = {}): AccountView {
  return {
    user_id: 'u1',
    name: 'a',
    jwt: '',
    jwt_exp_hours: null,
    jwt_exp_timestamp: null,
    checked_today: true,
    credits: null,
    remaining_credits: 0,
    device_id_masked: null,
    cooldown_type: null,
    cooldown_until: null,
    cooldown_reason: null,
    has_refresh_token: false,
    jwt_auto_refresh: false,
    credits_expire_at: null,
    ...over,
  } as AccountView;
}

const envOk: EnvStatus = { installed: true, running: false, version: '1.0', path: 'C:/x' };
const apiOff: ApiServiceStatus = {
  running: false,
  port: 7864,
  total_requests: 0,
  active_uid: null,
  last_error: null,
  started_at: null,
};

describe('buildTodos', () => {
  it('全新安装（无账号、无 Trae）只给一条安装引导，不刷屏', () => {
    const todos = buildTodos({
      accounts: [],
      apiStatus: null,
      certInstalled: false,
      env: { installed: false, running: false, version: null, path: null },
      platformCounts: { trae: 0, buddy: 0, qoder: 0 },
    });
    expect(todos).toHaveLength(1);
    expect(todos[0].id).toBe('env-missing');
  });

  it('一切正常时返回空数组 —— 工作台据此整块隐藏，不显示空卡', () => {
    const todos = buildTodos({
      accounts: [acc({ checked_today: true })],
      apiStatus: { ...apiOff, running: true },
      certInstalled: true,
      env: envOk,
      platformCounts: { trae: 1, buddy: 0, qoder: 0 },
    });
    expect(todos).toEqual([]);
  });

  it('CA 证书未装是 danger 且排最前（阻断抓包链路）', () => {
    const todos = buildTodos({
      accounts: [acc({ jwt_exp_hours: 3, has_refresh_token: true })],
      apiStatus: apiOff,
      certInstalled: false,
      env: envOk,
      platformCounts: { trae: 1, buddy: 0, qoder: 0 },
    });
    expect(todos[0].id).toBe('cert-missing');
    expect(todos[0].tone).toBe('danger');
  });

  it('JWT 临期且可续期 → 给"一键续期"；不可续期 → 引导重新登录', () => {
    const renewable = buildTodos({
      accounts: [acc({ jwt_exp_hours: 12, has_refresh_token: true })],
      apiStatus: { ...apiOff, running: true },
      certInstalled: true,
      env: envOk,
      platformCounts: { trae: 1, buddy: 0, qoder: 0 },
    });
    expect(renewable.some((t) => t.id === 'jwt-renew')).toBe(true);
    expect(renewable.some((t) => t.id === 'jwt-relogin')).toBe(false);

    const notRenewable = buildTodos({
      accounts: [acc({ jwt_exp_hours: 12, has_refresh_token: false })],
      apiStatus: { ...apiOff, running: true },
      certInstalled: true,
      env: envOk,
      platformCounts: { trae: 1, buddy: 0, qoder: 0 },
    });
    expect(notRenewable.some((t) => t.id === 'jwt-renew')).toBe(false);
    expect(notRenewable.some((t) => t.id === 'jwt-relogin')).toBe(true);
  });

  it('JWT 剩余 25h 不提醒（阈值严格 24h 内）', () => {
    const todos = buildTodos({
      accounts: [acc({ jwt_exp_hours: 25, has_refresh_token: true })],
      apiStatus: { ...apiOff, running: true },
      certInstalled: true,
      env: envOk,
      platformCounts: { trae: 1, buddy: 0, qoder: 0 },
    });
    expect(todos.some((t) => t.id.startsWith('jwt'))).toBe(false);
  });

  it('无账号的平台不产生签到提醒（避免给未启用平台刷噪音）', () => {
    const todos = buildTodos({
      accounts: [],
      apiStatus: null,
      certInstalled: true,
      env: envOk,
      platformCounts: { trae: 0, buddy: 0, qoder: 0 },
    });
    expect(todos.some((t) => t.id.startsWith('checkin'))).toBe(false);
  });

  it('积分已过期不再提示"即将过期"（只提示未来 7 天内的）', () => {
    const nowSec = 1_000_000;
    const todos = buildTodos({
      accounts: [
        acc({
          credits_expire_at: nowSec - 100, // 已过期
          remaining_credits: 500,
        }),
      ],
      apiStatus: { ...apiOff, running: true },
      certInstalled: true,
      env: envOk,
      nowSec,
      platformCounts: { trae: 1, buddy: 0, qoder: 0 },
    });
    expect(todos.some((t) => t.id === 'credits-expiring')).toBe(false);
  });

  it('剩余积分为 0 时到期不提示（没东西可用，提示无意义）', () => {
    const nowSec = 1_000_000;
    const todos = buildTodos({
      accounts: [
        acc({
          credits_expire_at: nowSec + 86400,
          remaining_credits: 0,
        }),
      ],
      apiStatus: { ...apiOff, running: true },
      certInstalled: true,
      env: envOk,
      nowSec,
      platformCounts: { trae: 1, buddy: 0, qoder: 0 },
    });
    expect(todos.some((t) => t.id === 'credits-expiring')).toBe(false);
  });

  it('严重度排序稳定：danger 全在 warn 前，warn 全在 info 前', () => {
    const nowSec = 1_000_000;
    const todos = buildTodos({
      accounts: [
        acc({ jwt_exp_hours: 2, has_refresh_token: true, checked_today: false }),
        acc({
          user_id: 'u2',
          credits_expire_at: nowSec + 3600,
          remaining_credits: 100,
          checked_today: false,
        }),
      ],
      apiStatus: apiOff,
      certInstalled: false,
      env: envOk,
      nowSec,
      platformCounts: { trae: 2, buddy: 0, qoder: 0 },
    });
    const order = { danger: 0, warn: 1, info: 2 };
    const weights = todos.map((t) => order[t.tone]);
    expect(weights).toEqual([...weights].sort((a, b) => a - b));
  });
});

describe('parseCapture', () => {
  it('空输入返回 null', () => {
    expect(parseCapture('')).toBeNull();
    expect(parseCapture('   ')).toBeNull();
  });

  it('? 前缀 → 问知识库', () => {
    expect(parseCapture('?上次那个报错')).toEqual({ kind: 'ask', text: '上次那个报错' });
  });

  it('/ 前缀 → 走命令', () => {
    expect(parseCapture('/checkin')).toEqual({ kind: 'command', text: 'checkin' });
  });

  it('只有前缀无内容 → null（不产生空意图）', () => {
    expect(parseCapture('?')).toBeNull();
    expect(parseCapture('/')).toBeNull();
  });

  it('普通文本 → 存笔记', () => {
    expect(parseCapture('  记一个想法  ')).toEqual({ kind: 'note', text: '记一个想法' });
  });
});
