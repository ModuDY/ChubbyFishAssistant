import { describe, expect, it } from 'vitest';
import {
  SPEECH_MAX_CHARS,
  clipForSpeech,
  isSpeakableText,
  localChineseVoices,
  pickVoice,
  prepareSpeech,
  sanitizeForSpeech,
  speechAllowed,
} from './tts';

function voice(name: string, lang: string, local: boolean): SpeechSynthesisVoice {
  return { name, lang, localService: local, voiceURI: `uri:${name}` } as SpeechSynthesisVoice;
}

const ALL_FLAGS = { enabled: true, auto: true, proactive: true };
const NO_FLAGS = { enabled: false, auto: false, proactive: false };

describe('speechAllowed', () => {
  it('总开关关闭时任何来源都不读', () => {
    expect(speechAllowed('manual', NO_FLAGS)).toBe(false);
    expect(speechAllowed('auto', NO_FLAGS)).toBe(false);
    expect(speechAllowed('proactive', NO_FLAGS)).toBe(false);
  });

  it('总开关打开后仅用户对话默认可读', () => {
    const flags = { enabled: true, auto: false, proactive: false };
    expect(speechAllowed('manual', flags)).toBe(true);
    expect(speechAllowed('auto', flags)).toBe(false);
    expect(speechAllowed('proactive', flags)).toBe(false);
  });

  it('自动点评与时间问候需要各自单独允许', () => {
    expect(speechAllowed('auto', ALL_FLAGS)).toBe(true);
    expect(speechAllowed('proactive', ALL_FLAGS)).toBe(true);
  });

  it('通知、本地台词与彩蛋第一版永不朗读', () => {
    expect(speechAllowed('notice', ALL_FLAGS)).toBe(false);
    expect(speechAllowed('local', ALL_FLAGS)).toBe(false);
  });
});

describe('sanitizeForSpeech', () => {
  it('去掉代码块、行内代码与链接地址', () => {
    const raw = '看这个 ```js\nconst a = 1\n``` 还有 `x` 和 [文档](https://example.com/a)';
    const out = sanitizeForSpeech(raw);
    expect(out).not.toContain('const');
    expect(out).not.toContain('example.com');
    expect(out).toContain('文档');
  });

  it('去掉 Markdown 标题与强调符号', () => {
    expect(sanitizeForSpeech('## 今天**很好**呀')).toBe('今天很好呀');
  });

  it('压掉连续空白并去掉长英文串', () => {
    const out = sanitizeForSpeech('你好   a1b2c3d4e5f6g7h8  今天');
    expect(out).not.toMatch(/\s{2,}/);
    expect(out).not.toContain('a1b2c3d4e5f6g7h8');
    expect(out).toContain('你好');
  });
});

describe('prepareSpeech', () => {
  it('空文本与纯符号不读', () => {
    expect(isSpeakableText('')).toBe(false);
    expect(isSpeakableText('--- ***')).toBe(false);
    expect(prepareSpeech('   ', 'manual')).toBe('');
  });

  it('用户对话最多 120 字，自动内容最多 60 字', () => {
    const long = '鱼'.repeat(200);
    expect(clipForSpeech(long, 'manual')).toHaveLength(SPEECH_MAX_CHARS.manual);
    expect(clipForSpeech(long, 'auto')).toHaveLength(SPEECH_MAX_CHARS.auto);
    expect(clipForSpeech(long, 'notice')).toBe('');
    expect(clipForSpeech(long, 'local')).toBe('');
  });

  it('短句原样保留中文标点', () => {
    expect(prepareSpeech('早呀，今天想干点什么？', 'proactive')).toBe('早呀，今天想干点什么？');
  });
});

describe('pickVoice', () => {
  const huihui = voice('Microsoft Huihui', 'zh-CN', true);
  const kangkang = voice('Microsoft Kangkang', 'zh-CN', true);
  const cloud = voice('Cloud Chinese', 'zh-CN', false);
  const english = voice('Microsoft David', 'en-US', true);

  it('只留本地中文音色', () => {
    expect(localChineseVoices([huihui, cloud, english])).toEqual([huihui]);
  });

  it('优先保存的 voiceURI', () => {
    expect(pickVoice([huihui, kangkang], 'uri:Microsoft Kangkang')).toBe(kangkang);
  });

  it('保存的音色失效时回落第一个本地中文音色', () => {
    expect(pickVoice([english, cloud, kangkang], 'uri:missing')).toBe(kangkang);
  });

  it('没有本地中文音色时返回 null（调用方静默降级）', () => {
    expect(pickVoice([cloud, english])).toBeNull();
    expect(pickVoice([])).toBeNull();
  });
});
