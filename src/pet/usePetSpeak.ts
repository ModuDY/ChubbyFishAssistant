/**
 * 桌宠语音仲裁层（P11）：所有朗读请求的唯一入口。
 *
 * 为什么不是"监听 pet-say-done 直接读"：桌宠还有本地台词、自动点评失败回退、
 * 通知气泡和主动问候。若各写各的，会出现"看得见却听不到"或"通知到了上一句还在响"。
 * 这里统一决定：能不能读、要不要打断、读多长。
 *
 * 两条发声通道：
 * - 在线 Edge 神经语音（P11 增补）：音质好，需联网；合成失败自动回退本地；
 * - 系统 `speechSynthesis`：离线兜底，音质一般。
 *
 * 硬约束：
 * - 同一时刻最多一个 utterance / 一段音频，不排队播放已经过期的内容；
 * - 在线合成是异步的：用递增序号丢弃"合成回来时已经过期"的结果；
 * - 关闭总开关时**不加载音色**（验收标准 1：关着的时候任何迹象都没有）；
 * - 没有本地中文音色且在线失败时静默降级，不报错、不弹窗、不刷日志。
 */
import { useCallback, useEffect, useRef } from 'react';
import { api } from '../lib/tauri';
import {
  DEFAULT_VOICE_ID,
  SPEECH_PRIORITY,
  isEdgeVoiceId,
  pickVoice,
  prepareSpeech,
  speechAllowed,
  type SpeechFlags,
  type SpeechSource,
} from './tts';
import { isQuietHour } from './flavor';

type Options = {
  flags: SpeechFlags;
  /** 用户选择的音色 ID（Edge 白名单 ID / 本地 voiceURI；空 = 在线默认音色） */
  voiceUri?: string | null;
  rate: number;
  volume: number;
  /** 隐藏 / 全屏收起 / 拖拽 / 输入或对局中：一律停声且不开始新的朗读 */
  blocked: boolean;
};

/** 把 Rust 返回的 base64 MP3 转成 Blob 需要的 ArrayBuffer */
function base64ToBuffer(b64: string): ArrayBuffer {
  const bin = atob(b64);
  const buffer = new ArrayBuffer(bin.length);
  const out = new Uint8Array(buffer);
  for (let i = 0; i < bin.length; i += 1) out[i] = bin.charCodeAt(i);
  return buffer;
}

function quietNow(): boolean {
  return isQuietHour(new Date().getHours());
}

export function usePetSpeak(opts: Options) {
  const optsRef = useRef(opts);
  optsRef.current = opts;

  const voicesRef = useRef<SpeechSynthesisVoice[]>([]);
  const voicesReadyRef = useRef(false);
  /** 当前正在播的来源优先级；0 = 没在播 */
  const currentRef = useRef(0);
  const utteranceRef = useRef<SpeechSynthesisUtterance | null>(null);
  /** 在线合成的请求序号：只允许最后一次请求真正播放 */
  const seqRef = useRef(0);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const audioUrlRef = useRef<string | null>(null);

  /** 停掉在线音频并释放 Blob URL（先摘事件，避免 stop 触发 onerror 又走回退） */
  const stopAudio = useCallback(() => {
    const audio = audioRef.current;
    audioRef.current = null;
    if (audio) {
      audio.onended = null;
      audio.onerror = null;
      try {
        audio.pause();
      } catch {
        /* 某些 WebView 在未播放状态 pause 会抛，忽略 */
      }
      audio.removeAttribute('src');
    }
    if (audioUrlRef.current) {
      URL.revokeObjectURL(audioUrlRef.current);
      audioUrlRef.current = null;
    }
  }, []);

  // 音色懒加载：开着总开关才枚举；首次 getVoices() 常为空，必须等 voiceschanged。
  // 3 秒兜底：某些环境该事件永不触发，不能让本地回退死等。
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

  /** 立刻停声（窗口隐藏、拖拽、输入、对局、通知抢占、组件卸载都走这里） */
  const cancel = useCallback(() => {
    // 序号自增 = 作废所有在途合成；回来时看到序号不一致就直接丢弃
    seqRef.current += 1;
    stopAudio();
    if (typeof window.speechSynthesis !== 'undefined') window.speechSynthesis.cancel();
    currentRef.current = 0;
    utteranceRef.current = null;
  }, [stopAudio]);

  // 进入"别打扰"态：主动停掉，恢复可见后也不续播旧句子
  useEffect(() => {
    if (opts.blocked) cancel();
  }, [opts.blocked, cancel]);

  useEffect(() => {
    return () => {
      seqRef.current += 1;
      stopAudio();
      if (typeof window.speechSynthesis !== 'undefined') window.speechSynthesis.cancel();
    };
  }, [stopAudio]);

  /** 本地通道：系统语音包。返回 false 表示没有可用音色 */
  const speakLocal = useCallback((text: string, priority: number, voiceUri?: string | null) => {
    if (typeof window.speechSynthesis === 'undefined') return false;
    const o = optsRef.current;
    const voice = pickVoice(voicesRef.current, voiceUri ?? o.voiceUri);
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
  }, []);

  /** 在线音频播放结束后复位（只复位自己这一轮） */
  const finishAudio = useCallback((audio: HTMLAudioElement, url: string) => {
    if (audioRef.current === audio) audioRef.current = null;
    if (audioUrlRef.current === url) {
      URL.revokeObjectURL(url);
      audioUrlRef.current = null;
    }
    currentRef.current = 0;
  }, []);

  /**
   * 提交一次朗读请求。
   *
   * 返回 true 表示请求已被受理（在线通道此时还在合成，不代表已经在响）。
   * 低优先级内容不能打断正在播的高优先级内容；同级或更高则先 cancel 再播。
   */
  const speak = useCallback(
    (source: SpeechSource, raw: string) => {
      const o = optsRef.current;
      if (!speechAllowed(source, o.flags)) return false;
      if (o.blocked) return false;
      // 深夜 23–5 点整体静默（与主动开口同一口径，用户主动对话也不读）
      if (quietNow()) return false;
      if (typeof window.speechSynthesis === 'undefined' && !isEdgeVoiceId(o.voiceUri ?? DEFAULT_VOICE_ID)) {
        return false;
      }

      const text = prepareSpeech(raw, source);
      if (!text) return false;

      const priority = SPEECH_PRIORITY[source];
      // 正在播更高优先级：让位（例如用户回答期间来了自动点评）
      if (currentRef.current > priority) return false;

      const voiceId = o.voiceUri?.trim() || DEFAULT_VOICE_ID;
      const edge = isEdgeVoiceId(voiceId);
      // 本地通道必须等音色加载完成；在线通道不依赖系统语音包
      if (!edge && !voicesReadyRef.current) return false;

      const seq = seqRef.current + 1;
      seqRef.current = seq;
      stopAudio();
      if (typeof window.speechSynthesis !== 'undefined') window.speechSynthesis.cancel();
      currentRef.current = priority;

      // 断网时直接走本地，省掉一次必然失败的 5s 连接超时
      if (edge && typeof navigator !== 'undefined' && navigator.onLine === false) {
        if (!speakLocal(text, priority)) {
          currentRef.current = 0;
          return false;
        }
        return true;
      }

      if (edge) {
        // 异步合成：回来后要重新校验状态，过期结果直接丢弃
        void (async () => {
          try {
            const b64 = await api.pet.ttsSynth(text, voiceId, o.rate);
            if (seqRef.current !== seq) return;
            const now = optsRef.current;
            if (now.blocked || quietNow() || !speechAllowed(source, now.flags)) {
              if (seqRef.current === seq) currentRef.current = 0;
              return;
            }
            const bytes = base64ToBuffer(b64);
            const url = URL.createObjectURL(new Blob([bytes], { type: 'audio/mpeg' }));
            const audio = new Audio(url);
            audio.volume = Math.min(1, Math.max(0, now.volume));
            audio.onended = () => {
              finishAudio(audio, url);
            };
            audio.onerror = () => {
              // 浏览器解码失败：退回本地，保证"能看见的话尽量也能听见"
              finishAudio(audio, url);
              if (seqRef.current === seq) speakLocal(text, priority);
            };
            audioRef.current = audio;
            audioUrlRef.current = url;
            await audio.play();
          } catch {
            if (seqRef.current !== seq) return;
            const now = optsRef.current;
            if (now.blocked || quietNow()) {
              currentRef.current = 0;
              return;
            }
            // 在线合成失败（断网 / 微软协议变更 / 超时）→ 本地静默兜底
            if (!speakLocal(text, priority)) currentRef.current = 0;
          }
        })();
        return true;
      }

      if (!speakLocal(text, priority)) {
        currentRef.current = 0;
        return false;
      }
      return true;
    },
    [finishAudio, speakLocal, stopAudio],
  );

  return { speak, cancel };
}

// ---- 设置页试听：不经过仲裁，直接播一句固定文本 ----
const PREVIEW_TEXT = '你好，我是大肥鱼，今天也在这儿陪你。';
let previewAudio: HTMLAudioElement | null = null;
let previewUrl: string | null = null;
let previewSeq = 0;

function stopPreview() {
  previewSeq += 1;
  if (previewAudio) {
    previewAudio.onended = null;
    previewAudio.onerror = null;
    try {
      previewAudio.pause();
    } catch {
      /* 忽略 */
    }
    previewAudio.removeAttribute('src');
    previewAudio = null;
  }
  if (previewUrl) {
    URL.revokeObjectURL(previewUrl);
    previewUrl = null;
  }
}

/**
 * 设置页「试听」：在线音色走 Edge，本地音色走 speechSynthesis。
 * 失败时 reject（由设置页 toast 展示），不影响桌宠运行。
 */
export async function previewVoice(
  voiceId: string | null,
  rate: number,
  volume: number,
): Promise<boolean> {
  stopPreview();
  const id = voiceId?.trim() || DEFAULT_VOICE_ID;
  const seq = previewSeq;

  if (isEdgeVoiceId(id)) {
    const b64 = await api.pet.ttsSynth(PREVIEW_TEXT, id, rate);
    if (seq !== previewSeq) return false;
    const bytes = base64ToBuffer(b64);
    const url = URL.createObjectURL(new Blob([bytes], { type: 'audio/mpeg' }));
    const audio = new Audio(url);
    previewAudio = audio;
    previewUrl = url;
    audio.volume = Math.min(1, Math.max(0, volume));
    audio.onended = () => {
      if (previewAudio === audio) stopPreview();
    };
    await audio.play();
    return true;
  }

  if (typeof window.speechSynthesis === 'undefined') return false;
  const voice = pickVoice(window.speechSynthesis.getVoices(), voiceId);
  if (!voice) return false;
  const synth = window.speechSynthesis;
  synth.cancel();
  const utter = new SpeechSynthesisUtterance(PREVIEW_TEXT);
  utter.voice = voice;
  utter.lang = voice.lang;
  utter.rate = Math.min(1.4, Math.max(0.8, rate));
  utter.volume = Math.min(1, Math.max(0, volume));
  synth.speak(utter);
  return true;
}
