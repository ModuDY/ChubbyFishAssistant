# tools/pet-pipeline —— 桌宠素材处理链（本地工具链）

把「绿幕源视频 / PR 手工抠像素材」加工成**符合本仓库播放规格**的透明 WebM，
供自制形象 / 换装 / 新动作使用。产出直接落 `src/assets/pet/videos/` 并过校验，
**不改任何运行时代码**。

> 这是**开发期工具**，与产品运行时隔离：不参与 `vite build`、不进安装包。
> 产物规格与运行时消费见 [`docs/pending/桌宠结合设计方案.md`](../../docs/pending/桌宠结合设计方案.md) §6.3 / §9 P4-A。

## 目录结构

```
tools/pet-pipeline/
├── README.md                 # 本文
├── build-asset.mjs           # 薄封装：准备输入 → 依序调 4 步 → 校验 → 落盘
├── check-asset.mjs           # 产物校验器（零依赖，纯 JS 解析 WebM/EBML）
└── vendor/                   # 上游脚本逐字复制（MIT，出处见 vendor/README.md）
    ├── watermark_step01.py   # step01 水印填充（源有水印时用）
    ├── fill_nn.py            # step01 依赖：最近邻填充
    ├── chroma_step02.py      # step02 路线A：HSV 色相自动抠像
    ├── pr_import_step02.py   # step02 路线B：导入 PR 手工抠像的透明 mov
    ├── normalize_step03.py   # step03 归一化 2160×1215 站立居中、脚底对齐
    ├── encode_thumbs.py      # step04 转码 640×360 VP9 alpha 播放变体
    ├── catalog-constants.md  # 播放画布常量 FEET_Y/PAD 的出处与换算
    └── LICENSE-pc2005-cloud.txt
```

## 产物规格（校验器门槛，`check-asset.mjs` 必过）

| 项 | 值 |
|---|---|
| 容器 / 编码 | WebM / **VP9 + Alpha**（`yuva420p`） |
| 播放画布 | **640×360**（16:9） |
| 帧率 | **24fps** |
| 脚底锚点 | 播放画布 `y≈330`（母版 2160×1215 上为 `FEET_Y=1115`，距底 100） |
| 时长 | > 0.5s；`loop` 动作首尾帧可衔接 |

## 前置依赖（本机需自备）

1. **Python 3.9+**，且可 `import numpy, scipy`（step01/step02 需要）。
   本机若把它装在 PATH 之外（推荐，避免污染全局环境），用 `--python` 显式指定：
   ```
   D:\work\agent\Environment\Python\python.exe        # 示例（本机当前布局）
   ```
   安装示例：`python-3.x-amd64.exe /quiet InstallAllUsers=0 TargetDir=D:\work\agent\Environment\Python PrependPath=0 Include_pip=1`，
   再 `...\python.exe -m pip install numpy scipy`。
2. **便携 ffmpeg（必须带 `libvpx-vp9`）**：上游脚本把路径**硬编码**为
   `tools/pet-pipeline/.tools/ffmpeg-9.0.1-essentials_build/bin/{ffmpeg.exe,ffprobe.exe}`
   （`ROOT/.tools/...`），**系统 PATH 里的 ffmpeg 不会被使用**。
   推荐把**真实文件放在仓库外**（如 `D:\work\agent\Environment\ffmpeg-9.0.2-essentials_build\`），
   再在仓库内建**目录联接**指向它——避免把两个 ~100MB 的二进制塞进工作区：
   ```powershell
   New-Item -ItemType Junction -Path tools\pet-pipeline\.tools\ffmpeg-9.0.1-essentials_build `
            -Target D:\work\agent\Environment\ffmpeg-9.0.2-essentials_build
   ```
   要点：junction **无需管理员**；`.tools/` 已 gitignore，不会入库；联接名须保持上游要求的
   `ffmpeg-9.0.1-essentials_build`（与实际构建版本 `9.0.2` 不同属正常）。
   验证：`...\ffmpeg.exe -version` 的 configuration 应含 `--enable-libvpx`
   （gyan **essentials** 构建实测含，无需另找 full 构建）。

> 缺任一依赖时 `build-asset.mjs` 会直接报错并给出提示，不会产出半成品。

## 用法

```powershell
# 路线 A（自动抠像）：源为绿幕 mp4，按动作名命名
node tools/pet-pipeline/build-asset.mjs --src "D:\素材\待机呼吸休闲.mp4" --alias idle-breathe --label "待机呼吸休闲"

# 路线 B（PR 手工抠像，推荐用于复杂动作）：源为带 alpha 的 mov
node tools/pet-pipeline/build-asset.mjs --src "D:\素材\吃汤圆.mov" --route pr --alias eat-tangyuan --label "吃汤圆"

# 源有水印（需 video/watermark_mask_v5.mkv 与 step01）
node tools/pet-pipeline/build-asset.mjs --src ... --watermark

# 只做校验（校验现有产物 / 目录；ffmpeg 就位时自动含像素级）
node tools/pet-pipeline/check-asset.mjs
node tools/pet-pipeline/check-asset.mjs src/assets/pet/videos/idle-breathe.webm

# 像素级明细（透明占比 / 脚底 / 居中 / 首尾帧差异），排查「飘了 / 偏心」时先跑这个
node tools/pet-pipeline/check-asset.mjs --report

# 强制像素级校验（找不到 ffmpeg 即失败，CI 用）；或只做容器级（跳过像素级）
node tools/pet-pipeline/check-asset.mjs --pixels
node tools/pet-pipeline/check-asset.mjs --no-pixels
```

`check-asset.mjs` 参数：

| 参数 | 说明 |
|---|---|
| `--json` | 机器可读输出（CI 用） |
| `--report` | 打印像素测量明细（隐含 `--pixels`） |
| `--pixels` | 强制像素级校验；找不到 ffmpeg 即失败 |
| `--no-pixels` | 跳过像素级校验，只做容器级 |
| `--feet-y <n>` | 脚底基线覆盖（缺省取 manifest `canvas.feetY`） |
| `--center-x <n>` | 水平中心基线覆盖（缺省取画布中心） |
| `--feet-tolerance <n>` | 脚底容差覆盖（缺省 20px） |
| `--center-tolerance <n>` | 水平中心容差覆盖（缺省 20px） |

> 像素级校验**必须用 `libvpx-vp9` 解码器**——ffmpeg 原生 vp9 解码器不还原 WebM 的 alpha
> 通道（实测全帧 alpha=255），会把正常素材误判成「alpha 已丢失」。校验器已自动带上该解码器；
> 若所用 ffmpeg 构建缺 libvpx，会退回默认解码器、跳过透明占比判定并给出告警。

参数：

| 参数 | 说明 |
|---|---|
| `--src <path>` | 源视频（mp4 走路线 A；mov 走路线 B）。**必填** |
| `--alias <id>` | 落盘文件名（ASCII，如 `idle-breathe`）。缺省由源文件名转写 |
| `--label <中文名>` | 仅用于打印/提示，便于后续登记 manifest |
| `--route auto\|pr` | 抠像路线；缺省按扩展名推断（`.mov`→pr，其余→auto） |
| `--watermark` | 先跑 step01 去水印（需先备好 mask） |
| `--keep` | 保留中间产物（`video/ pr/ step01..03`），便于排查 |
| `--python <path>` | 指定 python 解释器；缺省探测 PATH，也可用环境变量 `PET_PIPELINE_PYTHON`。**Python 未装进 PATH 时必用**（如 `D:\work\agent\Environment\Python\python.exe`） |

## 编排流程（`build-asset.mjs` 做什么）

1. **前置检查**：python / numpy / scipy / 便携 ffmpeg 是否就位。
2. **备料**：把 `--src` 复制成 `<alias>.<ext>` 到与「路线 / 是否去水印」对应的输入目录
   （上游脚本按文件名派生后续产物名，故统一用 ASCII alias）：
   - 路线 B（PR mov）→ `pr/`（`pr_import_step02.py` 的输入）
   - 路线 A + `--watermark` → `video/`（`watermark_step01.py` 的输入，其产物落 `step01/`）
   - 路线 A 无水印 → **直接放 `step01/`**（跳过 step01）

   > 注意：上游 `chroma_step02.py` 读的是 **`step01/`** 而非 `video/`，所以「不去水印」时必须
   > 把源直接放进 `step01/`——否则会报 `No MP4 files found in .../step01`（踩过一次）。
3. **依序调步**（cwd = `tools/pet-pipeline/`，与脚本的 `ROOT` 约定一致）：
   - 可选 `vendor/watermark_step01.py`
   - 路线 A：`vendor/chroma_step02.py` ／ 路线 B：`vendor/pr_import_step02.py`
   - `vendor/normalize_step03.py`
   - `vendor/encode_thumbs.py`
4. **校验后落盘**：对 `step04/<动作名>.webm` 跑 `check-asset.mjs --pixels`（容器级 + 像素级
   均为**强制项**，preflight 已确保便携 ffmpeg 就位）；**通过**才复制到
   `src/assets/pet/videos/<alias>.webm`（不通过则中止、不落盘）。
5. **提示后续手工步骤**：在 `src/assets/pet/manifest.json` 增一条（`id/label/group/loop/file`），
   并同步致谢登记。

> `normalize_step03.py` 会把测量结果写入 `step03/params.json`（画布 / 脚底锚点 / 每动作缩放比），
> 排查「角色飘了 / 踩空」时先看它。

## 已知边界

- **端到端已实测（2026-10-20）**：以 Python 3.13.15 + gyan `ffmpeg-9.0.2-essentials_build`
  （配置含 `--enable-libvpx`）跑通全链——合成绿幕 mp4 → step02 自动抠像（背景采样 `#00F800`）
  → step03 归一化（`scale=1.875`，即 900/480）→ step04 转码 → `check-asset` **PASS**
  （`V_VP9 640x360 24.00fps alpha=true 9.96s`）→ 落盘成功。
  实测环境：便携 ffmpeg 放在 `tools/pet-pipeline/.tools/ffmpeg-9.0.1-essentials_build/bin/`；
  Python 未进 PATH，用 `--python D:\work\agent\Environment\Python\python.exe` 指定。
  **二次复验（同日，含像素级门槛）**：合成 **1280×720** 绿幕 mp4（红色方块当角色）→ 全链 → 像素级
  **PASS**（`透明 87.2% / 脚底 330~330 / 中心均值 319.5`）→ 落盘成功；同时验证门槛确有拦截力
  （不合格产物被 `pixel decode failed: 所有采样帧都没有 alpha≥128 的可见像素` 拦下，未落盘）。
  > 源视频**须按 1280×720** 准备：`chroma_step02.py` 的背景采样边界常量即基于该尺寸，
  > 用 480×480 之类尺寸会得出近乎全透明的产物。
- **像素级检查已实现（2026-10-20）**：除容器级（分辨率 / 帧率 / 编码 / alpha 标志 / 时长）外，
  `check-asset.mjs` 现以 ffmpeg 解码采样帧做像素级校验，设计文档 §9 P4-A 的产物校验清单**已闭合**：
  - **透明像素占比 > 5%**（§8.3 T1 门槛；内置素材实测 76.0%~86.6%）；
  - **脚底对齐**：取「**有效脚底行**」——某行可见像素 ≥16px（`footRowMinPx`，用于剔除金币/音效等
    细长特效，`balance-*` 类实测包围盒最低点低到 y=357）——与基线 `feetY=330` 比对；
  - **水平居中**：包围盒中心**均值**与画布中心比对（角色左右晃动属正常，用均值而非极值）；
  - 容差均取 **20px**：对内置 31 个素材实测包络（脚底 325~343 / 中心均值 308.0~324.8）留余量，
    可放行正常素材，同时拦住「角色飘到半空 / 沉底 / 整体偏心」这类加工事故。
  - `loop` 动作的「首尾帧可衔接」仍**只告警不硬失败**（有损编码无可靠绝对阈值）。
- **容器级仍可在无 ffmpeg 环境使用**：默认「找得到 ffmpeg 就跑像素级、找不到则告警跳过」；
  CI 需要强制时为 `--pixels`，纯容器级为 `--no-pixels`。
- 上游脚本会**批量处理其目录下的全部文件**（非单文件）；`build-asset.mjs` 通过
  「每次只备一个源 + 清理工作目录」来保证单动作语义。
- step01 依赖特定水印 mask（`video/watermark_mask_v5.mkv`），只适用于**上游同款水印**；
  自制素材通常**不需要** step01（直接走 step02，源放 `step01/`）。

## 许可与致谢

`vendor/` 下 Python 脚本逐字复制自 [`PC2005-cloud/dsh-pet`](https://github.com/PC2005-cloud/dsh-pet)
（MIT，`Copyright (c) 2026 PC2005-cloud`，全文见 `vendor/LICENSE-pc2005-cloud.txt`）。
规格与播放画布常量出处见 `vendor/catalog-constants.md`（参考
[`MerZlin/dsh-pet-indesktop`](https://github.com/MerZlin/dsh-pet-indesktop)，MIT）。
新增/替换素材后须同步维护 `src/assets/pet/THIRD_PARTY_NOTICES.md` 与 `README.md` 致谢。
