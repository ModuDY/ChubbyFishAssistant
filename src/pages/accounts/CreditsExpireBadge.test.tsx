import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, expectTypeOf, it } from 'vitest';

import type { WorkBuddyAccountView } from '../../types';
import { CreditsExpireBadge } from './CreditsExpireBadge';

describe('Buddy 积分到期展示回归', () => {
  it('WorkBuddyAccountView 类型透出 credits_expire_at', () => {
    expectTypeOf<WorkBuddyAccountView['credits_expire_at']>().toEqualTypeOf<number | null>();
  });

  it('credits_expire_at 通过 CreditsExpireBadge 渲染剩余时间与到期提示', () => {
    const expireAt = Math.floor(Date.now() / 1000) + 3 * 86400 + 3600;
    const html = renderToStaticMarkup(<CreditsExpireBadge expireAt={expireAt} />);
    expect(html).toContain('3 天后过期');
    expect(html).toContain('最近到期积分包：');
  });

  it('无积分到期信息/已过期时展示兜底', () => {
    expect(renderToStaticMarkup(<CreditsExpireBadge expireAt={null} />)).toContain('>-<');
    const expired = Math.floor(Date.now() / 1000) - 60;
    expect(renderToStaticMarkup(<CreditsExpireBadge expireAt={expired} />)).toContain('已过期');
  });
});
