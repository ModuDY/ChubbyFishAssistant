/**
 * 右键菜单定位（纯函数，便于单测）
 *
 * 旧实现只用角色包围盒算「右侧固定槽位」，右键点在哪都一样，
 * 且用 `MENU_H = 400` 估值做垂直夹紧——窗口高只有 `360 × scale`，夹紧恒等于 4，
 * 卡片顶到窗口上沿、底部被裁（详见 `docs/pending/桌宠右键菜单重设计方案.md` §1）。
 *
 * 这里改成：
 * 1. 以**光标**为锚点，默认落在右下；
 * 2. 右/下空间不足时向另一侧翻转；
 * 3. 最后夹紧在窗口内（尺寸用实测值，不再估值）；
 * 4. 只要与角色包围盒重叠，整体换到角色另一侧。
 */

export type Point = { x: number; y: number };
export type Size = { width: number; height: number };
export type Rect = { left: number; top: number; width: number; height: number };

/** 卡片与窗口边缘的最小留白（px） */
export const MENU_EDGE = 8;
/** 卡片与光标之间的间距（px）；翻转后同样保持该间距 */
export const MENU_OFFSET = 6;
/**
 * 卡片与角色包围盒之间的最小间距（px）。
 *
 * 曾经按「重叠面积 > 卡片面积 30% 才换边」判定，但角色是**竖条**、卡片也是竖卡，
 * 一条边切进去 66px 只占卡片面积约 12%，阈值不触发——而视觉上正好是"半张脸被压住"，
 * 是最刺眼的那种侵入。面积比例对细长侵入天然不敏感，故改成「碰一下就躲」。
 */
export const PET_GAP = 10;

export type MenuPlacement = {
  left: number;
  top: number;
  /** 向左翻转（用于 transform-origin） */
  flippedX: boolean;
  /** 向上翻转（用于 transform-origin） */
  flippedY: boolean;
};

function clamp(value: number, min: number, max: number): number {
  // max 可能小于 min（窗口比卡片还小），此时以 min 优先，避免负区间抖动
  return Math.min(Math.max(value, min), Math.max(min, max));
}

function overlapArea(a: Rect, b: Rect): number {
  const w = Math.min(a.left + a.width, b.left + b.width) - Math.max(a.left, b.left);
  const h = Math.min(a.top + a.height, b.top + b.height) - Math.max(a.top, b.top);
  return w > 0 && h > 0 ? w * h : 0;
}

/**
 * 计算卡片落位。
 *
 * @param anchor  右键落点（CSS px，窗口坐标系）
 * @param size    卡片实测尺寸（getBoundingClientRect）
 * @param viewport 窗口尺寸（innerWidth / innerHeight）
 * @param petRect 角色可见包围盒（CSS px）；传了就做避让
 */
export function placeMenu(
  anchor: Point,
  size: Size,
  viewport: Size,
  petRect?: Rect,
): MenuPlacement {
  const availableW = Math.max(0, viewport.width - MENU_EDGE * 2);
  const availableH = Math.max(0, viewport.height - MENU_EDGE * 2);
  const w = Math.min(size.width, availableW);
  const h = Math.min(size.height, availableH);
  const rightLimit = Math.max(MENU_EDGE, viewport.width - MENU_EDGE - w);
  const bottomLimit = Math.max(MENU_EDGE, viewport.height - MENU_EDGE - h);

  let flippedX = false;
  let left = anchor.x + MENU_OFFSET;
  if (left > rightLimit) {
    flippedX = true;
    left = anchor.x - MENU_OFFSET - w;
  }
  left = clamp(left, MENU_EDGE, rightLimit);

  let flippedY = false;
  let top = anchor.y + MENU_OFFSET;
  if (top > bottomLimit) {
    flippedY = true;
    top = anchor.y - MENU_OFFSET - h;
  }
  top = clamp(top, MENU_EDGE, bottomLimit);

  // 角色避让：只要真正碰上就换边（含 PET_GAP 间距），不再按面积比例判定
  const overlapsPet = petRect
    ? overlapArea(
        { left: left - PET_GAP, top, width: w + PET_GAP * 2, height: h },
        petRect,
      ) > 0
    : false;

  if (petRect && overlapsPet) {
    const rightOfPet = petRect.left + petRect.width + PET_GAP;
    const leftOfPet = petRect.left - PET_GAP - w;
    const fitsRight = rightOfPet <= rightLimit;
    const fitsLeft = leftOfPet >= MENU_EDGE;
    // 两侧都能站时选**更宽**的那侧：卡片贴得越少越不容易压到角色的头发/裙摆边缘
    const roomRight = viewport.width - (petRect.left + petRect.width);
    const roomLeft = petRect.left;
    const next = fitsRight && (!fitsLeft || roomRight >= roomLeft)
      ? rightOfPet
      : fitsLeft
        ? leftOfPet
        : left;
    if (next !== left) {
      left = clamp(next, MENU_EDGE, rightLimit);
      flippedX = left < anchor.x;
    }
  }

  return { left, top, flippedX, flippedY };
}
