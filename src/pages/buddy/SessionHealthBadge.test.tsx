import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { SessionHealthBadge } from './SessionHealthBadge';
import type { WorkBuddyAccountView } from '../../types';

const health = (status: NonNullable<WorkBuddyAccountView['session_health']>['status']): NonNullable<WorkBuddyAccountView['session_health']> => ({
  status, last_refresh_at: null, last_attempt_at: null, next_retry_at: null, http_status: null, message: '',
});

describe('Buddy 会话维护状态', () => {
  it('没有真实续期记录时不显示验证成功', () => {
    const html = renderToStaticMarkup(<SessionHealthBadge health={health('unverified')} />);
    expect(html).toContain('尚未验证会话');
    expect(html).not.toContain('最近续期成功');
  });
  it('限流显示暂时失败和最早重试时间', () => {
    const html = renderToStaticMarkup(<SessionHealthBadge health={{ ...health('transient'), http_status: 429, next_retry_at: 1_800_000_000_000, message: '续期被限流' }} />);
    expect(html).toContain('暂时失败，等待重试');
    expect(html).toContain('最早重试');
    expect(html).not.toContain('需要重新登录');
  });
  it('明确失效显示重登，兼容旧后端缺字段', () => {
    expect(renderToStaticMarkup(<SessionHealthBadge health={health('auth_required')} />)).toContain('需要重新登录');
    expect(renderToStaticMarkup(<SessionHealthBadge health={undefined} />)).toBe('');
  });
});
