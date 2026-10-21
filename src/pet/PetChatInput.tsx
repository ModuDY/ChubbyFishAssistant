/**
 * 桌宠内联对话输入条（P7，2026-10-21）
 *
 * 定位：浮在角色头顶（气泡位置上方），点开才出现，Esc / 点外部收起。
 *
 * 三条必须在这里守住的约束（都踩过，不是假设）：
 *
 * 1. **点击穿透**：不做处理的话"看得见、点不动"。组件挂载/聚焦时立刻
 *    `api.pet.setChatFocus(true)` 让 Rust 穿透线程暂停切换，卸载时复位。
 * 2. **键盘焦点**：桌宠窗口是 `focus:false` 建的，只 `el.focus()` 收不到按键与 IME。
 *    故聚焦时调用 `api.pet.focusForInput()` 让 Rust `set_focus()` 激活窗口。
 * 3. **IME 组合期**：中文选词时回车是"确认候选词"，此时若发送就会发出半截拼音。
 *    由 `shouldSendOnEnter(key, shift, isComposing)` 短路（`compositionstart/end` 维护状态）。
 *
 * 交互：Enter 发送 / Shift+Enter 换行（输入是单行 input，Shift+Enter 忽略）/ Esc 收起。
 */
import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../lib/tauri';
import { MAX_INPUT_CHARS, shouldSendOnEnter } from './petChatTypes';

type Props = {
  /** 提交一句话（已 trim 且裁剪超长）；由父组件负责清空/关闭 */
  onSend: (text: string) => void;
  /** 收起输入条（Esc / 点外部 / 发送后由父组件决定） */
  onClose: () => void;
  /** 是否禁用（例如网关未启动时仍可打字，但会提示原因；这里用于"正在流式"时避免重复提交） */
  disabled?: boolean;
  /** 占位文案（不同态给不同提示，例如未开网关时提示去启动） */
  placeholder?: string;
};

export default function PetChatInput({ onSend, onClose, disabled, placeholder }: Props) {
  const [value, setValue] = useState('');
  /** IME 是否处于组合期（中文选词阶段）：此时回车是确认候选词，不是发送 */
  const composingRef = useRef(false);
  const inputRef = useRef<HTMLInputElement | null>(null);
  /** 上一次 value 是否为空：用于「空输入按 Esc 直接收起」 */
  const valueRef = useRef('');
  valueRef.current = value;

  /* 挂载即进入对话态；卸载复位（含异常路径，保证不会把穿透永久关掉） */
  useEffect(() => {
    void api.pet.setChatFocus(true).catch(() => {});
    void api.pet.focusForInput().catch(() => {});
    // 二次聚焦：Rust set_focus 是异步的，窗口刚激活时 WebView2 可能还没把焦点交给 DOM
    const timer = window.setTimeout(() => {
      inputRef.current?.focus();
    }, 60);
    return () => {
      window.clearTimeout(timer);
      void api.pet.setChatFocus(false).catch(() => {});
      void api.pet.focusForInput().catch(() => {});
    };
  }, []);

  const submit = useCallback(() => {
    const text = value.trim().slice(0, MAX_INPUT_CHARS);
    if (!text || disabled) return;
    onSend(text);
    setValue('');
    // 发送后立刻把焦点还给输入框：多轮对话要能"问完接着问"。
    // 父组件不再卸载本组件（保持打开），故这里只需销回焦点。
    inputRef.current?.focus();
  }, [value, disabled, onSend]);

  /**
   * 失焦自动收起（有护栏）：
   * 用户点角色/别处时输入框会失焦，若不收起就会一直"挂"在头顶。
   *
   * 但**不能**无条件收起——点「发送」按钮的瞬间 input 也会先失焦，
   * 那样会在提交前把输入条关掉。故用 `relatedTarget` 判断：焦点落回本组件内则忽略。
   * 另留 200ms 缓冲，覆盖"失焦后立刻又被 Rust 聚焦回来"的时序。
   */
  const onBlur = (e: React.FocusEvent<HTMLInputElement>) => {
    const next = e.relatedTarget as Node | null;
    const root = e.currentTarget.closest('.pet-chat');
    if (next && root?.contains(next)) return;
    window.setTimeout(() => {
      // 若这期间又聚焦回来了（发送后 focus()），就不收起
      if (document.activeElement?.closest('.pet-chat')) return;
      onClose();
    }, 200);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape') {
      e.preventDefault();
      // 有内容时先清空（避免误按丢失已打好的字），空输入才真的收起
      if (valueRef.current) setValue('');
      else onClose();
      return;
    }
    // IME 组合期回车用于选词，绝不能当发送
    if (shouldSendOnEnter(e.key, e.shiftKey, composingRef.current || e.nativeEvent.isComposing)) {
      e.preventDefault();
      submit();
      return;
    }
    // Shift+Enter：单行输入框里没有换行语义，明确吞掉，避免触发外层快捷键
    if (e.key === 'Enter' && e.shiftKey) e.preventDefault();
  };

  return (
    <div className="pet-chat" onPointerDown={(e) => e.stopPropagation()}>
      <input
        ref={inputRef}
        className="pet-chat-input"
        value={value}
        maxLength={MAX_INPUT_CHARS}
        placeholder={placeholder ?? '跟大肥鱼说点什么…'}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={onBlur}
        onCompositionStart={() => {
          composingRef.current = true;
        }}
        onCompositionEnd={() => {
          composingRef.current = false;
        }}
        spellCheck={false}
        autoComplete="off"
      />
      <button
        type="button"
        className="pet-chat-send"
        disabled={disabled || !value.trim()}
        onPointerDown={(e) => e.stopPropagation()}
        onClick={submit}
        title="发送（Enter）"
      >
        发送
      </button>
    </div>
  );
}
