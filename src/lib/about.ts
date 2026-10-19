// 软件宣传图 base64 内嵌（由 scripts/gen_asset_base64.mjs 从 promo_banner.jpg 生成）：
// dev server 关闭后弹窗图片仍可显示，不依赖运行中的静态服务器
import { promo_banner_base64 as promoBanner } from '../assets/promo-banner.base64';
// 赞赏码 base64 内嵌（同款原因：关于页在静态服务器不可用时也要能看到二维码）
import { donate_qr_base64 as donateQr } from '../assets/donate-qr.base64';

/** 应用品牌与关于信息（集中管理，改名只改这里；版本号不再硬编码——运行时经 getVersion() 读 Cargo.toml 单一来源） */
export const APP_NAME = '大肥鱼助手';
/**
 * 英文名：用于关于页署名、仓库与发布说明。
 * **不是** exe 名 / identifier / 数据目录——那些是持久化契约（见 AGENT.md 品牌命名约定）。
 */
export const APP_NAME_EN = 'ChubbyFishAssistant';
export const APP_TAGLINE = '多账号签到与管理 · 一站式工作台';
export const APP_OVERVIEW =
  'Windows 桌面端多账号签到与管理一站式工作台（Tauri 2 + React 18 + Rust）。' +
  '深度支持 Trae 客户端（Trae CN）：多账号签到、登录态独立切换、设备隔离、积分看板与 OpenAI / Anthropic 兼容 API 网关，数据全部本地存储。';
/** 当前作者 / 二次开发维护者 */
export const APP_AUTHOR = '刘冬阳';
/**
 * 原作者（上游）：本项目在其 MIT 开源版本基础上二次开发。
 * MIT 要求保留原版权署名，故单列，不能因为改名而抹掉。
 */
export const UPSTREAM_AUTHOR = '朱天伟';
/** 二次开发说明（关于页展示，紧跟作者一行） */
export const APP_BASED_ON = `API网关模块基于 ${UPSTREAM_AUTHOR} 的开源版本二次开发，原版权归其所有`;
export const APP_COPYRIGHT = `Copyright © 2026 ${APP_AUTHOR} · MIT License`;
export const APP_DISCLAIMER =
  '本工具与 Trae / WorkBuddy / 豆包等官方均无关联，仅供学习研究，请仅管理本人合法持有的账号，风险自担。';
/**
 * 桌宠素材致谢（合规义务，两个上游仓库都要署名）。
 * 素材与实现均为 MIT；形象为社区二创，与 DeepSeek 官方无关。详见 src/assets/pet/THIRD_PARTY_NOTICES.md。
 */
export const PET_ATTRIBUTION =
  '桌宠素材来自 PC2005-cloud/dsh-pet (MIT) · 工程实现参考 MerZlin/dsh-pet-indesktop (MIT)';

export const LINK_GITHUB = 'https://github.com/ModuDY';
export const LINK_REPO = 'https://github.com/ModuDY/ChubbyFishAssistant';

export { promoBanner, donateQr };
