import { useEffect, useRef, useState } from 'react';
import { open, save } from '@tauri-apps/plugin-dialog';
import { api } from '../../lib/tauri';
import type { AccountBackupConfig, AccountBackupPreview, AccountBackupStatus } from '../../types';
import { Modal, Spinner } from '../ui';

const platforms = { trae: 'Trae', buddy: 'WorkBuddy / CodeBuddy', qoder: 'Qoder', doubao: '豆包' };

function expiry(value: string | number | null) {
  if (value === null) return '未知';
  const time = typeof value === 'number' ? (value > 1e12 ? value : value * 1000) : Date.parse(value);
  if (!Number.isFinite(time)) return '未知';
  return `${time <= Date.now() ? '已过期 · ' : ''}${new Date(time).toLocaleString()}`;
}

export default function BackupDialog({ onClose }: { onClose: () => void }) {
  const [session, setSession] = useState('');
  const sessionRef = useRef('');
  const [password, setPassword] = useState('');
  const [authorizationCode, setAuthorizationCode] = useState('');
  const [restorePassword, setRestorePassword] = useState('');
  const [view, setView] = useState<AccountBackupStatus | null>(null);
  const [config, setConfig] = useState<AccountBackupConfig | null>(null);
  const [preview, setPreview] = useState<AccountBackupPreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const dirty = !!view && (JSON.stringify(config) !== JSON.stringify(view.config) || !!authorizationCode.trim());

  useEffect(() => () => {
    if (sessionRef.current) void api.backupAction(sessionRef.current, { action: 'lock' }).catch(() => {});
  }, []);

  const refresh = async (token: string, resetForm = false) => {
    const next = await api.backupAction<AccountBackupStatus>(token, { action: 'status' });
    setView(next);
    if (resetForm) setConfig(next.config);
  };

  const perform = async (work: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setError('');
    setMessage('');
    try { await work(); } catch (e) {
      const text = String(e);
      setError(text);
      if (text.includes('会话过期') || text.includes('面板已锁定')) {
        sessionRef.current = '';
        setSession('');
        setView(null);
        setConfig(null);
        setPreview(null);
        setAuthorizationCode('');
      }
    } finally { setBusy(false); }
  };

  const unlock = () => perform(async () => {
    const value = password;
    setPassword('');
    const token = await api.backupUnlock(value);
    sessionRef.current = token;
    setSession(token);
    await refresh(token, true);
  });

  const action = (request: Record<string, unknown>) => perform(async () => {
    try {
      const result = await api.backupAction<{ message?: string; status?: string }>(session, request);
      if (result.status && ['failed', 'auth_error', 'uncertain'].includes(result.status)) {
        setError(result.message || '发送失败，请检查历史记录');
      } else {
        setMessage(result.message || '操作完成');
      }
    } finally {
      await refresh(session);
    }
  });

  const saveConfig = (clearCode = false) => perform(async () => {
    const code = authorizationCode;
    setAuthorizationCode('');
    const next = await api.backupAction<AccountBackupStatus>(session, {
      action: 'save', config, authorization_code: clearCode ? '' : (code.trim() || undefined),
    });
    setView(next);
    setConfig(next.config);
    setMessage(clearCode ? '授权码已清除，邮件备份等待配置。' : '配置已保存。开启后将按计划把全部托管账号的加密备份发往所填收件箱。');
  });

  const exportFile = () => perform(async () => {
    const path = await save({ title: '保存加密账号备份（请选择新文件）', defaultPath: `aiwork-${Date.now()}.aiwbak`, filters: [{ name: 'AI Work 加密备份', extensions: ['aiwbak'] }] });
    if (!path) return;
    await api.backupAction(session, { action: 'export', path });
    setMessage('加密备份已导出。');
    await refresh(session);
  });

  const previewFile = () => perform(async () => {
    setPreview(null);
    const path = await open({ title: '选择加密账号备份', multiple: false, directory: false, filters: [{ name: 'AI Work 加密备份', extensions: ['aiwbak'] }] });
    if (!path || Array.isArray(path)) return;
    const password = restorePassword;
    setRestorePassword('');
    setPreview(await api.backupAction<AccountBackupPreview>(session, { action: 'preview', path, password }));
  });

  const restore = () => perform(async () => {
    if (!preview) return;
    const result = await api.backupAction<{ added: number; skipped: number }>(session, { action: 'restore', preview_id: preview.preview_id });
    setPreview(null);
    setMessage(`恢复完成：新增 ${result.added} 个，跳过 ${result.skipped} 个已有账号。重新进入账号页可查看。`);
    await refresh(session);
  });

  const patch = <K extends keyof AccountBackupConfig>(key: K, value: AccountBackupConfig[K]) => {
    if (config) setConfig({ ...config, [key]: value });
  };

  return (
    <Modal open onClose={() => { if (!busy) onClose(); }} title={session ? '加密账号备份' : '访问验证'} size={session ? 'xl' : undefined} bodyClass="max-h-[72vh] overflow-y-auto"
      footer={<button className="btn-secondary" disabled={busy} onClick={onClose}>锁定并关闭</button>}>
      <div className="space-y-4 text-sm">
        {error && <p role="alert" className="rounded bg-rose-50 p-3 text-rose-700 dark:bg-rose-950/40 dark:text-rose-300">{error}</p>}
        {message && <p role="status" className="rounded bg-emerald-50 p-3 text-emerald-700 dark:bg-emerald-950/40 dark:text-emerald-300">{message}</p>}
        {!session ? (
          <form onSubmit={(e) => { e.preventDefault(); void unlock(); }} className="space-y-3">
            <label className="block">入口密码<input className="input mt-1 w-full" type="password" autoComplete="off" autoFocus value={password} onChange={(e) => setPassword(e.target.value)} maxLength={128} /></label>
            <button className="btn-primary" type="submit" disabled={busy || !password}>{busy && <Spinner />} 进入</button>
          </form>
        ) : config && view ? (
          <>
            <p className="rounded bg-slate-100 p-3 dark:bg-zinc-800">状态：{view.status}<br />范围：本应用管理的全部账号 · 本地保留最近 {view.retention} 份密文<br />邮件正文会附上附件密码，恢复时无需另记；固定密码强度有限，请保护邮箱与附件。</p>
            <p className="text-slate-500 dark:text-zinc-400">应用运行期间执行计划；每日模式会在当天错过时间后补跑。不含客户端快照、会话、分组和其他机器数据。</p>
            {view.runtime?.error && <p role="alert" className="text-rose-600 dark:text-rose-400">上次定时检查：{view.runtime.error}</p>}
            <fieldset disabled={busy} className="space-y-3">
              <label className="flex items-center gap-2"><input type="checkbox" checked={config.enabled} onChange={(e) => patch('enabled', e.target.checked)} />启用定时邮件备份</label>
              <div className="grid grid-cols-2 gap-3">
                <label>发件邮箱<input className="input mt-1 w-full" type="email" value={config.sender} onChange={(e) => patch('sender', e.target.value)} /></label>
                <label>收件邮箱<input className="input mt-1 w-full" type="email" value={config.recipient} onChange={(e) => patch('recipient', e.target.value)} /></label>
                <label>SMTP 主机<input className="input mt-1 w-full" value={config.smtp_host} onChange={(e) => patch('smtp_host', e.target.value)} /></label>
                <label>加密连接<select className="input mt-1 w-full" value={config.smtp_port} onChange={(e) => patch('smtp_port', Number(e.target.value))}><option value={465}>465 · TLS</option><option value={587}>587 · STARTTLS</option></select></label>
                <label>备份频率<select className="input mt-1 w-full" value={config.interval_hours} onChange={(e) => patch('interval_hours', Number(e.target.value))}><option value={0}>每天</option><option value={6}>每 6 小时</option><option value={12}>每 12 小时</option></select></label>
                <label>每日时间<input className="input mt-1 w-full" type="time" disabled={config.interval_hours !== 0} value={config.daily_time} onChange={(e) => patch('daily_time', e.target.value)} /></label>
              </div>
              <label className="block">QQ 邮箱 SMTP 授权码{view.smtp_builtin ? '（已内置默认，留空沿用）' : view.smtp_configured ? '（已配置，留空保留）' : '（待配置）'}<input className="input mt-1 w-full" type="password" autoComplete="new-password" value={authorizationCode} onChange={(e) => setAuthorizationCode(e.target.value)} maxLength={512} /></label>
              <p className="text-xs text-slate-500">在 QQ 邮箱设置中开启 SMTP，填写授权码。授权码保存在本机加密保险库。保存前请核对收件邮箱。{view.smtp_builtin && ' 当前使用内置默认授权码，可留空直接发送。'}</p>
              {dirty && <p className="text-amber-600 dark:text-amber-400">配置已修改，请先保存再发送邮件。</p>}
              <div className="flex flex-wrap gap-2">
                <button className="btn-primary" onClick={() => void saveConfig()}>保存配置</button>
                <button className="btn-secondary" onClick={() => void action({ action: 'test_mail' })} disabled={!view.smtp_configured || dirty}>发送无凭据测试邮件</button>
                <button className="btn-secondary" onClick={() => void saveConfig(true)} disabled={!view.smtp_configured}>清除授权码</button>
              </div>
              <div className="flex flex-wrap gap-2 border-t border-slate-200 pt-3 dark:border-zinc-800">
                <button className="btn-primary" disabled={dirty} onClick={() => void action({ action: 'run' })}>{view.smtp_configured ? '立即备份并发送' : '立即本地加密备份'}</button>
                <button className="btn-secondary" onClick={() => void exportFile()}>导出加密附件</button>
                <button className="btn-secondary" onClick={() => void perform(() => refresh(session))}>刷新状态</button>
              </div>
              <div className="space-y-2 border-t border-slate-200 pt-3 dark:border-zinc-800">
                <label className="block">恢复密码<input className="input ml-2" type="password" autoComplete="off" placeholder="留空使用指定的默认密码" value={restorePassword} onChange={(e) => setRestorePassword(e.target.value)} maxLength={128} /></label>
                <button className="btn-secondary" onClick={() => void previewFile()}>选择附件并预览恢复</button>
              </div>
            </fieldset>
            {preview && <div className="space-y-2 rounded border border-slate-200 p-3 dark:border-zinc-700">
              <p>恢复预览：共 {preview.total} 个，新增 {preview.new_accounts} 个，跳过已有 {preview.skipped} 个。预览有效期 5 分钟。</p>
              <p className="text-xs text-slate-500">仅补入新账号，不覆盖已有令牌和设备指纹，不切换客户端。缺少 Refresh Token 的账号可能需要重新登录。</p>
              <div className="max-h-44 overflow-y-auto"><table className="w-full text-left text-xs"><thead><tr><th>平台 / 账号</th><th>凭据</th><th>到期时间</th><th>操作</th></tr></thead><tbody>{preview.accounts.map((account, i) => <tr key={i}><td className="py-1">{platforms[account.platform]} · {account.name}</td><td>{account.has_credential ? (account.has_refresh_token ? '含刷新凭据' : '无刷新凭据') : '无凭据'}</td><td>{expiry(account.expires_at)}</td><td>{account.will_add ? '新增' : '跳过'}</td></tr>)}</tbody></table></div>
              <button className="btn-primary" disabled={busy || preview.new_accounts === 0} onClick={() => void restore()}>确认补入 {preview.new_accounts} 个新账号</button>
            </div>}
            <div className="space-y-2 border-t border-slate-200 pt-3 dark:border-zinc-800">
              <p className="font-medium">最近记录</p>
              {!view.history.length && <p className="text-slate-500">暂无备份</p>}
              <div className="max-h-56 space-y-2 overflow-y-auto">{view.history.slice(0, 10).map((run) => <div key={run.id} className="rounded bg-slate-50 p-2 text-xs dark:bg-zinc-800/60">
                <div>{new Date(run.created_at).toLocaleString()} · {run.total} 个账号 · {run.attempt} 次发送</div>
                <p>{run.message}</p>
                {run.missing_credentials > 0 && <p className="text-amber-600 dark:text-amber-400">其中 {run.missing_credentials} 个账号没有登录凭据，仅备份元数据。</p>}
                {run.next_retry && <p>下次重试：{new Date(run.next_retry * 1000).toLocaleString()}</p>}
                {run.status !== 'accepted' && view.smtp_configured && <button className="btn-secondary mt-1" disabled={busy || dirty} onClick={() => void action({ action: 'retry', id: run.id })}>重试此附件</button>}
              </div>)}</div>
            </div>
          </>
        ) : <button className="btn-secondary" disabled={busy} onClick={() => void perform(() => refresh(session, true))}>重新加载备份配置</button>}
        {busy && <p className="flex items-center gap-2 text-slate-500"><Spinner /> 正在处理，请勿关闭窗口…</p>}
      </div>
    </Modal>
  );
}
