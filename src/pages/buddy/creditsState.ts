import type { WbCreditsResult } from '../../types';

export function creditsTotal(result: WbCreditsResult | null): number | null {
  const balances = result?.accounts.filter((account) => account.ok && account.balance != null && Number.isFinite(account.balance) && account.balance >= 0)
    .map((account) => account.balance as number) ?? [];
  return balances.length ? balances.reduce((total, balance) => total + balance, 0) : null;
}

export function creditsWarning(result: WbCreditsResult): string | null {
  if (result.stale) return '积分刷新失败，当前为历史缓存（非最新余额）';
  if (result.refreshing) return '积分正在后台更新，当前为历史缓存';
  const failed = result.accounts.filter((account) => !account.ok).length;
  return failed > 0 ? `${failed} 个账号积分查询失败，失败账号的旧余额未更新` : null;
}

export function creditsFeedback(result: WbCreditsResult): { tone: 'success' | 'warn'; message: string } {
  if (result.stale) return { tone: 'warn', message: '积分刷新失败，当前显示历史缓存（非最新余额）' };
  if (result.refreshing) return { tone: 'warn', message: '正在后台更新积分，当前显示历史缓存' };
  const succeeded = result.accounts.filter((account) => account.ok).length;
  const failed = result.accounts.length - succeeded;
  if (!result.ok || failed > 0 || succeeded === 0) {
    return { tone: 'warn', message: `积分刷新：成功 ${succeeded} 个，失败 ${failed} 个${succeeded === 0 ? '，无可更新余额' : ''}` };
  }
  if (result.cached) return { tone: 'warn', message: `显示 ${succeeded} 个账号的缓存积分，未重新查询` };
  return { tone: 'success', message: `积分已更新：${succeeded} 个账号` };
}
