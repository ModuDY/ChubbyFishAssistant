/**
 * 桌宠语音仲裁层（P11）：所有朗读请求的唯一入口。
 *
 * 为什么不是"监听 pet-say-done 直接读"：桌宠还有本地台词、自动点评失败回退、
 * 通知气泡和主动问候。若各写各的，会出现"看得见却听不到"或"通知到了上一句还在响"。
 * 这里统一决定：能不能读、要不要打断、读多长。
 *
 * 硬约束：
 * - 同一时刻最多一个 utterance，不排队播放已经过期的内容；
 * - 关闭总开关时**不加载音色**（验收标准 1：关着的时候任何迹象都没有）；
 * - 没有本地中文音色时静默降级，不报错、不弹窗、不刷日志。
 */
import { useCallback, useEffect, useRef } from 'react';
import {
  SPEECH_PRIORITY,
  pickVoice,
  prepareSpeech,
  speechAllowed,
  type SpeechFlags,
  type SpeechSource,
} from './tts';
import { isQuietHour } from './flavor';

type Options = {
  flags: SpeechFlags;
  /** 用户选择的 voiceURI（空 = 自动取第一个本地中文音色） */
  voiceUri?: string | null;
  rate: number;
  volume: number;
  /** 隐藏 / 全屏收起 / 拖拽 / 输入或对局中：一律停声且不开始新的朗读 */
  blocked: boolean;
};

export function usePetSpeak(opts: Options) {
  const optsRef = useRef(opts);
  optsRef.current = opts;

  const voicesRef = useRef<SpeechSynthesisVoice[]>([]);
  const voicesReadyRef = useRef(false);
  /** 当前正在播的来源优先级；0 = 没在播 */
  const currentRef = useRef(0);
  const utteranceRef = useRef<SpeechSynthesisUtterance | null>(null);

  // 音色懒加载：开着总开关才枚举；首次 getVoices() 常为空，必须等 voiceschanged。
  // 3 秒兜底：某些环境该事件永不触发，不能让朗读功能死等。
  useEffect(() => {
    if (!opts.flags.enabled || typeof window.speechSynthesis === 'undefined') return;
    const synth = window.speechSynthesis;
    let stopped = false;
    let timer: number | undefined;

    const load = () => {
      if (stopped) return;
      const list = synth.getVoices();
      if (list.length === 0) return;
      voicesRef.current = list;
      voicesReadyRef.current = true;
      if (timer) window.clearTimeout(timer);
    };

    load();
    synth.addEventListener('voiceschanged', load);
    timer = window.setTimeout(load, 3000);

    return () => {
      stopped = true;
      if (timer) window.clearTimeout(timer);
      synth.removeEventListener('voiceschanged', load);
      synth.cancel();
      currentRef.current = 0;
      utteranceRef.current = null;
    };
  }, [opts.flags.enabled]);

  /** 立刻停声（切 PCM 输出设备、窗口隐藏、通知抢占、组件卸载都走这里） */
  const cancel = useCallback(() => {
    if (typeof window.speechSynthesis === 'undefined') return;
    window.speechSynthesis.cancel();
    currentRef.current = 0;
    utteranceRef.current = null;
  }, []);

  // 进入"别打扰"态：主动停掉，恢复可见后也不续播旧句子
  useEffect(() => {
    if (opts.blocked) cancel();
  }, [opts.blocked, cancel]);

  /**
   * 提交一次朗读请求。
   *
   * 返回 true 表示真的开始播了（调用方一般不需要用它）。
   * 低优先级内容不能打断正在播的高优先级内容；同级或更高则先 cancel 再播。
   */
  const speak = useCallback(
    (source: SpeechSource, raw: string) => {
      const o = optsRef.current;
      if (!speechAllowed(source, o.flags)) return false;
      if (o.blocked) return false;
      // 深夜 23–5 点整体静默（与主动开口同一口径，用户主动对话也不读）
      if (isQuietHour(new Date().getHours())) return false;
      if (typeof window.speechSynthesis === 'undefined') return false;
      if (!voicesReadyRef.current || voicesRef.current.length === 0) return false;

      const text = prepareSpeech(raw, source);
      if (!text) return false;

      const priority = SPEECH_PRIORITY[source];
      // 正在播更高优先级：让位（例如用户回答期间来了自动点评）
      if (currentRef.current > priority) return false;

      const voice = pickVoice(voicesRef.current, o.voiceUri);
      if (!voice) return false;

      const synth = window.speechSynthesis;
      synth.cancel();

      const utter = new SpeechSynthesisUtterance(text);
      utter.voice = voice;
      utter.lang = voice.lang;
      utter.rate = Math.min(1.4, Math.max(0.8, o.rate));
      utter.volume = Math.min(1, Math.max(0, o.volume));
      const reset = () => {
        // 只清自己这一轮：新的朗读可能已经接手，别把它的状态抹掉
        if (utteranceRef.current === utter) {
          utteranceRef.current = null;
          currentRef.current = 0;
        }
      };
      utter.onend = reset;
      utter.onerror = reset;
      utteranceRef.current = utter;
      currentRef.current = priority;
      synth.speak(utter);
      return true;
    },
    [],
  );

  return { speak, cancel };
}

/** 设置页「试听」用：不经过仲裁，直接读一句固定文本（失败静默） */
export function previewSpeech(voiceUri: string | null, rate: number, volume: number): boolean {
  if (typeof window.speechSynthesis === 'undefined') return false;
  const voice = pickVoice(window.speechSynthesis.getVoices(), voiceUri);
  if (!voice) return false;
  const synth = window.speechSynthesis;
  synth.cancel();
  const utter = new SpeechSynthesisUtterance('你好，我是大肥鱼，今天也在这儿陪你。');
  utter.voice = voice;
  utter.lang = voice.lang;
  utter.rate = Math.min(1.4, Math.max(0.8, rate));
  utter.volume = Math.min(1, Math.max(0, volume));
  synth.speak(utter);
  return true;
}
