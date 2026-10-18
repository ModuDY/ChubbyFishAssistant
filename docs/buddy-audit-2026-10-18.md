# Buddy 模块审查（2026-10-18）

## 结论与范围

用户反馈的积分不准、凭证指向不对、删除不彻底、查询慢及反馈不可信，都有对应的代码依据；不能仅靠调整页面显示解决。

本次按当前工作树审查 WorkBuddy / CodeBuddy 的导入、OAuth、凭证存取、续期、积分、快照、删除和前端刷新链路。记录 18 项问题：8 项 P1、10 项 P2。P1 应优先修复，P2 为正确性、清理能力或性能交互缺陷。不等于这 18 项都已在用户今晚的真实账号上发生。

初次审查未读取真实账号凭据、未请求真实云端接口。用户批准修复后，进一步授权“允许真实账号查询，其余操作只用测试数据”；只读联调结果见下节。未操作真实客户端账号或删除真实本地账号数据；未提交、未升版、未打包发布，仓库原有未提交修改保留。下方 F01–F18 的代码位置与描述是修复前审查快照，不代表修复后的当前行为。

## 联调验收（2026-10-18）

### 真实账号只读查询

- 对按池 ID 排序的前三个账号抽样：只读 SQLite 加载元数据、只读加载既有 Stronghold/DPAPI 密钥；AT 仅在内存用于固定官方域名请求，未读取 RT 用于请求、未刷新凭证、未回写余额或缓存、未调用 AppState 初始化。以匿名序号记录，不输出 token、UID、手机号或原始响应。
- account#1：积分接口鉴权失败，旧缓存为 2200；本次余额为未知，未用其他账号或本机 quota 冒充成功；官方用量亦失败。按授权不续期，需用户后续重新登录或单独授权续期。
- account#2：官方积分解析为 2700，5 个包合计 2700，与原缓存一致；积分三件套 4379ms，官方用量 1006ms（1 条请求）。
- account#3：官方积分解析为 2249.6100002，6 个包合计相同，与原缓存一致；积分三件套 2557ms，官方用量 1866ms（42 条请求）。原始数值保留精度，界面两位小数展示；浮点尾数不代表重复计费。
- 两个精确筛选的只读测试通过，不代表三个账号全部查询成功，也不是全池验收。官方用量没有核对用户官网页面，未做大数据量分页压测或真实切号。

### 隔离界面联调与新增修复

独立无头 Edge 挂载生产账号页/概览组件，使用 Tauri 官方 mockIPC 和合成 A/B 两号；外网请求一律拦截，不连接真实 Tauri IPC。界面与生产 Rust HTTP/解析分别验证，不能声称完成了真实桌面端全链路验收。

- 实际复现并修复：先收到后台完成事件、后收到旧手动查询结果时，列表余额已是 123 而明细仍回退 37；概览也从最新 2723 回退 2637。增加积分事件修订号，与请求代次共同保护响应及二次账号列表结果；加载态仍由对应请求 finally 解除。
- 实际复现并修复：所有账号查询失败时概览显示 0.00，失败账号的旧正余额仍能进入 Top 榜。现在无任何有效余额显示“—”，有效零余额仍为 0，排行排除失败/非有限值。
- 8 项界面场景：余额/未知字段、手动刷新防连点、部分失败反馈、超过 16 秒后台完成事件（无额外轮询）、全失败未知合计与排行、账号页晚到响应、概览晚到响应、合成账号删除及清理选项参数。
- 删除测试仅验证前端确认、默认不勾选、勾选透传与列表更新；真实目录/凭证清理由隔离 Rust 回归覆盖，不删除真实账号验证。

### 复跑方式

```powershell
# Playwright 须已可用；不修改项目依赖，不自动安装
$env:BUDDY_PLAYWRIGHT_MODULE = '<现有 playwright 包目录>'
node scripts/buddy-ui-smoke.mjs

# 真实账号需用户授权；不要运行全部 ignored 测试
$env:AIWORK_BUDDY_LIVE_READONLY = '1'
$env:AIWORK_BUDDY_LIVE_DATA_DIR = Join-Path $env:APPDATA 'AIWorkAssistant'
try {
  cargo test --manifest-path src-tauri/Cargo.toml --locked buddy_live_readonly_ -- --ignored --nocapture --test-threads=1
} finally {
  Remove-Item Env:AIWORK_BUDDY_LIVE_READONLY
  Remove-Item Env:AIWORK_BUDDY_LIVE_DATA_DIR
}
```

界面报告写入系统临时目录 `aiwork-buddy-ui-smoke/results.json`，仅失败时生成合成页面截图。原打印真实原始响应、硬编码真实账号的 summary 探针已替换为显式授权、匿名只读测试。

本次前端 10 文件/75 项测试、类型检查、生产构建通过；Rust 全工作区 1086 通过、26 忽略，0 失败。构建仍有原有 chunk 大小及 Rust 编译/链接警告。

## 已实施的修复

### 积分为什么不对

1. **同一笔积分被加了两次**：旧解析器遍历整棵响应树，把 `CreditGroups` 汇总和 `Packages` 明细都相加，合成用例的 2600 被算成 5200。现在官方 `TotalCredits` 优先，缺失时按互斥子树解析，容量对象不再继续下钻重复累计。
2. **识别资源包但漏读剩余字段**：`CycleCapacityRemain` 未在余额候选中，`100 总量 / 37 剩余` 会变成 0。现在补齐该字段及 `CycleUsedCapacity`；无法解析的 remaining/total/used 保留 null，页面显示“未知”，不拿总容量或 0 冒充余额。
3. **余额可能属于另一个账号**：云端失败后没有身份校验的本机 quota 兜底，可能把当前客户端 B 的余额写给 A。已移除这一兜底；auth 文件只有 UID 正匹配才能参与凭证新鲜度比较，显式查询指定账号不再改查其他账号。
4. **旧值和合计污染**：旧缓存不核对账号集合、单号查询会写全池每日快照、企业不限额的负值哨兵会拉低合计。现在缓存 schema=2 并校验 `(id, uid)` 集合；只有完整、非缓存、全部成功且余额有限非负的全池结果进入每日快照，合计排除失败/负哨兵。
5. **失败看起来像成功**：部分失败或刷新失败回退历史值也提示“积分已更新”。现在分别提示成功/失败数量、缓存、后台更新及失败回退，并保留页面警告，不把旧值包装成新余额。

以上是代码及合成回归确认的错误路径，不等同于已逐条确认用户真实账号今晚的服务端响应。

### 审查项落实情况

| 审查项 | 当前实现 |
|---|---|
| F01–F04 | 正匹配 UID；存储 UID 缺失从池补齐、冲突拒绝；OAuth 保存身份、企业标识和域；精确区分 AT/RT 并清 camelCase 别名；取消无身份 quota 余额兜底。网关、CLI 和官方用量也校验工具凭证身份。 |
| F05–F08 | 手动/任务/网关刷新及删除共享账号锁和跨进程锁；锁表不移除仍有持有者或等待者的锁；落库检查池成员；刷新持久化失败向上传播。CodeBuddy 快照独立取自身 storage UID，恢复校验 meta、storage、vscdb，不再复制 WorkBuddy auth。 |
| F09 | 删除弹窗新增“同时清理助手托管快照与聊天备份”选项；覆盖双端主槽、`.bak`、`.bak2`、聊天备份及匹配标记，清凭证/vault、CLI 匹配 token、标签、白名单和当前用量/积分/活动缓存。路径检查限制在托管目录，清理错误汇总返回；后端幂等、可重复调用重试。 |
| F10–F12 | 缓存校验 schema 和成员，删号清缓存，在途结果回写时校验成员；CLI 按凭证反查稳定池 ID，不再只靠新 token 哈希；指定账号无匹配凭证明确报错，不改查其他账号。 |
| F13–F17 | 请求代次防旧响应覆盖、积分查询完成前不提前解除加载态；后台完成事件 `wb-credits-updated` 替代四次短轮询；手动/冷启动/调度/SWR 和单账号查询共享取数去重；安全凭证单行读取；套餐补查后台四路并发并节流，不阻塞余额响应。 |
| F18 | 剩余额度字段和 null 语义补齐，增加容量 100/剩余 37、未知/非法数值及双计回归。 |

### 初次修复验证与剩余边界

- 使用合成凭证、独立临时 SQLite/vault 和模拟文件验证；没有运行读取真实凭证的 ignored 探针。
- 前端 `npm test`：10 个文件、73 项通过；`npm run typecheck` 和 `npm run build` 通过。构建仍有既存的大体积 chunk 提示。
- Rust `cargo test --manifest-path src-tauri/Cargo.toml --locked --workspace`：1086 通过、0 失败、25 忽略；新增覆盖 UID 隔离、AT/RT 加密往返、共享刷新锁、删号拒绝晚到写入、缓存成员/schema、积分字段、并发取数去重、全池每日快照守卫、托管目录清理和双端独立快照。保留既有编译警告，未执行 ignored 探针。
- **未做真实桌面多账号切换/删除端到端验收和网络延迟压测**；新增仅限前三号只读查询与隔离界面联调，不会据此承诺所有真实客户端版本/企业响应均已覆盖。
- 已经丢失的 AT 无法凭空恢复；旧 CodeBuddy 快照若缺少自身身份或 meta/storage 冲突会拒绝恢复，需要从正确登录态重新保存；凭证缺失时需重新导入或登录。
- “清理托管数据”不删除官方客户端登录态/聊天原件、历史签到/审计账本、独立导出文件或邮件备份。取消勾选则保留托管快照和聊天备份。
- 后端支持重复删除重试，但尚无独立的逐项清理结果页；部分清理失败会返回合并错误，账号出池后仍需按原 ID 重试或排查残留。未实现账号级查询进度/取消、逐失败账号的一键批量重试、完整凭证代次系统或自动修复存量凭据。
- 套餐补查完成后暂不独立推送专用事件，下一次列表加载显示；官方用量全池仍串行，积分 paid/free 资源包分页及跨端精确去重仍需更多真实响应样例验证。
- 本轮不拆分网关和全局状态，先固定身份、缓存及删除契约，避免扩大风险。

## P1：身份、凭证和生命周期

### F01：缺 UID 时，凭证选择会放过其他账号或身份不明的本机凭证

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:302`。
- `effective_creds` 仅当目标 UID、auth UID 都非空且不相等时，才排除 auth 文件。目标 UID 缺失，或 auth 文件有 token 但 UID 解析为空，都会绕过身份校验；随后只比较到期时间。
- 触发：池中 A 缺 UID、客户端登录 B，且 A 存储中无到期时间或 B 的到期更晚。查询 A 实际可选中 B 的 token，并把结果标到 A 的池 ID。签到和续期也复用此入口。
- 验证：抽取原函数，内存替代存储和 auth 文件，分别复现“缺目标 UID 选中 B”“缺 auth UID 接受未知身份 token”。
- 建议：身份匹配必须先于新鲜度比较；身份无法确认时失败关闭，不以其他账号补位。明确校验存储记录 UID 与池 UID，不只是文件 UID。

### F02：OAuth 入池有 UID，但写入凭证副本时把 UID 丢了

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\oauth.rs:320`。
- OAuth 获取并保存了池账号 UID，但凭证记录只写 access_token、refresh_token、expires_at_ms。新账号凭证记录没有 UID；`effective_creds` 又不会从调用参数的池 UID 补齐返回的 `Creds.uid`。
- 影响：客户端当前账号不是该 OAuth 账号时，任务侧可能发 `X-No-User-Id`，活跃上报/任务判据缺身份；企业身份字段也没有沿这条写入链路保留。之前给 auth 导入补 UID 的修改没有覆盖这条路径。
- 验证：用 OAuth 当前写入形状构造合成记录，已确认池 UID 为 A 时返回凭证 UID 仍为空。
- 建议：统一导入/OAuth 的凭证规范化模型，原子保存账号身份和凭据；补修“池有 UID、凭证无 UID”的存量记录，而不只补池内空 UID。

### F03：camelCase 的 accessToken 被误分类成 refresh token

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:199`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:246`。
- 两处通过 `ends_with("ccess_token")` 区分 token 类型，`accessToken` 不满足该条件。写入时可能丢掉 access token，读取时把 refresh token 填到 accessToken；`creds_of` 又优先读取 camelCase 键。
- 影响：兼容旧记录、迁移或导入 camelCase 凭证后，请求可能直接使用 refresh token 作为 Bearer。该路径确实被启动凭证迁移使用，不是不可达代码。
- 验证：原安全存取函数 + 内存 DB/vault 往返，合成 AT/RT 分离输入后，返回 access_token 实际等于合成 RT。
- 建议：精确枚举字段，入口立即规范化为 snake_case；对存量混合键做迁移与冲突检测。已经丢失的 AT 不能凭空恢复，应明确要求重新获取凭证。

### F04：云端积分失败后，把本机当前客户端的余额归给任意被查询账号

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_credits.rs:515`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:801`。
- `local_quota_balance` 不接收账号 ID/UID，只取本机发现的首个可用 quota 服务余额；调用方直接把它作为本轮目标账号的成功结果。
- 触发：多个账号的云端查询失败，而本机某个客户端 quota 仍能返回。多个账号可能都被写成同一余额，池合计也会被放大。
- 建议：本机服务必须能证明目标身份一致才能参与该账号余额；否则返回“未知/失败”，不能伪装成成功。应区分客户端、账号、计费区域和余额来源。

### F05：手动续期与后台/网关续期没有共用完整刷新锁

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\accounts.rs:624`；对照 `D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:897`。
- 手动续期仅获取自己的 `wb_renew_locks`，没有获取任务/网关使用的 `refresh_lock_for` 和跨进程刷新锁。token 表锁只串行化落库，不能保护之前已经发出的刷新请求。
- 影响：手动续期与签到、积分 401 自愈、网关刷新可同时拿旧 RT 请求；服务端轮换 RT 时，可能互相失效或后写覆盖，最终需要重登。
- 建议：所有续期入口统一走“账号锁 + 跨进程锁 + 重读 + 刷新 + 持久化”的同一服务。

### F06：删除与在途续期竞态，会让凭据删除后重新出现

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\accounts.rs:261`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\accounts.rs:746`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:915`。
- 删除不等待在途刷新，反而先移除锁表项；续期网络返回后，无条件 upsert token/vault。后面的池写回虽检查账号仍在不在，却已经写回了凭据。
- 可成立的时序：续期读 A → 删除 A 并清凭据 → 续期返回 → A 的凭据重新落库。列表看不见 A，不代表其凭据不存在。再次导入还能产生两把不同的账号锁。
- 建议：账号删除与刷新共享生命周期协调，使用删除标记/代次校验，落库前拒绝过期任务结果。仅在网络返回后查一次池是否存在仍不足以消除检查与写入之间的竞态。

### F07：CodeBuddy 快照混用了 WorkBuddy 的身份和 CodeBuddy 的登录库

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\switcher\authfile.rs:127`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\switcher\authfile.rs:140`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\switcher\authfile.rs:199`。
- CodeBuddy 备份也先复制共享 WorkBuddy auth 文件，并从它取 UID 写入 meta，然后复制 CodeBuddy 自己的 vscdb/storage.json。恢复侧却明确知道两端登录真源不同，CodeBuddy 不恢复共享 auth。
- 触发：WorkBuddy 登录 A、CodeBuddy 登录 B，保存 B 的 CodeBuddy 快照。快照 meta/auth 可能是 A，vscdb 是 B；后续确认却按 meta 的 A 判断，出现错号提示、误判回退，缺 UID 回填也可能被错误 meta 带偏。
- 额外问题：共享 auth 文件不存在时，备份提前返回成功，即使 CodeBuddy 有有效 vscdb 也不备份。
- 建议：CodeBuddy 身份/meta 必须来自自身登录真源；双端采用独立的快照验证与必选文件条件，不让共享 auth 决定 CodeBuddy 是否可备份。

### F08：刷新成功后忽略持久化失败，可能丢掉唯一的新 RT

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:880`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:915`。
- 两条任务刷新路径使用 `let _ = save_token_store(...)`，忽略失败后继续报告刷新成功/返回新凭证。
- 触发：上游已轮换 RT，但保险库或数据库落盘失败。当前内存请求可能暂时成功，重启后只剩失效旧 RT。
- 建议：持久化错误必须作为刷新结果的一部分显式传播；不能在新凭据没有可靠保存时提示续期已完成。设计恢复状态，但不得退回明文落盘。

## P2：清理、展示、查询及性能

### F09：没有完整清理入口，即使传 delete_snapshot=true 也漏删

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src\pages\buddy\BuddyAccounts.tsx:644`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\accounts.rs:282`。
- UI 固定传 false，并明确只移出账号池；这是当前保留快照的产品行为，不应误说成“参数失效”。但用户没有可选的完整清理流程。
- 即使后端传 true，也只删两端主槽：遗漏 `.bak`、`.bak2`、两端会话备份、匹配的 current_account 标记及 CLI settings 中仍生效的 token。历史数据是否保留也没有单独策略。主槽删除错误被忽略，不能报告逐项失败。
- vault 删除也是无返回值、失败仅日志；token 行成功删除后可能仍残留 vault 项，之后再调用账号删除还可能因池/行都不存在而无法重试。
- 建议：分开提供“移出账号池”和“清理助手托管数据”，列清理预览、保留项、逐项结果、失败重试。客户端当前会话/聊天原件和历史审计记录必须明确选择，不能顺手无差别删除。

### F10：积分缓存不校验账号集合，增删账号后仍返回旧集合

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_credits.rs:678`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_credits.rs:723`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\credits.rs:1000`。
- 积分缓存命中在读取当前池之前，不带账号集合指纹，删除也不使缓存失效。全账号用量聚合缓存同样只看时间。
- 影响：删除后看板仍计入旧账号，新导入账号十分钟内可能没有积分数据。全失败的 stale 回退可继续返回旧集合；在途取数也可能把已删账号重新写入缓存。
- 建议：缓存按账号集合/生命周期代次验证，增删导入立即失效或按账号合并；返回和落库前都过滤仍有效的成员。不要把“历史账本保留”与“当前余额汇总”混在一起。

### F11：CLI 当前号用 token 哈希反推，但池 ID 已经改为稳定保留

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\cli.rs:50`；对照 `D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\accounts.rs:441`。
- 同 UID 换 token 重导入时，池保留原 ID；CLI 当前号却根据 settings 中最新 token 重新计算 ID。新 token 哈希不一定等于原池 ID。
- 影响：即使桥接正确账号，返回的 active_account_id 也可能在池中找不到，昵称/当前号徽标缺失，轮换决策把当前号当成未知。
- 验证：使用两枚合成 token 验证新旧哈希确实不同；与当前保留 ID 的合并语义构成确定冲突。
- 建议：凭证与稳定账号 ID 显式关联，CLI 当前身份按规范化凭证记录匹配或可靠身份验证，不靠可轮换 token 生成持久 ID。

### F12：显式查询 A 无凭证时，官方用量接口悄悄换成 B

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\credits.rs:713`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\credits.rs:1377`。
- 传了 user_id 后，如果目标 `pick` 失败，仍沿当前账号、首个池账号、孤儿 token 的降级链选其他账号。
- 影响：指定 A 的用量/活动查询可能成功返回 B。响应虽有实际 account_id，但命令并未保证调用方要求的目标。
- 建议：仅未指定目标时允许默认选号；显式目标不存在、无凭证或身份不明时直接返回明确错误。

### F13：积分刷新失败或回退旧值，账号页仍提示“积分已更新”

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src\pages\buddy\BuddyAccounts.tsx:283`、`D:\work\agent\AiWork\TraeWorkAssistant\src\pages\buddy\BuddyAccounts.tsx:295`。
- 页面只处理 Promise 是否 reject，不检查 `stale`、各账号 `ok` 或实际刷新成功数。后端全失败回退缓存时返回正常结果，部分失败也正常返回，页面最终仍显示成功 toast。
- 建议：按成功/失败/沿用缓存分别计数；展示实际取数时间、失败原因和可重试项。没有更新任何余额时，不应说积分已更新。

### F14：列表加载态提前结束，重复请求没有代次或共享去重

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src\pages\buddy\BuddyAccounts.tsx:259`、`D:\work\agent\AiWork\TraeWorkAssistant\src\pages\buddy\BuddyAccounts.tsx:331`；后台去重位置 `D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\credits.rs:74`。
- 积分请求被启动后没有 await，finally 很快清掉 loading。手动按钮重新可点，用户无法区分列表已加载和积分仍在查询；事件刷新也可叠加。
- 请求没有前端序号保护，旧请求可在新请求后覆盖页面状态。后端的 IN_FLIGHT 只覆盖 SWR 后台分支，不覆盖手动刷新、冷启动无缓存或调度任务。
- 建议：列表与积分采用独立状态；共享同一取数任务，支持进度/取消等待；前端只接受最新请求结果，后端按账号或范围 singleflight。

### F15：SWR 后台刷完后，页面不一定更新

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src\pages\buddy\BuddyAccounts.tsx:283`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\credits.rs:19`、`D:\work\agent\AiWork\TraeWorkAssistant\src\pages\buddy\BuddyOverview.tsx:129`。
- 账号页不消费 refreshing，也不监听刷新完成事件。后台更新不会主动推动页面重读；套餐已齐全时，积分返回后连列表也不会重取。
- 概览虽轮询，但仅四次，总等待约十五秒；单请求超时三十秒，刷新超过该窗口后仍可能停在旧值。概览“刷新”按钮本身也没有传 fresh=true，十分钟 TTL 内通常只读缓存。
- 建议：统一刷新完成/失败事件或任务状态接口；刷新中显示明确状态，结束后更新余额与积分包。手动刷新与默认读缓存要有一致且可解释的语义。

### F16：N 个账号取数，会重复读取并解密整池凭据，形成近似平方级工作量

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:186`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_common.rs:303`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\cli.rs:70`。
- 每个 `effective_creds` 都加载全部 token 行，并对每个账号调用 vault；积分对 N 个账号分别调用，CLI 候选扫描也逐账号整池加载。vault 还有共享互斥，增加网络工作线程不能解决这部分争用。
- 这是从调用链得到的复杂度判断，不是用户机器上的实测耗时。
- 建议：新增单账号安全读取，或每轮批量读取一次后只在需要刷新时重新获取；列表只读非敏感元信息，不为显示徽标解密所有 token。

### F17：即使积分命中缓存，仍可能被套餐补查串行网络拖住

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\credits.rs:43`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\commands\workbuddy\credits.rs:224`。
- `credits_refresh_now` 无论是否命中积分缓存都执行套餐回填；缺套餐的账号逐个调用网络接口，每个十秒超时，失败/unknown 不设置负缓存，下次继续查。
- 影响：N 个缺套餐账号可额外消耗约 N×10 秒超时预算，积分其实已取到却迟迟不返回。云端失败后的本地 quota 端口扫描也逐账号重复，进一步加重慢路径。
- 建议：套餐回填独立后台任务，采用有限并发和失败退避；余额先返回，积分包/套餐随后补齐。本地服务发现应每轮共享并缓存，但身份校验不能省略。

### F18：识别了 CycleCapacityRemain 资源包，却没有读取它的剩余额度

- 位置：`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_credits.rs:146`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_credits.rs:166`、`D:\work\agent\AiWork\TraeWorkAssistant\src-tauri\src\tasks\wb_credits.rs:258`。
- 容量扫描支持 CycleCapacityRemain，但 `remaining_of` 候选缺少它；没有其他剩余字段/已用字段时返回 None，包列表随后把它转为 0。
- 影响：使用这一兼容字段的资源包显示零余额、漏掉到期提醒，依赖包剩余的 CLI 候选也可能被错判不可用。官方总余额存在时，可能表现为“总余额正确、包明细和轮换判断错误”。
- 验证：原 `remaining_of` + 合成包 `{CycleCapacitySize:100, CycleCapacityRemain:37}`，返回 None，确认后续会归零。
- 建议：统一余额字段适配表；无法判定时保留 unknown，不把解析失败当作零。

## 修复前已有修改与验证结果（审查基线）

- 当前工作树已经加入：优先读取 TotalCredits、避免 CreditGroups 与 Packages 双计、列表透出积分到期字段、部分删除凭证/白名单/网关池联动。这些属于已有修改，本次不重复算作“尚未修复”的发现。
- 余额双计的回归测试目前通过；但它不覆盖账号身份、local quota 误归属、缓存集合、并发删除和 UI 刷新结果，因此不能据此认定积分链路整体已可靠。
- 已运行当前 Rust 积分解析/缓存/并发测试：11 通过、1 忽略。忽略项会读取真实凭证并请求云端，本次没有运行。
- 已运行当前 wb_common 企业识别/指纹/请求头测试：5 通过；命令层积分辅助测试：10 通过。合计现有相关测试 26 通过、1 忽略；编译存在原有警告。
- 独立内存 harness 从当前源文件抽取原函数，使用模拟 DB/vault/auth，不接触真实数据：5 个缺陷复现检查成立；另用合成 token 验证 CLI 哈希与稳定池 ID 冲突。复现检查断言当前错误行为，检查通过不代表缺陷已修复，也不替代集成测试。
- 未做真实多账号端到端验证、网络延迟压测或 UI 人工验收；没有声称复现用户今晚的全部实际操作。

## 原建议修复顺序与验收清单

1. **先保证是谁**：统一 BuddyIdentity / CredentialResolver，修 F01–F04、F07、F11、F12；拒绝跨账号兜底，修复存量别名和缺 UID 数据。
2. **再保证凭证与删除一致**：统一刷新/导入/删除的生命周期协调，处理 F05、F06、F08、F09。删除具备预览、逐项结果与幂等重试，不自动清用户客户端原件。
3. **统一积分真源与缓存**：修 F10、F18；每账号携带来源、获取时间、状态、错误和账号代次；失败不写成零，不把过期数据伪装新数据。
4. **性能和交互一起修**：修 F13–F17；批量安全读取、singleflight、后台套餐补查、账号级进度、局部重试、完成事件，避免只能盯着整页 spinner。
5. **补集成回归再发布**：至少覆盖 A/B 两账号同时登录、OAuth 新账号、UID 缺失、camel/snake 混合键、云端失败但本地有余额、手动与后台同时续期、续期中删除、删除后重启、增删缓存失效、部分查询失败、刷新超过十五秒及重复点击。完整清理要检查两端主槽/两代回滚槽/会话备份/token 行/vault/CLI 标记，并验证其他账号完全不受影响。

原建议的完整产品能力仍按上方“验证与边界”区分；自动回归不能代替真实客户端多账号验收。
