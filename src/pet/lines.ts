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
  | 'click';

const LINES: Record<PetLinePool, string[]> = {
  idle: ['摸鱼中…', '有事叫我', '今天也要好好签收', '水面很平静'],
  working: ['正在逐个处理…', '别急，我在跑', '一个接一个来', '叼着任务不松口'],
  happy: ['都搞定啦', '收工！', '这波稳了', '尾巴都摇起来了'],
  sad: ['这个没成功…', '有点不对劲', '再看看日志吧', '唉，翻车了'],
  rich: ['积分到账', '钱袋又鼓了一点', '叮当叮当', '攒着攒着就多了'],
  broke: ['余额见底了…', '空空如也', '要吃土了', '得省着点用'],
  sleepy: ['我先眯一会儿…', '呼…呼…', '有事再叫我', '眼皮有点沉'],
  click: ['在呢', '戳我干嘛', '有事？', '别戳了别戳了', '嗯？'],
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
