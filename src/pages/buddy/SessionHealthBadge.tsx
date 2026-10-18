import type { WorkBuddyAccountView } from '../../types';

const labels: Record<string, string> = {
  unverified: '尚未验证会话',
  healthy: '最近续期成功',
  expiring: '凭证临期',
  transient: '暂时失败，等待重试',
  auth_required: '需要重新登录',
};

export function SessionHealthBadge({ health }: { health: WorkBuddyAccountView['session_health'] }) {
  if (!health) return null;
  const formatTime = (value: number) => new Date(value).toLocaleString('zh-CN');
  const details = [health.message,
    health.last_refresh_at ? `最近续期：${formatTime(health.last_refresh_at)}` : '尚无成功续期记录',
    health.last_attempt_at ? `最近尝试：${formatTime(health.last_attempt_at)}` : '',
    health.next_retry_at ? `最早重试：${formatTime(health.next_retry_at)}` : '',
  ].filter(Boolean).join('\n');
  const tone = health.status === 'auth_required' ? 'text-rose-500'
    : health.status === 'transient' || health.status === 'expiring' ? 'text-amber-500' : 'text-slate-400';
  return <div className={`mt-1 ${tone}`} title={details}>
    <div>{labels[health.status] || '尚未验证会话'}</div>
    {health.next_retry_at && <div>最早重试 {formatTime(health.next_retry_at)}</div>}
  </div>;
}
