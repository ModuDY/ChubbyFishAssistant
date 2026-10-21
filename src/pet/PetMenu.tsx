/**
 * 桌宠右键菜单（V2）
 *
 * 一张卡片、三个视图（main / actions / packs），**不再用「组折叠 + 内层滚动」**：
 * 106 个动作改为「搜索 + 分类 chips + 扁平列表」，一眼一层。
 * 定位、尺寸、点击穿透都不在这里管——由 `PetRoot` 测量后通过 `placement` 传入。
 */
import { useEffect, useRef, useState } from 'react';
import type { CSSProperties, KeyboardEvent as ReactKeyboardEvent, RefObject } from 'react';
import {
  CalendarCheck,
  Check,
  ChevronLeft,
  ChevronRight,
  EyeOff,
  Layers,
  LayoutGrid,
  MapPin,
  MessageCircle,
  Plug,
  RefreshCw,
  Search,
  Shuffle,
  Sparkles,
  X,
} from 'lucide-react';
import type { PetAiMode } from '../types';
import type { PetAction } from './actions';
import type { PetTransientKind } from './petState';
import { INTERACTIONS } from './interaction';
import { filterActions, groupActions, GROUP_LABELS, recentActions, shortLabel } from './menuModel';
import type { MenuPlacement } from './menuPlacement';

export type PetMenuView = 'main' | 'actions' | 'packs';
export type PackEntry = { id: string; name: string };

export type PetMenuProps = {
  view: PetMenuView;
  /** 由 PetRoot 实测卡片尺寸后算出的落点 */
  placement: MenuPlacement;
  cardRef: RefObject<HTMLDivElement>;
  aiMode: PetAiMode;
  gatewayRunning: boolean;
  actions: PetAction[];
  recents: string[];
  packs: PackEntry[];
  currentPack: string;
  onClose: () => void;
  onViewChange: (view: PetMenuView) => void;
  onCheckin: () => void;
  onChat: () => void;
  onSay: () => void;
  onToggleGateway: () => void;
  onInteract: (kind: PetTransientKind) => void;
  onPlayAction: (id: string) => void;
  onPlayRandom: () => void;
  onPickPack: (id: string) => void;
  onResetPosition: () => void;
  onReload: () => void;
  onHide: () => void;
  /** 请求 Rust 激活窗口：键盘导航与搜索输入需要窗口持有焦点（focus:false 建的窗口不会自动激活） */
  onRequestKeyboard: () => void;
};

export default function PetMenu(props: PetMenuProps) {
  const { view, placement, cardRef } = props;

  // 键盘兜底：方向键在可聚焦条目间移动（搜索框里按 ↓ 也落到列表第一项）
  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    const root = cardRef.current;
    if (!root) return;
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(e.key)) return;
    const items = Array.from(root.querySelectorAll<HTMLElement>('[data-nav]'));
    if (items.length === 0) return;
    e.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLElement);
    let next = current;
    if (e.key === 'ArrowDown') next = current < 0 ? 0 : (current + 1) % items.length;
    else if (e.key === 'ArrowUp') next = current < 0 ? items.length - 1 : (current - 1 + items.length) % items.length;
    else if (e.key === 'Home') next = 0;
    else next = items.length - 1;
    items[next]?.focus();
    items[next]?.scrollIntoView({ block: 'nearest' });
  };

  // 切视图后把焦点交给第一项（动作视图由搜索框自己抢焦点）
  // 80ms 后再聚焦一次：Rust 的 set_focus 是异步的，窗口刚激活时 WebView2 可能重置 DOM 焦点
  useEffect(() => {
    if (view === 'actions') return;
    const focusFirst = () => {
      cardRef.current?.querySelector<HTMLElement>('[data-nav]')?.focus({ preventScroll: true });
    };
    focusFirst();
    const timer = window.setTimeout(focusFirst, 80);
    return () => window.clearTimeout(timer);
  }, [view, cardRef]);

  const style: CSSProperties = {
    left: placement.left,
    top: placement.top,
    transformOrigin: `${placement.flippedX ? 'right' : 'left'} ${placement.flippedY ? 'bottom' : 'top'}`,
  };

  return (
    <div
      className="pet-menu-backdrop"
      onPointerDown={props.onClose}
      onContextMenu={(e) => e.preventDefault()}
    >
      <div
        ref={cardRef}
        className="pet-menu-card"
        style={style}
        role="menu"
        aria-label="桌宠菜单"
        tabIndex={-1}
        onPointerDown={(e) => e.stopPropagation()}
        onContextMenu={(e) => e.preventDefault()}
        onKeyDown={onKeyDown}
      >
        {view === 'main' ? <QuickMenu {...props} /> : null}
        {view === 'actions' ? <ActionPalette {...props} /> : null}
        {view === 'packs' ? <PackAndPosition {...props} /> : null}
      </div>
    </div>
  );
}

/* ------------------------------ L1 快菜单 ------------------------------ */

function QuickMenu(props: PetMenuProps) {
  const { aiMode, gatewayRunning } = props;
  return (
    <div className="pet-menu-body">
      <div className="pet-menu-pills">
        {INTERACTIONS.map((it) => (
          <button
            key={it.id}
            type="button"
            className="pet-menu-pill"
            data-nav
            onClick={() => props.onInteract(it.kind)}
          >
            {it.label}
          </button>
        ))}
      </div>

      {aiMode !== 'off' ? (
        <>
          <button type="button" className="pet-menu-item" data-nav onClick={props.onChat}>
            <MessageCircle className="pet-menu-icon" aria-hidden />
            <span>跟我聊聊…</span>
          </button>
          <button type="button" className="pet-menu-item" data-nav onClick={props.onSay}>
            <Sparkles className="pet-menu-icon" aria-hidden />
            <span>让它自己说一句</span>
          </button>
        </>
      ) : null}

      <button type="button" className="pet-menu-item" data-nav onClick={props.onCheckin}>
        <CalendarCheck className="pet-menu-icon" aria-hidden />
        <span>一键签到</span>
      </button>
      <button type="button" className="pet-menu-item" data-nav onClick={props.onToggleGateway}>
        <Plug className="pet-menu-icon" aria-hidden />
        <span>{gatewayRunning ? '停止 API 网关' : '启动 API 网关'}</span>
        <span className="pet-menu-state">
          <i className={gatewayRunning ? 'is-on' : undefined} />
          {gatewayRunning ? '运行中' : '已停止'}
        </span>
      </button>

      <div className="pet-menu-sep" />

      <button
        type="button"
        className="pet-menu-item is-focal"
        data-nav
        onClick={() => props.onViewChange('actions')}
      >
        <LayoutGrid className="pet-menu-icon" aria-hidden />
        <span>动作点播…</span>
        <ChevronRight className="pet-menu-chevron" aria-hidden />
      </button>
      <button
        type="button"
        className="pet-menu-item"
        data-nav
        onClick={() => props.onViewChange('packs')}
      >
        <Layers className="pet-menu-icon" aria-hidden />
        <span>形象与位置…</span>
        <ChevronRight className="pet-menu-chevron" aria-hidden />
      </button>

      <div className="pet-menu-sep" />

      <button type="button" className="pet-menu-item" data-nav onClick={props.onReload}>
        <RefreshCw className="pet-menu-icon" aria-hidden />
        <span>重新加载</span>
      </button>
      <button type="button" className="pet-menu-item" data-nav onClick={props.onHide}>
        <EyeOff className="pet-menu-icon" aria-hidden />
        <span>隐藏桌宠</span>
      </button>
    </div>
  );
}

/* ----------------------------- L2 动作点播 ----------------------------- */

function ActionPalette(props: PetMenuProps) {
  const [query, setQuery] = useState('');
  const [group, setGroup] = useState('all');
  const inputRef = useRef<HTMLInputElement>(null);

  // 进入动作面板即把焦点交给搜索框：打开就能打字，不必先点一下
  // （Rust 的 set_focus 是异步的，故延迟 60ms 二次聚焦，与 PetChatInput 同款处理）
  useEffect(() => {
    props.onRequestKeyboard();
    const timer = window.setTimeout(() => inputRef.current?.focus({ preventScroll: true }), 60);
    return () => window.clearTimeout(timer);
  }, [props.onRequestKeyboard]);

  const groups = groupActions(props.actions);
  const scoped =
    group === 'recent'
      ? recentActions(props.actions, props.recents)
      : group === 'all'
        ? props.actions.filter((a) => a.url)
        : (groups.find((g) => g.key === group)?.items ?? []);
  const list = filterActions(scoped, query);

  return (
    <>
      <div className="pet-menu-head">
        <button type="button" className="pet-menu-back" data-nav onClick={() => props.onViewChange('main')}>
          <ChevronLeft aria-hidden />
          返回
        </button>
        <span className="pet-menu-title">动作点播</span>
        <span className="pet-menu-count">共 {props.actions.filter((a) => a.url).length}</span>
      </div>

      <div className="pet-menu-search">
        <Search className="pet-menu-icon" aria-hidden />
        <input
          ref={inputRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="搜索动作…"
          aria-label="搜索动作"
          onFocus={props.onRequestKeyboard}
        />
        {query ? (
          <button type="button" className="pet-menu-clear" onClick={() => setQuery('')} aria-label="清空搜索">
            <X aria-hidden />
          </button>
        ) : null}
      </div>

      <div className="pet-menu-chips">
        {props.recents.length > 0 ? (
          <button
            type="button"
            className={`pet-menu-chip ${group === 'recent' ? 'is-on' : ''}`}
            onClick={() => setGroup('recent')}
          >
            最近
          </button>
        ) : null}
        <button
          type="button"
          className={`pet-menu-chip ${group === 'all' ? 'is-on' : ''}`}
          onClick={() => setGroup('all')}
        >
          全部
        </button>
        {groups.map((g) => (
          <button
            key={g.key}
            type="button"
            className={`pet-menu-chip ${group === g.key ? 'is-on' : ''}`}
            onClick={() => setGroup(g.key)}
          >
            {g.label} {g.items.length}
          </button>
        ))}
      </div>

      <div className="pet-menu-list" role="group" aria-label="动作列表">
        {list.map((a) => (
          <button
            key={a.id}
            type="button"
            className="pet-menu-item"
            data-nav
            onClick={() => props.onPlayAction(a.id)}
          >
            <span className="pet-menu-label">{shortLabel(a.label)}</span>
            <span className="pet-menu-tag">{GROUP_LABELS[a.group] ?? a.group}</span>
          </button>
        ))}
        {list.length === 0 ? <p className="pet-menu-hint">没有匹配的动作，换个词试试</p> : null}
      </div>

      <div className="pet-menu-foot">
        <button type="button" className="pet-menu-random" onClick={props.onPlayRandom}>
          <Shuffle aria-hidden />
          随机来一个
        </button>
        <span>{list.length} 个结果</span>
      </div>
    </>
  );
}

/* --------------------------- L2b 形象与位置 --------------------------- */

function PackAndPosition(props: PetMenuProps) {
  return (
    <>
      <div className="pet-menu-head">
        <button type="button" className="pet-menu-back" data-nav onClick={() => props.onViewChange('main')}>
          <ChevronLeft aria-hidden />
          返回
        </button>
        <span className="pet-menu-title">形象与位置</span>
      </div>

      <div className="pet-menu-body">
        <button
          type="button"
          className={`pet-menu-item ${props.currentPack === 'builtin' ? 'is-active' : ''}`}
          data-nav
          onClick={() => props.onPickPack('builtin')}
        >
          {props.currentPack === 'builtin' ? (
            <Check className="pet-menu-icon" aria-hidden />
          ) : (
            <span className="pet-menu-dot" />
          )}
          <span>内置形象</span>
          {props.currentPack === 'builtin' ? <span className="pet-menu-tag">当前</span> : null}
        </button>

        {props.packs.map((p) => (
          <button
            key={p.id}
            type="button"
            className={`pet-menu-item ${props.currentPack === p.id ? 'is-active' : ''}`}
            data-nav
            onClick={() => props.onPickPack(p.id)}
          >
            {props.currentPack === p.id ? (
              <Check className="pet-menu-icon" aria-hidden />
            ) : (
              <span className="pet-menu-dot" />
            )}
            <span>{p.name}</span>
            {props.currentPack === p.id ? <span className="pet-menu-tag">当前</span> : null}
          </button>
        ))}

        {props.packs.length === 0 ? (
          <p className="pet-menu-hint">
            未发现素材包。放到 %APPDATA%\AIWorkAssistant\pets\&lt;id&gt;\ 下即可。
          </p>
        ) : null}

        <div className="pet-menu-sep" />

        <button type="button" className="pet-menu-item" data-nav onClick={props.onResetPosition}>
          <MapPin className="pet-menu-icon" aria-hidden />
          <span>回到初始位置</span>
        </button>
      </div>
    </>
  );
}
