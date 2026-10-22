/** 工作台「待办 / 提醒」规则引擎（纯函数，无副作用、无 IO）。
 *
 * 设计红线（方案 §4.2 / §10）：
 * - **只列"今天确实要动手"的事**，不列指标、不列统计、不列"一切正常"；
 * - 没有可做的事就返回空数组，工作台**整块隐藏**（不显示空卡片）；
 * - 每条必须能一键执行或明确指向某个页面，否则不该出现。
 *
 * 所有输入都来自既有 store（accounts / apiStatus / certInstalled / env），
 * 因此新增一条规则 = 改这个文件，不必动 Rust，也不必加 IPC。
 */
import type { AccountView, ApiServiceStatus, CaptureIntent, EnvStatus, ViewKey } from '../types';

/** 规则产出的待办项（与 WorkbenchTodo 同构，单独导出便于测试） */
export interface TodoItem {
  id: string;
  title: string;
  detail?: string;
  tone: 'danger' | 'warn' | 'info';
  actionLabel: string | null;
  action:
    | { kind: 'nav'; view: ViewKey }
    | { kind: 'renew_jwt' }
    | { kind: 'checkin'; platform: 'trae' | 'buddy' | 'qoder' }
    | { kind: 'start_api' }
    | { kind: 'install_cert' }
    | null;
}

export interface TodoInput {
  accounts: AccountView[];
  apiStatus: ApiServiceStatus | null;
  certInstalled: boolean;
  env: EnvStatus | null;
  /** 各平台账号数（用于判断"该平台是否需要提醒签到"，无账号则完全不提示） */
  platformCounts?: { trae: number; buddy: number; qoder: number };
  /** 当前 Unix 秒（注入以便测试，默认取 now） */
  nowSec?: number;
}

/** 严重度排序权重：danger 必须排在 warn/info 前 */
const TONE_WEIGHT: Record<TodoItem['tone'], number> = {
  danger: 0,
  warn: 1,
  info: 2,
};

/**
 * 从既有数据推导今日待办。
 *
 * 规则清单（按优先级）：
 * 1. Trae 未安装 → 引导安装（info，但有阻塞性，仅在完全未配置时出现）
 * 2. CA 证书未装 → 无法抓包（danger，阻断签到链路）
 * 3. JWT 24h 内过期 → 一键续期（danger）
 * 4. 今日未签到（按平台分组，仅对有账号的平台提示）
 * 5. 积分 7 日内过期 → 提示尽快使用（warn）
 * 6. 网关未运行但曾配置过自动启动（info）
 */
export function buildTodos(input: TodoInput): TodoItem[] {
  const {
    accounts,
    apiStatus,
    certInstalled,
    env,
    platformCounts,
    nowSec = Math.floor(Date.now() / 1000),
  } = input;

  const todos: TodoItem[] = [];
  const counts = platformCounts ?? { trae: accounts.length, buddy: 0, qoder: 0 };

  // ── 规则 1：Trae 未安装 ──
  // 只在"完全没有任何平台账号"时提示——已有账号说明环境早已跑通，
  // 此时 env 探测失败更可能是路径变更，不该用"未安装"制造焦虑。
  if (!env?.installed && counts.trae === 0 && counts.buddy === 0 && counts.qoder === 0) {
    todos.push({
      id: 'env-missing',
      title: '未检测到 Trae 客户端',
      detail: '代理抓包与账号切换依赖 Trae，安装后即可开始',
      tone: 'info',
      actionLabel: '前往下载',
      action: { kind: 'nav', view: 'settings' },
    });
  }

  // ── 规则 2：CA 证书未安装（阻断链路，优先级最高）──
  if (env?.installed && !certInstalled) {
    todos.push({
      id: 'cert-missing',
      title: 'CA 证书尚未安装',
      detail: '不装则代理无法解密 HTTPS，签到接口抓不到',
      tone: 'danger',
      actionLabel: '一键安装',
      action: { kind: 'install_cert' },
    });
  }

  // ── 规则 3：JWT 24h 内过期（含已过期）──
  // 只统计有 refresh_token 的账号：无 refresh_token 的账号续期必然失败，
  // 提示"续期"是误导，应引导重新登录。
  const jwtExpiring = accounts.filter(
    (a) => a.jwt_exp_hours !== null && a.jwt_exp_hours <= 24,
  );
  const jwtRenewable = jwtExpiring.filter((a) => a.has_refresh_token).length;
  const jwtRelogin = jwtExpiring.length - jwtRenewable;

  if (jwtRenewable > 0) {
    todos.push({
      id: 'jwt-renew',
      title: `${jwtRenewable} 个账号 JWT 24 小时内过期`,
      detail: '过期后签到与积分查询会失败，续期即可延长',
      tone: jwtExpiring.some((a) => (a.jwt_exp_hours ?? 99) <= 6) ? 'danger' : 'warn',
      actionLabel: '一键续期',
      action: { kind: 'renew_jwt' },
    });
  }
  if (jwtRelogin > 0) {
    todos.push({
      id: 'jwt-relogin',
      title: `${jwtRelogin} 个账号凭证即将过期且无法自动续期`,
      detail: '这些账号没有 refresh_token，需要重新登录获取',
      tone: 'warn',
      actionLabel: '去处理',
      action: { kind: 'nav', view: 'accounts' },
    });
  }

  // ── 规则 4：今日未签到（按平台分组）──
  const traeUnchecked = accounts.filter((a) => !a.checked_today).length;
  if (counts.trae > 0 && traeUnchecked > 0) {
    const all = traeUnchecked === counts.trae;
    todos.push({
      id: 'checkin-trae',
      title: all
        ? `Trae ${traeUnchecked} 个账号今日未签到`
        : `Trae 还有 ${traeUnchecked} 个账号今日未签到`,
      detail: all ? '今日签到可拿满额积分' : '部分账号已完成，补齐剩余即可',
      tone: all ? 'warn' : 'info',
      actionLabel: '去签到',
      action: { kind: 'checkin', platform: 'trae' },
    });
  }

  // Buddy / Qoder 的签到状态不在 accounts 里（各自独立命令），
  // 因此只做"该平台有账号就提示去签到页"的弱提醒，不猜具体完成数。
  if (counts.buddy > 0) {
    todos.push({
      id: 'checkin-buddy',
      title: 'Buddy 签到与成长任务',
      detail: `${counts.buddy} 个账号；每日成长含旅行/盲盒/任务三项`,
      tone: 'info',
      actionLabel: '去签到',
      action: { kind: 'checkin', platform: 'buddy' },
    });
  }
  if (counts.qoder > 0) {
    todos.push({
      id: 'checkin-qoder',
      title: 'Qoder 每日签到',
      detail: `${counts.qoder} 个账号；单次覆盖 0 点签到与 10:00 登录奖励`,
      tone: 'info',
      actionLabel: '去签到',
      action: { kind: 'checkin', platform: 'qoder' },
    });
  }

  // ── 规则 5：积分 7 日内过期 ──
  const creditsExpiring = accounts.filter(
    (a) =>
      a.credits_expire_at != null &&
      a.credits_expire_at > nowSec &&
      a.credits_expire_at <= nowSec + 7 * 86400 &&
      (a.remaining_credits ?? 0) > 0,
  ).length;
  if (creditsExpiring > 0) {
    todos.push({
      id: 'credits-expiring',
      title: `${creditsExpiring} 个账号的积分 7 日内过期`,
      detail: '过期即作废，建议尽快用于实际请求',
      tone: 'warn',
      actionLabel: '看详情',
      action: { kind: 'nav', view: 'credits' },
    });
  }

  // ── 规则 6：网关未运行 ──
  // 只在"有账号 但网关关着"时提示——没账号时提示开网关是空转。
  if (accounts.length > 0 && apiStatus && !apiStatus.running) {
    todos.push({
      id: 'api-stopped',
      title: 'API 网关当前未运行',
      detail: 'IDE 与第三方工具将无法调用本地模型池',
      tone: 'info',
      actionLabel: '启动网关',
      action: { kind: 'start_api' },
    });
  }

  // 按严重度稳定排序（同级保持规则定义顺序，避免每次渲染跳动）
  return todos
    .map((t, i) => ({ t, i }))
    .sort((a, b) => TONE_WEIGHT[a.t.tone] - TONE_WEIGHT[b.t.tone] || a.i - b.i)
    .map((x) => x.t);
}

/** 快速捕获输入解析：`?` 问知识库、`/` 走命令、其余存笔记（方案 §4.2 区块⑤） */
export function parseCapture(raw: string): CaptureIntent | null {
  const text = raw.trim();
  if (!text) return null;
  if (text.startsWith('?')) {
    const q = text.slice(1).trim();
    return q ? { kind: 'ask', text: q } : null;
  }
  if (text.startsWith('/')) {
    const cmd = text.slice(1).trim();
    return cmd ? { kind: 'command', text: cmd } : null;
  }
  return { kind: 'note', text };
}
