/**
 * 桌宠互动小游戏：石头剪刀布（P9，纯函数层）
 *
 * 为什么游戏逻辑放这里而不是组件里：判定、连胜、记分都是可单测的纯逻辑，
 * 组件只负责「把状态画出来 + 把事件递进来」——与 `petChatTypes.ts` /
 * `menuPlacement.ts` 的分层方式一致。零 token、零网络：出招是本地随机，
 * AI（pet_say）只负责事后吐槽，不参与判定（判定走 AI 会把回合拖到数秒）。
 *
 * 动作联动（P9 核心）：每一轮结果都映射到既有动作池——
 * 赢 → work-celebrate / 输 → work-sad / 平 → click-shy；
 * 连败 3 次升级 click-tsundere（炸毛），连胜 3 次升级 talent-*（得意表演）。
 * 这样「玩家的操作直接驱动桌宠状态机」，回合间有来有回，像真人互动。
 */

/** 三种出招（展示用 emoji 在 UI 层映射，这里只管语义） */
export type RpsMove = 'rock' | 'paper' | 'scissors';

export const RPS_MOVES: readonly RpsMove[] = ['rock', 'paper', 'scissors'];

/** 一轮对决的结果（pet 视角） */
export type RpsOutcome = 'win' | 'lose' | 'draw';

/** 出招的克制关系：returns 「a 是否赢 b」 */
function beats(a: RpsMove, b: RpsMove): boolean {
  return (
    (a === 'rock' && b === 'scissors') ||
    (a === 'paper' && b === 'rock') ||
    (a === 'scissors' && b === 'paper')
  );
}

/** 单轮判定（pet 视角）；平局 = 同招 */
export function judgeRound(pet: RpsMove, player: RpsMove): RpsOutcome {
  if (pet === player) return 'draw';
  return beats(pet, player) ? 'win' : 'lose';
}

/** 对局累计记分（pet 视角胜/负，平局不计分） */
export type RpsScore = { pet: number; player: number };

/** 连续同侧结果的计数（kind=null 表示刚出过平局/尚未开局） */
export type RpsStreak = { kind: RpsOutcome | null; count: number };

export function applyOutcome(score: RpsScore, outcome: RpsOutcome): RpsScore {
  if (outcome === 'win') return { pet: score.pet + 1, player: score.player };
  if (outcome === 'lose') return { pet: score.pet, player: score.player + 1 };
  return { ...score };
}

/**
 * 连胜/连败连续计数（同侧累计，出平局清零）。
 * 为什么单独算而不是从 score 差值推：3 胜 1 负再 1 胜 = 中间断过连胜，
 * 差值推不出来「当下是否正在连胜」。
 */
export function updateStreak(prev: RpsStreak, outcome: RpsOutcome): RpsStreak {
  if (outcome === 'draw' || prev.kind !== outcome) {
    return { kind: outcome === 'draw' ? null : outcome, count: outcome === 'draw' ? 0 : 1 };
  }
  return { kind: outcome, count: prev.count + 1 };
}

/** 连击阈值：达到即触发升级动作（炸毛 / 得意表演） */
export const STREAK_LEVEL_AT = 3;

/**
 * 一轮结束后的动作语义（`PetTransientKind`），由 PetRoot 播放。
 *
 * 优先级：连击升级 > 单轮结果。玩家视角的输赢与 pet 相反——
 * pet 连败 3 局 = 玩家连胜 3 局 = 桌宠炸毛（click-tsundere）；
 * pet 连胜 3 局 = 桌宠得意（talent-* 随机一个才艺）。
 */
export type RpsReactionAction =
  | 'work-celebrate' // pet 单轮赢
  | 'work-sad' // pet 单轮输
  | 'click-shy' // 平局
  | 'click-tsundere' // pet 连败 3（玩家连胜）
  | 'talent-violin'; // pet 连胜 3：放一段小提琴得意（素材 id 见 manifest.json）

/** 连胜 3 时的得意动作候选（都在 `TALENT_ACTIONS` 内，已过 coverage 校验） */
export const STREAK_SHOWOFF_ACTIONS = ['talent-violin', 'talent-flute', 'talent-maid-curtsy'];

export function reactionAction(outcome: RpsOutcome, streak: RpsStreak): RpsReactionAction {
  if (streak.kind === 'lose' && streak.count >= STREAK_LEVEL_AT) return 'click-tsundere';
  if (streak.kind === 'win' && streak.count >= STREAK_LEVEL_AT) return 'talent-violin';
  if (outcome === 'win') return 'work-celebrate';
  if (outcome === 'lose') return 'work-sad';
  return 'click-shy';
}

/** 结果 → 台词池 key（PetRoot 的 pickLine 消费；连击有专属文案） */
export type RpsLinePool = 'happy' | 'sad' | 'click' | 'tease' | 'talent';

export function linePoolForOutcome(
  outcome: RpsOutcome,
  streak: RpsStreak,
): RpsLinePool {
  if (streak.kind === 'lose' && streak.count >= STREAK_LEVEL_AT) return 'tease';
  if (streak.kind === 'win' && streak.count >= STREAK_LEVEL_AT) return 'talent';
  if (outcome === 'win') return 'happy';
  if (outcome === 'lose') return 'sad';
  return 'click';
}

/**
 * 桌宠出招。刻意**真随机**：不做「克制玩家上一招」的花活——
 * 那种「智能」一旦被玩家察觉就是负体验；真随机 + 输了炸毛才是宠物感。
 */
export function petMove(): RpsMove {
  return RPS_MOVES[Math.floor(Math.random() * RPS_MOVES.length)];
}

/** 玩家出招是否合法（外部输入防御：按钮只发三种，但类型上仍要拦） */
export function isValidMove(m: unknown): m is RpsMove {
  return typeof m === 'string' && (RPS_MOVES as readonly string[]).includes(m);
}
