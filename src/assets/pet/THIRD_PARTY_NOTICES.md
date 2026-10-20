# 第三方素材声明（桌宠）

本目录下的桌宠动作素材与相关参考实现来自以下开源仓库，均为 MIT 许可。

## 1. 素材来源

- **PC2005-cloud/dsh-pet** — `https://github.com/PC2005-cloud/dsh-pet`（MIT，`Copyright (c) 2026 PC2005-cloud`）
  - 本目录 `videos/*.webm`（大肥鱼桌宠动作）来自该仓库 `dsh-pet/assets/webm`，**上游 106 个动作已全量收录**（2026-10-20）。
  - 收录范围说明：31 个业务相关动作由状态机驱动；其余 75 个彩蛋按三类时机播放——节日类（`festival-*`）依日期触发、饮食起居类（`life-*`）依早/午/晚时段触发、其余生活/才艺/特效进待机随机池分桶轮转；全部动作均可在桌宠右键菜单手动点播。
  - 落盘文件名为 ASCII 别名（`<group>-<slug>.webm`），上游中文原名保留在 `manifest.json` 的 `label` 字段；中文名 → 别名的完整映射见 `tools/pet-pipeline/fetch-webm.mjs` 的 `CATALOG`。
  - MIT 许可全文见同目录 `LICENSE-dsh-pet.txt`（随副本保留，履行 MIT 署名义务）。

- **MerZlin/dsh-pet-indesktop** — `https://github.com/MerZlin/dsh-pet-indesktop`（MIT）
  - 本项目的桌宠**工程实现**（状态优先级聚合、隐藏即停、点击穿透、点击热区/气泡锚点几何等）参考了该仓库的 Qt 实现与踩坑结论；**未直接复制其代码**，外壳由本项目以 Tauri 2 重做。

## 2. 未采用的第三方资源（授权未核实，故未引入）

- **字体**：上游气泡字体（如「上首软糖体」）**不在仓库 MIT 覆盖范围内**，商用需单独授权。本项目 P0 统一使用系统字体，**未引入**。
- **音效**：上游 `assets/sounds/` 音效素材未逐项核实授权，**未引入**。

## 3. 形象与商标提示

MIT 许可覆盖的是**仓库的代码与素材文件**；桌宠**形象本身**为社区二创、拟人化地指向 DeepSeek 品牌，DeepSeek 官方从未发布过该吉祥物。此属**商标 / 形象风险，与代码许可无关**。自用 / 内部使用风险低；若对外分发或商用，建议替换为自制或已授权形象（架构上规划为可替换素材包）。

## 4. 新增 / 替换素材时

新增或替换 `videos/*.webm` 后，须同步更新 `manifest.json` 与本声明（保留来源与许可信息）。二次加工 / 换装统一走上游 4 步转换管线（水印 → 抠像 → 归一化 → 编码），规格须与 `manifest.json` 的 `canvas` 一致：`640×360` / `24fps` / 脚底对齐 `y=330` / 留白 `PAD=30`。

批量补充上游素材可直接用 `node tools/pet-pipeline/fetch-webm.mjs`（幂等，已存在且大小相符则跳过；未登记进 `CATALOG` 的上游文件会报错退出，避免漏挂），随后 `node tools/pet-pipeline/check-asset.mjs --pixels` 校验。
