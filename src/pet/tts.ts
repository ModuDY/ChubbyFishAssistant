/**
 * 桌宠语音（P11）纯函数层：朗读文本清洗 / 长度口径 / 音色挑选 / 权限判定。
 *
 * 为什么单独成文件：这些规则要能单测，且设置页（试听、音色下拉）与运行时
 * 仲裁层必须用**同一套**判定——否则会出现"设置页说能读、实际不读"。
 * 本模块不碰 `speechSynthesis`，也不读取任何 React 状态。
 */

/** 语音来源：只有前三类可能被朗读，后两类第一版只显示 */
export type SpeechSource = 'manual' | 'auto' | 'proactive' | 'notice' | 'local';

/** 打断优先级：数字越大越优先 */
export const SPEECH_PRIORITY: Record<SpeechSource, number> = {
  notice: 4,
  manual: 3,
  proactive: 2,
  auto: 1,
  local: 0,
};

/** 每类来源的朗读上限（汉字）。用户提问的答复可以长一点，自动内容必须短 */
export const SPEECH_MAX_CHARS: Record<SpeechSource, number> = {
  manual: 120,
  auto: 60,
  proactive: 60,
  notice: 0,
  local: 0,
};

export type SpeechFlags = {
  enabled: boolean;
  auto: boolean;
  proactive: boolean;
};

/**
 * 是否允许朗读某类来源。
 *
 * 第一版口径（与设计稿一致）：用户主动对话在总开关打开后可读；
 * 自动点评与时间问候各自需要单独允许；通知与本地台词/彩蛋只显示。
 */
export function speechAllowed(source: SpeechSource, flags: SpeechFlags): boolean {
  if (!flags.enabled) return false;
  if (source === 'manual') return true;
  if (source === 'auto') return flags.auto;
  if (source === 'proactive') return flags.proactive;
  return false;
}

/**
 * 清理成"适合朗读"的文本：
 * - 去 Markdown 代码块 / 行内代码 / 链接 / 标题与强调符号；
 * - 保留中文标点（句读决定停顿），压掉连续空白；
 * - 去掉常见的纯英文标识（URL、路径、长串 ID）——念出来全是字母噪音。
 */
export function sanitizeForSpeech(raw: string): string {
  let s = raw;
  // 代码块整体丢弃：读出来是 "const a 等于..."，听众只会更困惑
  s = s.replace(/```[\s\S]*?```/g, ' ');
  s = s.replace(/`([^`]*)`/g, '$1');
  // 链接只留文字，URL 本身不念
  s = s.replace(/\[([^\]]*)\]\((?:[^)]*)\)/g, '$1');
  s = s.replace(/\bhttps?:\/\/\S+/gi, ' ');
  // Markdown 标记
  s = s.replace(/^#{1,6}\s*/gm, '');
  s = s.replace(/[*_~>#]+/g, '');
  // 表格分隔行 / 列表符号
  s = s.replace(/^\s*[-+*]\s+/gm, '');
  s = s.replace(/^\s*\|.*\|\s*$/gm, ' ');
  // 过长的英文/数字串（版本号、哈希、路径）不念
  s = s.replace(/[A-Za-z0-9_\-./\\:]{12,}/g, ' ');
  s = s.replace(/\s+/g, ' ').trim();
  return s;
}

/** 是否值得朗读：空文本 / 纯符号 / 没有中文也没有字母的，直接跳过 */
export function isSpeakableText(text: string): boolean {
  if (!text) return false;
  return /[\u4e00-\u9fa5A-Za-z]/.test(text);
}

/** 按来源截断（不追加"后面不念了"这类元话术） */
export function clipForSpeech(text: string, source: SpeechSource): string {
  const max = SPEECH_MAX_CHARS[source];
  if (max <= 0) return '';
  const chars = Array.from(text);
  return chars.length <= max ? text : chars.slice(0, max).join('');
}

/** 清洗 + 判空 + 截断，一步到位；返回空串表示不该读 */
export function prepareSpeech(raw: string, source: SpeechSource): string {
  const cleaned = sanitizeForSpeech(raw);
  if (!isSpeakableText(cleaned)) return '';
  return clipForSpeech(cleaned, source);
}

/** 本地中文音色（`localService` 才不联网；云端音色在没有网络时会静默失败） */
export function localChineseVoices(voices: readonly SpeechSynthesisVoice[]): SpeechSynthesisVoice[] {
  return voices.filter((v) => /^zh/i.test(v.lang) && v.localService);
}

/**
 * 挑音色：优先用户保存的 voiceURI；失效时回落第一个本地中文音色；都没有 → null。
 * 返回 null 时调用方应"不读"而不是报错（精简版 Windows 真的可能没有中文语音包）。
 */
export function pickVoice(
  voices: readonly SpeechSynthesisVoice[],
  savedUri?: string | null,
): SpeechSynthesisVoice | null {
  const local = localChineseVoices(voices);
  if (savedUri) {
    const hit = local.find((v) => v.voiceURI === savedUri);
    if (hit) return hit;
  }
  return local[0] ?? null;
}
