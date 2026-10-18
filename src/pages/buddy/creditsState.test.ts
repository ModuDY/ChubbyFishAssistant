import { describe, expect, it } from 'vitest';
import { creditsFeedback, creditsTotal, creditsWarning } from './creditsState';
import type { WbCreditAccount, WbCreditsResult } from '../../types';

const account = (ok: boolean): WbCreditAccount => ({ user_id: 'test', name: '测试', ok, balance: ok ? 37 : null, packages: [], source: 'test' });
const result = (patch: Partial<WbCreditsResult> = {}): WbCreditsResult => ({ ok: true, accounts: [account(true)], ...patch });

describe('Buddy 积分刷新反馈', () => {
  it('全部失败、未知或不限量不伪装为零余额', () => {
    expect(creditsTotal(null)).toBeNull();
    expect(creditsTotal(result({ accounts: [] }))).toBeNull();
    expect(creditsTotal(result({ accounts: [account(false)] }))).toBeNull();
    for (const balance of [null, -1, NaN, Infinity]) {
      expect(creditsTotal(result({ accounts: [{ ...account(true), balance }] }))).toBeNull();
    }
    expect(creditsTotal(result({ accounts: [{ ...account(true), balance: 0 }] }))).toBe(0);
  });
  it('合计只纳入成功查询的有限非负余额', () => {
    expect(creditsTotal(result({ accounts: [account(true), { ...account(false), balance: 999 }, { ...account(true), balance: 2600 }] }))).toBe(2637);
  });
  it('仅真实成功查询显示已更新', () => {
    expect(creditsFeedback(result())).toEqual({ tone: 'success', message: '积分已更新：1 个账号' });
  });
  it('全部失败回退缓存不报告成功', () => {
    expect(creditsFeedback(result({ stale: true, cached: true })).tone).toBe('warn');
    expect(creditsFeedback(result({ stale: true })).message).toContain('历史缓存');
  });
  it('区分后台刷新、缓存与部分失败', () => {
    expect(creditsWarning(result())).toBeNull();
    expect(creditsWarning(result({ accounts: [account(false)] }))).toContain('旧余额未更新');
    expect(creditsFeedback(result({ refreshing: true })).message).toContain('后台');
    expect(creditsFeedback(result({ cached: true })).tone).toBe('warn');
    expect(creditsFeedback(result({ accounts: [account(true), account(false)] }))).toEqual({ tone: 'warn', message: '积分刷新：成功 1 个，失败 1 个' });
    expect(creditsFeedback(result({ accounts: [] })).tone).toBe('warn');
  });
});
