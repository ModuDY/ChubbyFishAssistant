import { useState } from 'react';
import { CornerDownLeft } from 'lucide-react';
import { useAppStore } from '../../../store';
import { parseCapture } from '../../../lib/workbenchTodos';

/**
 * 区块⑤：快速捕获。
 *
 * 前缀语法（方案 §4.2）：
 * - `?查询` → 问知识库（P1 实现后可用；当前如实提示未实现，不假装能答）
 * - `/命令` → 走命令（P2 工作流接入）
 * - 其余   → 存为笔记（P1 知识库 kb_notes 落地后可用）
 *
 * P0 阶段的定位是**把入口先立起来**：用户输入的意图能被正确识别，
 * 并且在能力未落地时给出明确说明，而不是提交后静默丢失。
 */
export default function TaskCaptureWidget() {
  const pushToast = useAppStore((s) => s.pushToast);
  const [text, setText] = useState('');

  const submit = () => {
    const intent = parseCapture(text);
    if (!intent) return;
    switch (intent.kind) {
      case 'ask':
        // 知识库是 P1，当前如实告知，避免用户以为存进去了
        pushToast('info', `「问知识库」将在知识库功能（P1）落地后可用：${intent.text}`);
        break;
      case 'command':
        pushToast('info', `命令执行将在工作流（P2）落地后可用：${intent.text}`);
        break;
      case 'note':
        pushToast('info', `笔记存档将在知识库（P1）落地后可用：${intent.text.slice(0, 40)}`);
        break;
    }
    setText('');
  };

  return (
    <div className="space-y-2">
      <div className="relative">
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            // Enter 提交；Shift+Enter 留给将来的多行笔记（当前单行，先不处理）
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder="记点什么…"
          className="input !pr-9 text-sm"
          aria-label="快速捕获"
        />
        <button
          onClick={submit}
          disabled={!text.trim()}
          className="absolute right-1.5 top-1/2 flex h-6 w-6 -translate-y-1/2 items-center justify-center rounded-md text-slate-400 transition hover:bg-slate-100 hover:text-slate-600 disabled:opacity-30 dark:hover:bg-zinc-800"
          aria-label="提交"
          title="回车提交"
        >
          <CornerDownLeft size={13} />
        </button>
      </div>
      <ul className="space-y-1 text-[11px] text-slate-400 dark:text-zinc-500">
        <li>
          <span className="font-mono text-slate-500 dark:text-zinc-400">?</span> 开头 · 问知识库
        </li>
        <li>
          <span className="font-mono text-slate-500 dark:text-zinc-400">/</span> 开头 · 执行命令
        </li>
        <li>其余内容 · 存为笔记</li>
      </ul>
    </div>
  );
}
