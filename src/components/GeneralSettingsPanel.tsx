import { useEffect, useState } from 'react';
import { Save, RotateCcw, Bell, Send } from 'lucide-react';
import { useAppStore } from '../store';
import { withMinDelay } from '../lib/delay';
import { api } from '../lib/tauri';
import { THEMES } from '../lib/themes';
import { APP_TABS } from './Sidebar';
import { ICON_CHOICES, resolveAppIcon, DEFAULT_APP_ICONS } from '../lib/appIcons';
import type {
  AppKey,
  Settings as SettingsType,
  PetAiMode,
  EvolutionMetrics,
} from '../types';

/**
 * 通用设置面板：外观 / 语言 / 通用与通知 / 代理相关配置。
 * 供「系统设置」弹框（左下角系统图标）使用；环境配置页不包含这些区块。
 * 表单为本地状态，点击「保存」才持久化。
 */
export default function GeneralSettingsPanel() {
  const settings = useAppStore((s) => s.settings);
  const saveSettings = useAppStore((s) => s.saveSettings);
  const refreshSettings = useAppStore((s) => s.refreshSettings);
  const toast = useAppStore((s) => s.pushToast);

  const [form, setForm] = useState<SettingsType | null>(null);
  const [saving, setSaving] = useState(false);
  // 开机自启（T11）：注册表 Run 项即时生效，不随「保存设置」提交
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  // 通知渠道「发送测试」执行态
  const [testing, setTesting] = useState(false);
  // 应用图标选择器：当前展开替换哪个应用的图标（null = 全部收起）
  const [iconPickerFor, setIconPickerFor] = useState<AppKey | null>(null);
  // P4-3：可用的桌宠素材包（用户目录 pets/<id>/，不含内置）
  const [packs, setPacks] = useState<Array<{ id: string; name: string; action_count: number }>>([]);
  // 桌宠对话专用模型候选：统一模型目录的 id 列表（加载失败留空 = 只能跟随网关默认）
  const [modelOptions, setModelOptions] = useState<string[]>([]);
  // 自我进化 P1：网关观测快照（只读，10s 轮询；网关未运行时不显示）
  const [metrics, setMetrics] = useState<EvolutionMetrics | null>(null);

  useEffect(() => {
    void api.pet
      .packList()
      .then((list) => setPacks(Array.isArray(list) ? list : []))
      .catch(() => setPacks([]));
    void api.apiServer
      .unifiedModels(true)
      .then((list) =>
        setModelOptions(
          Array.isArray(list) ? list.map((m) => m.id).filter(Boolean).sort() : [],
        ),
      )
      .catch(() => setModelOptions([]));
  }, []);

  useEffect(() => {
    const load = () => {
      void api.evolution
        .metrics()
        .then((m) => setMetrics(m?.running ? m : null))
        .catch(() => setMetrics(null));
    };
    load();
    const timer = window.setInterval(load, 10_000);
    return () => window.clearInterval(timer);
  }, []);

  const sendTest = async () => {
    setTesting(true);
    try {
      const msg = await api.misc.notifyTest();
      toast('success', msg);
    } catch (e) {
      toast('error', String(e));
    } finally {
      setTesting(false);
    }
  };

  useEffect(() => {
    void refreshSettings();
    api.misc
      .autostartStatus()
      .then(setAutostart)
      .catch(() => setAutostart(false));
  }, [refreshSettings]);

  const toggleAutostart = async () => {
    if (autostartBusy) return;
    setAutostartBusy(true);
    const next = !autostart;
    try {
      await api.misc.autostartSet(next);
      setAutostart(next);
      toast('success', next ? '已开启开机自启' : '已关闭开机自启');
    } catch (e) {
      toast('error', `设置开机自启失败：${String(e)}`);
    } finally {
      setAutostartBusy(false);
    }
  };

  useEffect(() => {
    if (settings && !form) {
      // 兼容旧主题值：light→graphite、dark→charcoal
      const theme = settings.theme === 'light' ? 'graphite' : settings.theme === 'dark' ? 'charcoal' : settings.theme;
      setForm({ ...settings, theme });
    }
  }, [settings, form]);

  const dirty = form != null && settings != null && JSON.stringify(form) !== JSON.stringify(settings);

  const update = <K extends keyof SettingsType>(key: K, val: SettingsType[K]) => {
    setForm((prev) => (prev ? { ...prev, [key]: val } : prev));
  };

  // 切换固定应用：新固定项强制从隐藏列表移除（始终可见、不可取消）
  const setPinned = (key: AppKey) => {
    setForm((prev) =>
      prev ? { ...prev, pinned_app: key, hidden_apps: prev.hidden_apps.filter((a) => a !== key) } : prev,
    );
  };

  // 勾选 = 显示（移出隐藏列表）；取消勾选 = 隐藏（加入隐藏列表）
  const toggleAppVisible = (key: AppKey) => {
    setForm((prev) =>
      prev
        ? {
            ...prev,
            hidden_apps: prev.hidden_apps.includes(key)
              ? prev.hidden_apps.filter((a) => a !== key)
              : [...prev.hidden_apps, key],
          }
        : prev,
    );
  };

  // 设置应用图标：null 或与内置默认同名 = 删除自定义项（回退 APP_TABS 默认图标）
  const setAppIcon = (key: AppKey, icon: string | null) => {
    setForm((prev) => {
      if (!prev) return prev;
      const appIcons = { ...prev.app_icons };
      if (!icon || icon === DEFAULT_APP_ICONS[key]) delete appIcons[key];
      else appIcons[key] = icon;
      return { ...prev, app_icons: appIcons };
    });
  };

  const save = async () => {
    if (!form) return;
    setSaving(true);
    try {
      await withMinDelay(saveSettings(form));
      toast('success', '设置已保存');
    } catch {
      /* toast 已发出 */
    } finally {
      setSaving(false);
    }
  };

  const reset = () => {
    if (settings) setForm({ ...settings });
  };

  if (!form) {
    return <div className="py-6 text-center text-sm text-slate-400">加载中…</div>;
  }

  return (
    <div className="flex flex-col gap-4 text-sm">
      {/* 外观 */}
      <section className="card p-4">
        <h3 className="mb-3 font-medium">外观</h3>
        <div className="grid gap-3 sm:grid-cols-2">
          <div>
            <label className="label">主题</label>
            <select value={form.theme} onChange={(e) => update('theme', e.target.value)} className="input">
              <option value="system">跟随系统</option>
              {THEMES.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name}
                </option>
              ))}
            </select>
          </div>
          <div>
            <label className="label">语言</label>
            <select value={form.language} onChange={(e) => update('language', e.target.value)} className="input">
              <option value="zh-CN">简体中文</option>
              <option value="en-US">English</option>
            </select>
          </div>
        </div>
      </section>

      {/* 应用显示（侧边栏应用 Tab：固定应用单选 + 显示勾选 + 行首图标点击换图标） */}
      <section className="card p-4">
        <h3 className="mb-1 font-medium">应用显示</h3>
        <p className="mb-3 text-xs text-slate-400">
          控制左下角侧边栏显示哪些应用。「固定应用」始终显示且不可隐藏（默认 Trae）；其余应用可自由勾选。
          隐藏仅收起入口，<b>不删除账号数据、不影响签到 / 保活等定时任务</b>，重新勾选即可恢复；
          若当前正在浏览的应用被隐藏，会自动跳回固定应用。「侧边栏显示」行首的图标可点击更换应用图标。
        </p>
        <div className="grid gap-x-8 gap-y-2 sm:grid-cols-2">
          <div className="space-y-2">
            <label className="label">固定应用（始终显示）</label>
            {APP_TABS.map((tab) => (
              <label key={tab.key} className="flex items-center gap-2">
                <input
                  type="radio"
                  name="pinned-app"
                  checked={form.pinned_app === tab.key}
                  onChange={() => setPinned(tab.key)}
                />
                {tab.label}
                {tab.title ? <span className="text-xs text-slate-400">（{tab.title}）</span> : null}
              </label>
            ))}
          </div>
          <div className="space-y-2">
            <label className="label">侧边栏显示</label>
            {APP_TABS.map((tab) => {
              const pinned = form.pinned_app === tab.key;
              const visible = pinned || !form.hidden_apps.includes(tab.key);
              const customizedName = form.app_icons[tab.key];
              const Current = resolveAppIcon(customizedName) ?? tab.icon;
              return (
                <div key={tab.key} className={`flex items-center gap-2 ${pinned ? 'opacity-60' : ''}`}>
                  {/* 行首图标：点击弹出候选表更换该应用在侧边栏的显示图标（仅 UI 偏好） */}
                  <button
                    type="button"
                    title={`更换「${tab.label}」图标${customizedName ? `（当前：${customizedName}）` : '（当前：默认）'}`}
                    onClick={() => setIconPickerFor(tab.key)}
                    className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-slate-200 text-slate-600 transition hover:border-zinc-400 hover:text-zinc-800 active:scale-95 dark:border-zinc-700 dark:text-zinc-300 dark:hover:border-zinc-500 dark:hover:text-zinc-100"
                  >
                    <Current size={15} />
                  </button>
                  <label className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={visible}
                      disabled={pinned}
                      onChange={() => toggleAppVisible(tab.key)}
                    />
                    {tab.label}
                    {pinned ? <span className="text-xs text-slate-400">（固定应用，不可隐藏）</span> : null}
                  </label>
                </div>
              );
            })}
          </div>
        </div>
      </section>

      {/* 图标选择弹框：点「侧边栏显示」行首图标弹出，在候选表内选择；选后需点底部「保存设置」生效 */}
      {iconPickerFor
        ? (() => {
            const tab = APP_TABS.find((t) => t.key === iconPickerFor)!;
            const customizedName = form.app_icons[tab.key];
            const selectedName = customizedName ?? DEFAULT_APP_ICONS[tab.key];
            return (
              <div
                className="fixed inset-0 z-[60] flex items-center justify-center p-4"
                onClick={() => setIconPickerFor(null)}
              >
                <div className="absolute inset-0 bg-black/40 backdrop-blur-sm" />
                <div
                  className="card animate-fade-in relative z-10 w-full max-w-sm p-4 shadow-xl"
                  role="dialog"
                  aria-modal="true"
                  onClick={(e) => e.stopPropagation()}
                >
                  <h3 className="mb-1 text-sm font-semibold text-slate-800 dark:text-zinc-100">
                    更换「{tab.label}」图标
                  </h3>
                  <p className="mb-3 text-xs text-slate-400">
                    选择侧边栏显示图标，点底部「保存设置」生效；非法/缺失自动回退内置默认（{DEFAULT_APP_ICONS[tab.key]}）。
                  </p>
                  <div className="grid grid-cols-8 gap-1">
                    {ICON_CHOICES.map((c) => {
                      const Choice = c.icon;
                      const selected = selectedName === c.name;
                      return (
                        <button
                          key={c.name}
                          type="button"
                          title={c.name}
                          onClick={() => {
                            setAppIcon(tab.key, c.name);
                            setIconPickerFor(null);
                          }}
                          className={`flex h-8 items-center justify-center rounded-md transition ${
                            selected
                              ? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-900'
                              : 'text-slate-600 hover:bg-slate-200 dark:text-zinc-400 dark:hover:bg-zinc-800'
                          }`}
                        >
                          <Choice size={16} />
                        </button>
                      );
                    })}
                  </div>
                  <div className="mt-3 flex items-center justify-between">
                    {customizedName ? (
                      <button
                        className="btn-outline !px-2.5 !py-1 text-xs"
                        onClick={() => {
                          setAppIcon(tab.key, null);
                          setIconPickerFor(null);
                        }}
                      >
                        <RotateCcw size={12} /> 恢复默认
                      </button>
                    ) : (
                      <span className="text-xs text-slate-400">当前使用默认图标</span>
                    )}
                    <button className="btn-outline !px-2.5 !py-1 text-xs" onClick={() => setIconPickerFor(null)}>
                      取消
                    </button>
                  </div>
                </div>
              </div>
            );
          })()
        : null}

      {/* 通用与通知（两列布局：左选项/右开关组） */}
      <section className="card p-4">
        <h3 className="mb-3 font-medium">通用与通知</h3>
        <div className="grid gap-x-8 gap-y-3 text-sm sm:grid-cols-2">
          <div className="space-y-3">
            <div>
              <label className="label">通知方式</label>
              <select value={form.notify} onChange={(e) => update('notify', e.target.value)} className="input">
                <option value="toast">应用内 Toast</option>
                <option value="system">系统通知</option>
                <option value="both">Toast + 系统通知</option>
                <option value="none">不通知</option>
              </select>
            </div>
            <div>
              <label className="label">日志保留天数</label>
              <input
                type="number"
                value={form.log_retention_days}
                onChange={(e) => update('log_retention_days', Math.min(365, Math.max(1, Number(e.target.value) || 30)))}
                className="input w-24"
                min={1}
                max={365}
              />
              <p className="mt-1 text-xs text-slate-400">
                应用启动时自动清理超过保留天数的运行日志（代理 / 签到 / 切换日志）。
              </p>
            </div>
          </div>
          <div className="space-y-3">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.launch_minimized}
                onChange={(e) => update('launch_minimized', e.target.checked)}
              />
              启动时最小化到托盘
            </label>
            <label className="flex items-center gap-2">
              <input type="checkbox" checked={form.tray} onChange={(e) => update('tray', e.target.checked)} />
              启用系统托盘图标
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={autostart}
                onChange={() => void toggleAutostart()}
                disabled={autostartBusy}
              />
              开机自启
              <span className="text-xs text-slate-400">（开关即时生效，无需保存）</span>
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.silent_checkin}
                onChange={(e) => update('silent_checkin', e.target.checked)}
              />
              启动静默签到
              <span className="text-xs text-slate-400">（启动 60 秒后自动为未签到账号签到）</span>
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.auto_start_api}
                onChange={(e) => update('auto_start_api', e.target.checked)}
              />
              启动时自动开启 API 网关
              <span className="text-xs text-slate-400">（网关监听所有网卡，局域网设备可访问）</span>
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.sessions_preview_enabled}
                onChange={(e) => update('sessions_preview_enabled', e.target.checked)}
              />
              会话明细显示首条消息概述
              <span className="text-xs text-slate-400">
                （用量看板「会话说明」列；关闭仅隐藏展示，不删除缓存）
              </span>
            </label>
            <p className="text-xs text-slate-400">托盘与最小化设置变更后需重启应用生效。</p>
          </div>
        </div>
      </section>

      {/* 桌宠（P0）：开关 / 气泡 / 缩放 / 打盹；位置由窗口拖拽自动记忆 */}
      <section className="card p-4">
        <h3 className="mb-1 font-medium">桌宠</h3>
        <p className="mb-3 text-xs text-slate-400">
          桌面上的「大肥鱼」：把签到、积分、切换账号的进度变成实时动作，不参与业务、不写任何数据。
          默认关闭；开启后也可以从托盘菜单「显示桌宠 / 隐藏桌宠」随时切换。保存后立即生效。
        </p>
        <div className="grid gap-x-8 gap-y-3 text-sm sm:grid-cols-2">
          <div className="space-y-3">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.pet_enabled ?? false}
                onChange={(e) => update('pet_enabled', e.target.checked)}
              />
              启用桌面宠物
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.pet_bubble ?? true}
                onChange={(e) => update('pet_bubble', e.target.checked)}
              />
              显示气泡台词
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.pet_gateway_link ?? false}
                onChange={(e) => update('pet_gateway_link', e.target.checked)}
              />
              联动 API 网关活动
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.pet_anniversary ?? true}
                onChange={(e) => update('pet_anniversary', e.target.checked)}
              />
              节日与时段动作（节令 / 饭点自动播）
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.pet_easter_egg ?? true}
                onChange={(e) => update('pet_easter_egg', e.target.checked)}
              />
              待机随机彩蛋（生活 / 才艺 / 特效）
            </label>
            <div>
              <label className="label">大小</label>
              <div className="flex items-center gap-2">
                <input
                  type="range"
                  min={0.6}
                  max={1.5}
                  step={0.1}
                  value={form.pet_scale ?? 1}
                  onChange={(e) => update('pet_scale', Number(e.target.value))}
                  className="w-40"
                />
                <span className="text-xs text-slate-400">{Math.round((form.pet_scale ?? 1) * 100)}%</span>
              </div>
            </div>
          </div>
          <div className="space-y-3">
            <div>
              <label className="label">打盹触发（无事件分钟数）</label>
              <input
                type="number"
                min={1}
                max={120}
                value={form.pet_sleep_minutes ?? 5}
                onChange={(e) =>
                  update('pet_sleep_minutes', Math.min(120, Math.max(1, Number(e.target.value) || 5)))
                }
                className="input w-24"
              />
            </div>
            <div>
              <label className="label">彩蛋触发间隔（分钟）</label>
              <input
                type="number"
                min={1}
                max={30}
                value={form.pet_easter_egg_minutes ?? 1}
                onChange={(e) =>
                  update(
                    'pet_easter_egg_minutes',
                    Math.min(30, Math.max(1, Number(e.target.value) || 1)),
                  )
                }
                className="input w-24"
              />
              <p className="text-xs text-slate-400 mt-1">节日 / 时段 / 随机彩蛋共用此间隔（1~30）。</p>
            </div>
            <div>
              <label className="label">透明度</label>
              <div className="flex items-center gap-2">
                <input
                  type="range"
                  min={0.3}
                  max={1}
                  step={0.05}
                  value={form.pet_opacity ?? 1}
                  onChange={(e) => update('pet_opacity', Number(e.target.value))}
                  className="w-40"
                />
                <span className="text-xs text-slate-400">
                  {Math.round((form.pet_opacity ?? 1) * 100)}%
                </span>
              </div>
            </div>
            <div>
              <label className="label">AI 对话</label>
              <select
                value={form.pet_ai_mode ?? 'off'}
                onChange={(e) => update('pet_ai_mode', e.target.value as PetAiMode)}
                className="input w-40"
              >
                <option value="off">关闭（只用本地台词）</option>
                <option value="manual">仅主动（右键「跟我聊一句」）</option>
                <option value="auto">自动点评 + 主动</option>
              </select>
            </div>
            <div>
              <label className="label">人格</label>
              <select
                value={form.pet_persona ?? 'whale'}
                onChange={(e) => update('pet_persona', e.target.value)}
                className="input w-40"
              >
                <option value="whale">憨憨大肥鱼</option>
                <option value="tsundere">傲娇</option>
                <option value="butler">正经管家</option>
                <option value="zen">禅意</option>
              </select>
            </div>
            <div>
              <label className="label">对话模型</label>
              <select
                value={form.pet_ai_model ?? ''}
                onChange={(e) => update('pet_ai_model', e.target.value || null)}
                className="input w-56"
              >
                <option value="">跟随网关默认模型</option>
                {modelOptions.map((id) => (
                  <option key={id} value={id}>
                    {id}
                  </option>
                ))}
              </select>
              <p className="text-xs text-slate-400 mt-1">
                想让桌宠回得快就选 flash 档模型（如 deepseek-v4-flash）；
                跟随默认时用的是网关设置里的模型，重型模型会明显变慢。
              </p>
            </div>
            <div>
              <label className="label">联网查天气</label>
              <select
                value={(form.pet_online ?? true) ? 'on' : 'off'}
                onChange={(e) => update('pet_online', e.target.value === 'on')}
                className="input w-40"
              >
                <option value="on">开启</option>
                <option value="off">关闭</option>
              </select>
              <p className="text-xs text-slate-400 mt-1">
                问天气时会实时查一次公开数据源（wttr.in，无需账号），
                把结果交给模型再回答。只在这个话题上联网，别的闲聊不会发出请求。
              </p>
            </div>
            <div>
              <label className="label">所在城市</label>
              <input
                type="text"
                value={form.pet_weather_city ?? ''}
                onChange={(e) => update('pet_weather_city', e.target.value || null)}
                placeholder="如 东莞 / Dongguan，留空按 IP 自动定位"
                className="input w-56"
              />
              <p className="text-xs text-slate-400 mt-1">
                建议填上。自动定位依赖出口 IP，走代理或被运营商 NAT 时会定位到别的城市
                （实测本机出口在东莞，数据源却报郑州）。
              </p>
            </div>
            <div>
              <label className="label">形象（素材包）</label>
              <select
                value={form.pet_pack ?? 'builtin'}
                onChange={(e) => update('pet_pack', e.target.value)}
                className="input w-56"
              >
                <option value="builtin">内置形象</option>
                {packs.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}（{p.action_count} 个动作）
                  </option>
                ))}
              </select>
              <p className="text-xs text-slate-400 mt-1">
                自定义形象放在 <code>%APPDATA%\AIWorkAssistant\pets\&lt;id&gt;\</code>
                （manifest.json + videos），包里缺少的动作自动回落内置素材。
              </p>
            </div>
            <p className="text-xs text-amber-600 dark:text-amber-500">
              AI 对话经本机 API 网关生成，会真实消耗账号额度（每日上限 200 次；自动点评 2 分钟节流）。
            </p>
            <p className="text-xs text-slate-400">
              拖到哪记到哪（500ms 去抖后落盘）；双击桌宠可呼出主窗口。
            </p>
          </div>
        </div>
      </section>

      {/* 通知渠道（F-19，Trae/Buddy 全平台共用） */}
      <section className="card p-4">
        <div className="mb-1 flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Bell size={16} className="text-amber-500" />
            <h3 className="font-medium">通知渠道</h3>
          </div>
          <button
            className="btn-outline !px-2.5 !py-1 text-xs"
            disabled={testing || dirty}
            title={dirty ? '表单有未保存的修改，请先点底部「保存设置」再测试' : '按已保存的配置向全部渠道发送测试消息'}
            onClick={() => void sendTest()}
          >
            <Send size={13} /> {testing ? '发送中…' : '发送测试'}
          </button>
        </div>
        <p className="mb-3 text-xs text-slate-400">
          签到完成 / 调度任务失败时推送到手机（Bark / Server酱）或自建 Webhook，Trae 与 Buddy 全平台共用，
          未配置的渠道自动跳过；修改后点底部「保存设置」生效。
        </p>
        <div className="grid gap-x-8 gap-y-3 text-sm sm:grid-cols-2">
          <div className="space-y-3">
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.notify_enabled}
                onChange={(e) => update('notify_enabled', e.target.checked)}
              />
              启用通知推送（总开关）
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.notify_on_checkin}
                onChange={(e) => update('notify_on_checkin', e.target.checked)}
              />
              签到完成时通知
            </label>
            <label className="flex items-center gap-2">
              <input
                type="checkbox"
                checked={form.notify_on_task_fail}
                onChange={(e) => update('notify_on_task_fail', e.target.checked)}
              />
              调度任务失败时通知
            </label>
          </div>
          <div className="space-y-3">
            <div>
              <label className="label">Bark 推送地址</label>
              <input
                type="text"
                value={form.notify_bark_url ?? ''}
                onChange={(e) => update('notify_bark_url', e.target.value.trim() || null)}
                placeholder="https://api.day.app/你的Key"
                className="input"
              />
            </div>
            <div>
              <label className="label">Server酱 SendKey</label>
              <input
                type="text"
                value={form.notify_serverchan_sendkey ?? ''}
                onChange={(e) => update('notify_serverchan_sendkey', e.target.value.trim() || null)}
                placeholder="SCT...（sct.ftqq.com 获取）"
                className="input"
              />
            </div>
            <div>
              <label className="label">通用 Webhook 地址</label>
              <input
                type="text"
                value={form.notify_webhook_url ?? ''}
                onChange={(e) => update('notify_webhook_url', e.target.value.trim() || null)}
                placeholder="https://...（POST JSON，兼容企业微信机器人等自建端）"
                className="input"
              />
            </div>
            <div>
              <label className="label">提醒对象（仅企业微信群机器人）</label>
              <select
                value={form.notify_wechat_mention ?? 'none'}
                onChange={(e) => update('notify_wechat_mention', e.target.value)}
                className="input"
              >
                <option value="none">不提醒任何人</option>
                <option value="all">提醒所有人（@all）</option>
                <option value="some">提醒指定成员</option>
              </select>
              <p className="mt-1 text-xs text-slate-400">
                仅当 Webhook 指向企业微信群机器人时生效；填了其它自建端请保持「不提醒」。
              </p>
            </div>
            {form.notify_wechat_mention === 'some' && (
              <div>
                <label className="label">成员名单</label>
                <input
                  type="text"
                  value={(form.notify_wechat_mention_list ?? []).join(',')}
                  onChange={(e) => {
                    const arr = e.target.value
                      .split(/[,，]/)
                      .map((s) => s.trim())
                      .filter(Boolean);
                    update('notify_wechat_mention_list', arr.length > 0 ? arr : null);
                  }}
                  placeholder="成员 userid 或手机号，多个用逗号分隔"
                  className="input"
                />
                <p className="mt-1 text-xs text-slate-400">
                  填企业微信成员 userid（如 WangQing）或手机号，两种可混填。
                </p>
              </div>
            )}
          </div>
        </div>
      </section>

      {/* 代理相关 */}
      <section className="card p-4">
        <h3 className="mb-1 font-medium">代理</h3>
        <p className="mb-3 text-xs text-slate-400">本地 MITM 代理的端口与抓包行为，修改后需重启代理生效。</p>
        <div className="space-y-3 text-sm">
          <div className="grid gap-3 sm:grid-cols-2">
            <div>
              <label className="label">代理端口</label>
              <input
                type="number"
                value={form.proxy_port}
                onChange={(e) => update('proxy_port', Math.min(65535, Math.max(1, Number(e.target.value) || 8899)))}
                className="input w-32"
                min={1}
                max={65535}
              />
            </div>
            <label className="flex items-center gap-2 self-end pb-2.5">
              <input
                type="checkbox"
                checked={form.auto_start_proxy}
                onChange={(e) => update('auto_start_proxy', e.target.checked)}
              />
              启动时自动开启代理
            </label>
          </div>
          <div>
            <label className="label">代理监听域名列表</label>
            <textarea
              value={form.proxy_domains}
              onChange={(e) => update('proxy_domains', e.target.value)}
              className="input min-h-[60px] text-xs"
              placeholder="trae.cn,trae.com.cn,mchost.guru,zijieapi.com,bytedance.com,volcengine.com,volces.com,treecode.com,doubao.com"
            />
            <p className="mt-1 text-xs text-slate-400">
              逗号分隔的域名后缀列表，即 <b>解密白名单</b>（同 Charles SSL Proxying）：列表内域名走
              MITM 解密并记录日志；未列出的域名透明直通、不记录。注意：做证书锁定（cert pinning）的客户端
              （如豆包 ttnet 原生栈）对其 API 域解密会被拒，应只列入需要抓取凭证/流量的具体域名
              （如 www.doubao.com），不要配宽后缀。留空则使用默认值。
            </p>
          </div>
          <div>
            <label className="label">代理抓取日志路径</label>
            <input
              type="text"
              value={form.proxy_log_path ?? ''}
              onChange={(e) => update('proxy_log_path', e.target.value.trim() || null)}
              placeholder="留空则默认 %APPDATA%\AIWorkAssistant\logs"
              className="input"
            />
            <p className="mt-1 text-xs text-slate-400">
              代理拦截到的完整请求/响应将记录到此目录，按 100MB 滚动存储。
            </p>
          </div>
        </div>
      </section>

      {/* 自我进化 P1：网关观测（只读）——目前只观测、不参与任何路由决策 */}
      {metrics && (
        <section className="card p-4">
          <h3 className="mb-1 font-medium">网关观测</h3>
          <p className="mb-3 text-xs text-slate-400">
            本次启动以来各上游的成功率与延迟（10 秒刷新）。指数滑动值 EWMA 抗尖峰，样本太少时参考意义有限。
          </p>
          <div className="overflow-x-auto">
            <table className="w-full text-left text-xs">
              <thead className="text-slate-400">
                <tr>
                  <th className="py-1 pr-3 font-normal">上游</th>
                  <th className="py-1 pr-3 font-normal">累计成功率</th>
                  <th className="py-1 pr-3 font-normal">近期成功率</th>
                  <th className="py-1 pr-3 font-normal">平均耗时</th>
                  <th className="py-1 pr-3 font-normal">请求数</th>
                  <th className="py-1 font-normal">Tokens</th>
                </tr>
              </thead>
              <tbody>
                {Object.entries(metrics.pools).map(([name, p]) => (
                  <tr key={name} className="border-t border-slate-200/60 dark:border-zinc-700/60">
                    <td className="py-1.5 pr-3">{name}</td>
                    <td className="py-1.5 pr-3">{(p.successRate * 100).toFixed(0)}%</td>
                    <td className="py-1.5 pr-3">{(p.ewmaSuccess * 100).toFixed(0)}%</td>
                    <td className="py-1.5 pr-3">{Math.round(p.ewmaLatencyMs)} ms</td>
                    <td className="py-1.5 pr-3">{p.total}</td>
                    <td className="py-1.5">{p.tokens.toLocaleString()}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      )}

      {/* 保存条 */}
      <div className="flex items-center justify-end gap-2 pb-1">
        {dirty && <span className="mr-auto text-xs text-amber-500">有未保存的更改</span>}
        <button onClick={reset} disabled={!dirty} className="btn-outline disabled:opacity-40">
          <RotateCcw size={15} /> 撤销
        </button>
        <button onClick={save} disabled={saving || !dirty} className="btn-outline disabled:opacity-40">
          <Save size={15} /> {saving ? '保存中…' : '保存设置'}
        </button>
      </div>
    </div>
  );
}
