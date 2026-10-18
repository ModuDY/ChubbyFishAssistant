/// <reference types="vite/client" />

/**
 * 桌宠验证页需要显式 `?url` 导入 webm 素材。
 * vite/client 默认只声明了部分资源类型，webm 的 `?url` 变体需自行补充，
 * 否则 TS 会报「找不到模块」。
 */
declare module '*.webm?url' {
  const src: string;
  export default src;
}
