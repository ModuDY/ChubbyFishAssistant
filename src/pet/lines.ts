/**
 * 本地台词池（P0/P1 完全离线：不花 token、不依赖网关）
 *
 * 取词原则：
 * - 只说"当前发生了什么"，不复述账号名/金额以外的东西（宠物只读事件，不参与业务判断）；
 * - 失败提示不说教、不制造焦虑（签到失败本来就会在页面上有详细信息）；
 * - P3 接网关后，这里退化为"兜底池"（网关未启动 / 无健康账号时使用）。
 */

export type PetLinePool =
  | 'idle'
  | 'working'
  | 'happy'
  | 'sad'
  | 'rich'
  | 'broke'
  | 'sleepy'
  | 'click'
  // ── 2026-10-20：彩蛋按动作类别配台词（吃西瓜时说"水面很平静"太跳戏） ──
  | 'life'
  | 'talent'
  | 'sfx'
  | 'festival'
  | 'mumble'
  // ── 2026-10-20：交互新语义（抚摸 / 被戳烦了） ──
  | 'hover'
  | 'tease';

const LINES: Record<PetLinePool, string[]> = {
  idle: ['摸鱼中…', '有事叫我', '今天也要好好签收', '水面很平静'],
  working: ['正在逐个处理…', '别急，我在跑', '一个接一个来', '叼着任务不松口'],
  happy: ['都搞定啦', '收工！', '这波稳了', '尾巴都摇起来了'],
  sad: ['这个没成功…', '有点不对劲', '再看看日志吧', '唉，翻车了'],
  rich: ['积分到账', '钱袋又鼓了一点', '叮当叮当', '攒着攒着就多了'],
  broke: ['余额见底了…', '空空如也', '要吃土了', '得省着点用'],
  sleepy: ['我先眯一会儿…', '呼…呼…', '有事再叫我', '眼皮有点沉'],
  click: ['在呢', '戳我干嘛', '有事？', '别戳了别戳了', '嗯？'],
  life: ['来点吃的吧', '这个我喜欢', '嘴里有点空', '有点饿了', '就当休息一下'],
  talent: ['看我露一手', '才艺时间到', '献丑了', '鼓掌鼓掌'],
  sfx: ['哇——', '看那边！', '变个戏法', '别眨眼'],
  festival: ['应景一下', '过节就是要有仪式感', '今天得有点不一样', '图个喜庆'],
  mumble: ['这个我得想想…', '嗯…嗯…', '先理一理', '让我琢磨一下'],
  hover: ['呼噜呼噜…', '再摸摸', '好舒服', '别停啊', '就这儿'],
  tease: ['别戳了！', '再戳我生气了', '哼', '住手啦', '戳坏了怎么办'],
};

/** 随机取一句；传入 avoid 可避免连续两次相同 */
export function pickLine(pool: PetLinePool, avoid?: string): string {
  const list = LINES[pool];
  if (list.length === 0) return '';
  if (list.length === 1) return list[0];
  for (let i = 0; i < 6; i += 1) {
    const line = list[Math.floor(Math.random() * list.length)];
    if (line !== avoid) return line;
  }
  return list[0];
}

/** 千分位（气泡里的大数字更好读） */
export function formatAmount(n: number): string {
  if (!Number.isFinite(n)) return '—';
  return n.toLocaleString('zh-CN', { maximumFractionDigits: 0 });
}
