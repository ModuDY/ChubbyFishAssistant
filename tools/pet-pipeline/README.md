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
2. **便携 ffmpeg（必须带 `libvpx-vp9`）**，固定放在：
   ```
   tools/pet-pipeline/.tools/ffmpeg-9.0.1-essentials_build/bin/{ffmpeg.exe,ffprobe.exe}
   ```
   上游脚本把该路径**硬编码**（`ROOT/.tools/...`），故本目录结构必须与之一致；
   **系统 PATH 里的 ffmpeg 不会被使用**。官方 essentials build 不含 libvpx，
   需选带 libvpx 的构建（或自行编译 `--enable-libvpx`）。

> 缺任一依赖时 `build-asset.mjs` 会直接报错并给出提示，不会产出半成品。

## 用法

```powershell
# 路线 A（自动抠像）：源为绿幕 mp4，按动作名命名
node tools/pet-pipeline/build-asset.mjs --src "D:\素材\待机呼吸休闲.mp4" --alias idle-breathe --label "待机呼吸休闲"

# 路线 B（PR 手工抠像，推荐用于复杂动作）：源为带 alpha 的 mov
node tools/pet-pipeline/build-asset.mjs --src "D:\素材\吃汤圆.mov" --route pr --alias eat-tangyuan --label "吃汤圆"

# 源有水印（需 video/watermark_mask_v5.mkv 与 step01）
node tools/pet-pipeline/build-asset.mjs --src ... --watermark

# 只做校验（校验现有产物 / 目录）
node tools/pet-pipeline/check-asset.mjs
node tools/pet-pipeline/check-asset.mjs src/assets/pet/videos/idle-breathe.webm
```

参数：

| 参数 | 说明 |
|---|---|
| `--src <path>` | 源视频（mp4 走路线 A；mov 走路线 B）。**必填** |
| `--alias <id>` | 落盘文件名（ASCII，如 `idle-breathe`）。缺省由源文件名转写 |
| `--label <中文名>` | 仅用于打印/提示，便于后续登记 manifest |
| `--route auto\|pr` | 抠像路线；缺省按扩展名推断（`.mov`→pr，其余→auto） |
| `--watermark` | 先跑 step01 去水印（需先备好 mask） |
| `--keep` | 保留中间产物（`video/ step01..03`），便于排查 |

## 编排流程（`build-asset.mjs` 做什么）

1. **前置检查**：python / numpy / scipy / 便携 ffmpeg 是否就位。
2. **备料**：把 `--src` 复制进 `tools/pet-pipeline/{video|pr}/`，文件名保持动作名
   （上游脚本按此名派生后续产物名）。
3. **依序调步**（cwd = `tools/pet-pipeline/`，与脚本的 `ROOT` 约定一致）：
   - 可选 `vendor/watermark_step01.py`
   - 路线 A：`vendor/chroma_step02.py` ／ 路线 B：`vendor/pr_import_step02.py`
   - `vendor/normalize_step03.py`
   - `vendor/encode_thumbs.py`
4. **校验后落盘**：对 `step04/<动作名>.webm` 跑 `check-asset.mjs`；**通过**才复制到
   `src/assets/pet/videos/<alias>.webm`（不通过则中止、不落盘）。
5. **提示后续手工步骤**：在 `src/assets/pet/manifest.json` 增一条（`id/label/group/loop/file`），
   并同步致谢登记。

> `normalize_step03.py` 会把测量结果写入 `step03/params.json`（画布 / 脚底锚点 / 每动作缩放比），
> 排查「角色飘了 / 踩空」时先看它。

## 已知边界

- **本机未做端到端实测**：仓库当前环境缺带 `libvpx` 的 ffmpeg，4 步链只能跑到「前置检查失败」。
  待便携 ffmpeg 就位后再跑一次真实素材验证（并回填实测结论）。
- 上游脚本会**批量处理其目录下的全部文件**（非单文件）；`build-asset.mjs` 通过
  「每次只备一个源 + 清理工作目录」来保证单动作语义。
- step01 依赖特定水印 mask（`video/watermark_mask_v5.mkv`），只适用于**上游同款水印**；
  自制素材通常**不需要** step01（直接走 step02）。

## 许可与致谢

`vendor/` 下 Python 脚本逐字复制自 [`PC2005-cloud/dsh-pet`](https://github.com/PC2005-cloud/dsh-pet)
（MIT，`Copyright (c) 2026 PC2005-cloud`，全文见 `vendor/LICENSE-pc2005-cloud.txt`）。
规格与播放画布常量出处见 `vendor/catalog-constants.md`（参考
[`MerZlin/dsh-pet-indesktop`](https://github.com/MerZlin/dsh-pet-indesktop)，MIT）。
新增/替换素材后须同步维护 `src/assets/pet/THIRD_PARTY_NOTICES.md` 与 `README.md` 致谢。
