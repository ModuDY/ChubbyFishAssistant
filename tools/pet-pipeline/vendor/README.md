# vendor：上游素材处理链（逐字复制）

本目录是 **逐字复制** 自上游仓库的素材处理链 Python 脚本，用于自制形象 / 换装时
产出符合本仓库播放规格的透明 WebM。**不修改**上游脚本内容（含其内置的目录约定与
ffmpeg 路径），以保留可核对性与 MIT 许可的可追溯性。

## 来源与许可

- 仓库：`https://github.com/PC2005-cloud/dsh-pet`（MIT，`Copyright (c) 2026 PC2005-cloud`）
- 路径前缀：`scripts/`
- 许可全文：见 [`LICENSE-pc2005-cloud.txt`](./LICENSE-pc2005-cloud.txt)（原样保留）

| 本目录文件 | 上游路径 |
|---|---|
| `watermark_step01.py` | `scripts/watermark_step01.py` |
| `fill_nn.py` | `scripts/fill_nn.py` |
| `chroma_step02.py` | `scripts/chroma_step02.py` |
| `pr_import_step02.py` | `scripts/pr_import_step02.py` |
| `normalize_step03.py` | `scripts/normalize_step03.py` |
| `encode_thumbs.py` | `scripts/encode_thumbs.py` |

> 规格说明（画布 / 帧率 / 脚底锚点 / 编码）见上游另一个仓库
> `MerZlin/dsh-pet-indesktop` 的 `docs/ASSET-PIPELINE.md`；其 `pet/catalog.py` 的
> 播放画布常量 `FEET_Y=330` / `PAD=30` 见 [`catalog-constants.md`](./catalog-constants.md)。

## 为什么是「逐字复制」而不是改写

上游脚本把工作目录（`video/`、`step01/`…`step04/`）与 ffmpeg 路径都**硬编码**为
`脚本所在目录的上一级`（`ROOT = Path(__file__).resolve().parent.parent`）与
`ROOT/.tools/ffmpeg-9.0.1-essentials_build/bin/ffmpeg.exe`。保留原样后：

- 工作目录落在 `tools/pet-pipeline/`（本封装层），不与产品运行时代码混淆；
- 便携 ffmpeg 固定放到 `tools/pet-pipeline/.tools/ffmpeg-9.0.1-essentials_build/bin/`，
  与脚本预期完全一致，无需改动上游脚本。

编排与校验由封装层 [`../build-asset.mjs`](../build-asset.mjs) 负责（准备输入 → 依序调步 →
调 [`../check-asset.mjs`](../check-asset.mjs) 校验 → 通过后才落盘到 `src/assets/pet/videos/`）。

## 核对方式

若要确认逐字一致性，可直接与上游 raw 文件比对：

```sh
diff tools/pet-pipeline/vendor/chroma_step02.py \
  <(curl -s https://raw.githubusercontent.com/PC2005-cloud/dsh-pet/main/scripts/chroma_step02.py)
```

上游更新脚本时应同步重取本目录，并更新上表与 `THIRD_PARTY_NOTICES.md` 的记录。
