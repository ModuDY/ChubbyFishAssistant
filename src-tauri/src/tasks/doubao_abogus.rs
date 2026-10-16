//! 豆包 bdms（`a_bogus`）编解码器（网关自行生成签名的地基）。
//!
//! ── 为什么单独成模块 ──────────────────────────────────────
//! `a_bogus` 是豆包 Web/网关请求里的一个「自签名」参数，由前端 JS 生成、
//! 服务端反向校验。这里只做**纯算法**编解码，不碰网络、不碰账号，便于单测对拍；
//! 真正的请求组装留给上层调用方。
//!
//! ── 结构（真机抓包逆向 + 样本验证）────────────────────────
//! ```text
//! a_bogus = s4_encode( prefix[4] ++ rc4_variant( version[8] ++ garble_3to4(payload) ) )
//! ```
//! 解码方向即倒序：s4 解码 → 取前 4 字节 prefix → 余段按单字节 key（`0xD3`）做
//! RC4 变体 → 取前 8 字节 version → 余段按 4 字节整组反 garble 还原 payload，
//! 最后一组之后的残字节存为 `trailer`（真机 2 字节，与 payload 一起保证长度守恒）。
//! 真机样本形态：`a_bogus` 192 字符 → 解出 142 字节（含 `=` 补齐）→
//! prefix 4 + RC4 段 138（version 8 + garble 段 128（对应 payload 96）+ trailer 2）。
//!
//! ── 硬约束 ────────────────────────────────────────────────
//! 本文件**不出现任何真实 a_bogus / msToken / cookie 取值**；常量只有算法表与掩码，
//! 测试数据一律为合成字节。错误约定与仓库一致：`Result<_, String>`。

#![allow(dead_code)] // 本步只落地算法层（网关自签名地基），尚未接线，生产调用方稍后消费

use base64::Engine as _;

/// s4：`a_bogus` 最终输出用的自定义 base64 表（与标准表位序一致：索引 i 表示同一 6 bit 值）。
///
/// 注意它**不是严格的标准字符集置换**：索引 27 上是 `-`（标准表该位是 `b`），
/// 且全表无 `+`。对应地，实现按「标准字符 → 表内字符」逐位置换，字符集不必相同，
/// 只要 64 个字符互异即可保持可逆（见单测 `tables_have_distinct_chars`）。
const S4_TABLE: &[u8; 64] = b"Dkdpgh2ZmsQB80/MfvV36XI1R45-WUAlEixNLwoqYTOPuzKFjJnry79HbGcaStCe";
/// s3：UA 预处理用的自定义 base64 表（本步仅作常量备用，与 s4 同构）
const S3_TABLE: &[u8; 64] = b"ckdp1h4ZKsUB80/Mfvw36XIgR25+WQAlEi7NLboqYTOPuzmFjJnryx9HVGDaStCe";
/// 标准 base64 表（置换的基准：索引 i 在两个表里表示同一个 6 bit 值）
const STD_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// RC4 变体的固定单字节 key（真机逆向值；不是机密材料，只是协议常量）
const RC4_KEY: u8 = 0xD3;

/// prefix 固定 4 字节
pub const PREFIX_LEN: usize = 4;
/// version 固定 8 字节
pub const VERSION_LEN: usize = 8;

/// garble 的六个位掩码（三组互补：`A|B == C|D == E|F == 255`，
/// 且两两相与为 0，正是「反解能拿回原字节」的前提）
const MASK_A: u8 = 145;
const MASK_B: u8 = 110;
const MASK_C: u8 = 66;
const MASK_D: u8 = 189;
const MASK_E: u8 = 44;
const MASK_F: u8 = 211;

/// `a_bogus` 拆出的三段 + 尾随字节。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ABogusParts {
    /// 前 4 字节（真机为随请求变化的头，不参与 payload，原样透传）
    pub prefix: [u8; 4],
    /// 8 字节版本段（bdms 版本标识）
    pub version: [u8; 8],
    /// 还原后的明文 payload（真机为 96 字节 = 3 的倍数）
    pub payload: Vec<u8>,
    /// RC4 段里「最后一个完整 4 字节 garble 组」之后的剩余字节（真机为 2）。
    ///
    /// 不带任何已知语义，但对**长度守恒**是必需的：encode 时原样拼在 garble 段之后，
    /// 才能让 `s4_encode` 的字符数与原文一致（否则 96 字节 payload 只会编出 188 字符，
    /// 而不是真机的 192）。合成数据里长度可为 0~3。
    pub trailer: Vec<u8>,
}

/// s4 编码：字节 → `a_bogus` 字符（标准 base64 后按 s4 表置换，`=` 补齐）
pub fn s4_encode(data: &[u8]) -> String {
    table_encode(S4_TABLE, data)
}

/// s4 解码：`a_bogus` 字符 → 字节；非法字符/长度返回 `None`
pub fn s4_decode(text: &str) -> Option<Vec<u8>> {
    table_decode(S4_TABLE, text)
}

/// s3 编码（UA 预处理用表；通用实现，与 s4 同款）
pub fn s3_encode(data: &[u8]) -> String {
    table_encode(S3_TABLE, data)
}

/// s3 解码
pub fn s3_decode(text: &str) -> Option<Vec<u8>> {
    table_decode(S3_TABLE, text)
}

/// 用给定 64 字符表做「标准 base64 的字符置换」编码。
///
/// 表是标准表的置换、位序一致，所以对标准 base64 结果逐字符查表即可
/// （比手接位运算更不容易写错）；`=` 是补齐符、不在表内，原样保留。
fn table_encode(table: &[u8; 64], data: &[u8]) -> String {
    let standard = base64::engine::general_purpose::STANDARD.encode(data);
    let mut out = String::with_capacity(standard.len());
    for ch in standard.bytes() {
        if ch == b'=' {
            out.push('=');
            continue;
        }
        // 标准 base64 输出必在标准表内，position 不会落空（unwrap_or 只为免 panic）
        let index = STD_TABLE.iter().position(|&c| c == ch).unwrap_or(0);
        out.push(table[index] as char);
    }
    out
}

/// 用给定 64 字符表解码；先置换回标准字符再走标准 base64 解码。
///
/// 先建「自定义字符 → 标准字符」反表 + 合法字符位图，任何不在表内且不是 `=` 的
/// 字符直接返回 `None`（decode 的字符集校验就落在这里）。
fn table_decode(table: &[u8; 64], text: &str) -> Option<Vec<u8>> {
    let mut mapped = [0u8; 256];
    let mut valid = [false; 256];
    for (index, &standard_char) in STD_TABLE.iter().enumerate() {
        let custom = table[index];
        mapped[custom as usize] = standard_char;
        valid[custom as usize] = true;
    }
    mapped[b'=' as usize] = b'=';
    valid[b'=' as usize] = true;

    let mut standard_bytes = Vec::with_capacity(text.len());
    for byte in text.bytes() {
        if !valid[byte as usize] {
            return None;
        }
        standard_bytes.push(mapped[byte as usize]);
    }
    base64::engine::general_purpose::STANDARD.decode(&standard_bytes).ok()
}

/// RC4 变体（对称，加解密同一函数）。
///
/// 与标准 RC4 的两处差异：
/// 1. S-box **反转**初始化（`S[i] = 255 - i`，不是 `0..255`）；
/// 2. KSA 的 j 递推多了一项 `j * S[i]`：`j = (j * S[i] + j + key[i % len]) % 256`。
/// PRGA 与标准一致。
pub fn rc4_variant(data: &[u8], key: &[u8]) -> Vec<u8> {
    // 空 key 会让 `i % 0` panic；这里按恒等处理（协议中 key 恒为单字节，仅为健壮性）
    if key.is_empty() {
        return data.to_vec();
    }
    let mut s = [0u8; 256];
    for (i, slot) in s.iter_mut().enumerate() {
        *slot = 255u8.wrapping_sub(i as u8);
    }
    // KSA：j * S[i] 会溢出不大于 256 次，按 u8 环绕即等价于 % 256
    let mut j: u8 = 0;
    for i in 0..256usize {
        let key_byte = key[i % key.len()];
        j = j.wrapping_mul(s[i]).wrapping_add(j).wrapping_add(key_byte);
        s.swap(i, j as usize);
    }
    // PRGA
    let mut i: u8 = 0;
    let mut j: u8 = 0;
    let mut out = Vec::with_capacity(data.len());
    for &byte in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        let k = s[s[i as usize].wrapping_add(s[j as usize]) as usize];
        out.push(byte ^ k);
    }
    out
}

/// 位掩码 3→4 扩展（正向 garble）：每 3 字节 + 注入 1 随机字节 → 4 字节。
///
/// 注入的随机位只影响 `out0..out2` 中被掩码选中、原数据没有的那些位，
/// 反解时被丢弃；`out3` 是原数据三字节按位打包的冗余副本（用于补齐缺失位）。
/// 输入不是 3 的倍数时右侧补 0 凑整（真机 payload 恒为 96 字节 = 3 的倍数，故此分支
/// 在真机路径上永不触发；仅保健壮性）。**这一补齐是有损的**：补齐的 0 会被编进 garble
/// 结果，反解时无法再区分「原本就是 0」还是「补出来的 0」，拿到的是补齐后的 3 的倍数
/// 长度（真机 96 字节不受影响）。
pub fn garble_3to4(data: &[u8]) -> Vec<u8> {
    let groups = (data.len() + 2) / 3;
    let mut out = Vec::with_capacity(groups * 4);
    for group in 0..groups {
        let d0 = data.get(group * 3).copied().unwrap_or(0);
        let d1 = data.get(group * 3 + 1).copied().unwrap_or(0);
        let d2 = data.get(group * 3 + 2).copied().unwrap_or(0);
        let mut rnd = [0u8; 1];
        random_fill(&mut rnd);
        let r = rnd[0];

        out.push((r & MASK_A) | (d0 & MASK_B));
        out.push((r & MASK_C) | (d1 & MASK_D));
        out.push((r & MASK_E) | (d2 & MASK_F));
        out.push((d0 & MASK_A) | (d1 & MASK_C) | (d2 & MASK_E));
    }
    out
}

/// 位掩码 4→3 反解（反向 garble）：4 字节 → 3 字节，丢弃注入的随机位。
///
/// 输入长度非 4 的倍数时这里只处理整组，末尾残字节由调用方保管
/// （`decode` 会把它存进 [`ABogusParts::trailer`]，真机为 2 字节，
/// 保证重编码时长度守恒）。
pub fn ungarble_3to4(data: &[u8]) -> Vec<u8> {
    let groups = data.len() / 4;
    let mut out = Vec::with_capacity(groups * 3);
    for group in 0..groups {
        let o0 = data[group * 4];
        let o1 = data[group * 4 + 1];
        let o2 = data[group * 4 + 2];
        let o3 = data[group * 4 + 3];
        out.push((o0 & MASK_B) | (o3 & MASK_A));
        out.push((o1 & MASK_D) | (o3 & MASK_C));
        out.push((o2 & MASK_F) | (o3 & MASK_E));
    }
    out
}

/// 组装三段 + trailer → `a_bogus` 字符串（长度与原文守恒）。
///
/// 与真机原文不一定逐字符相同：garble 每次注入新的随机字节，
/// 故同一 payload 的 `a_bogus` 会变（这正是可用来做兼容性验证的性质）；
/// 但字符数与原文一致——`trailer` 原样回填，保证 `s4_encode` 的字节数不变。
pub fn encode(parts: &ABogusParts) -> String {
    let garbled = garble_3to4(&parts.payload);
    let mut inner = Vec::with_capacity(VERSION_LEN + garbled.len() + parts.trailer.len());
    inner.extend_from_slice(&parts.version);
    inner.extend_from_slice(&garbled);
    inner.extend_from_slice(&parts.trailer);

    let rc4 = rc4_variant(&inner, &[RC4_KEY]);
    let mut bytes = Vec::with_capacity(PREFIX_LEN + rc4.len());
    bytes.extend_from_slice(&parts.prefix);
    bytes.extend_from_slice(&rc4);
    s4_encode(&bytes)
}

/// 拆解 `a_bogus` 字符串 → 三段 + trailer（校验长度/字符集，失败给中文错误）。
pub fn decode(a_bogus: &str) -> Result<ABogusParts, String> {
    let bytes = s4_decode(a_bogus).ok_or_else(|| "a_bogus 字符集或长度非法".to_string())?;
    if bytes.len() < PREFIX_LEN + VERSION_LEN {
        return Err(format!(
            "a_bogus 解出字节过短：{} < {}",
            bytes.len(),
            PREFIX_LEN + VERSION_LEN
        ));
    }

    let mut prefix = [0u8; PREFIX_LEN];
    prefix.copy_from_slice(&bytes[..PREFIX_LEN]);

    let rc4 = rc4_variant(&bytes[PREFIX_LEN..], &[RC4_KEY]);
    let mut version = [0u8; VERSION_LEN];
    version.copy_from_slice(&rc4[..VERSION_LEN]);

    // RC4 段去掉 version 后：按 4 字节整组反解出 payload，最后一组之后的残字节存入
    // trailer（真机为 2 字节）。这样 encode 才能原样回填、保持总长不变。
    let rest = &rc4[VERSION_LEN..];
    let garble_len = rest.len() / 4 * 4;
    let payload = ungarble_3to4(&rest[..garble_len]);
    let trailer = rest[garble_len..].to_vec();

    Ok(ABogusParts {
        prefix,
        version,
        payload,
        trailer,
    })
}

/// 解码后重编码（同一 payload、不同随机位），用于兼容性验证。
pub fn reencode(a_bogus: &str) -> Result<String, String> {
    Ok(encode(&decode(a_bogus)?))
}

// ---------------- SM3 摘要与 URL 哈希（网关自签名的哈希地基） ----------------

/// SM3 全函数 IV（GB/T 32905-2016 规定的 8 个 32 位初始字）
const SM3_IV: [u32; 8] = [
    0x7380166f, 0x4914b2b9, 0x172442d7, 0xda8a0600, 0xa96f30bc, 0x163138aa, 0xe38dee4d, 0xb0fb0e4e,
];
/// SM3 常量 T_j：j∈[0,15] 用 [`SM3_T0`]，j∈[16,63] 用 [`SM3_T1`]
const SM3_T0: u32 = 0x79cc4519;
const SM3_T1: u32 = 0x7a879d8a;
/// URL 哈希的固定盐（真机逆向值：字符串 `dhzx`，非机密材料）
const URL_HASH_SALT: &[u8] = b"dhzx";

/// 32 位循环左移：用 `rotate_left`（内部按 `n mod 32` 处理，n=0 恒等），
/// 规避手写 `(x << n) | (x >> (32 - n))` 在 `n==0`（`>>32` 未定义）与 `n>=32` 的坑。
#[inline]
fn rotl(x: u32, n: u32) -> u32 {
    x.rotate_left(n)
}

/// SM3 置换函数 P0
#[inline]
fn sm3_p0(x: u32) -> u32 {
    x ^ rotl(x, 9) ^ rotl(x, 17)
}

/// SM3 置换函数 P1
#[inline]
fn sm3_p1(x: u32) -> u32 {
    x ^ rotl(x, 15) ^ rotl(x, 23)
}

/// SM3 压缩函数：以单个 64 字节分组更新链值 `v`（W/W' 消息扩展 + 64 轮迭代）
fn sm3_compress(v: &mut [u32; 8], block: &[u8]) {
    // W：前 16 字直接取分组（大端），其余按扩展式递推
    let mut w = [0u32; 68];
    for (i, word) in w.iter_mut().enumerate().take(16) {
        *word = u32::from_be_bytes([block[i * 4], block[i * 4 + 1], block[i * 4 + 2], block[i * 4 + 3]]);
    }
    for j in 16..68 {
        w[j] = sm3_p1(w[j - 16] ^ w[j - 9] ^ rotl(w[j - 3], 15)) ^ rotl(w[j - 13], 7) ^ w[j - 6];
    }
    // W'：W[j] ^ W[j+4]
    let mut w1 = [0u32; 64];
    for j in 0..64 {
        w1[j] = w[j] ^ w[j + 4];
    }

    let (mut a, mut b, mut c, mut d) = (v[0], v[1], v[2], v[3]);
    let (mut e, mut f, mut g, mut h) = (v[4], v[5], v[6], v[7]);
    for j in 0..64u32 {
        let t = if j < 16 { SM3_T0 } else { SM3_T1 };
        // 均为 u32 环绕加法（SM3 规定按 2^32 取模），用 wrapping_add 防溢出恐慌
        let ss1 = rotl(rotl(a, 12).wrapping_add(e).wrapping_add(rotl(t, j)), 7);
        let ss2 = ss1 ^ rotl(a, 12);
        let (ff, gg) = if j < 16 {
            (a ^ b ^ c, e ^ f ^ g)
        } else {
            ((a & b) | (a & c) | (b & c), (e & f) | ((!e) & g))
        };
        let tt1 = ff.wrapping_add(d).wrapping_add(ss2).wrapping_add(w1[j as usize]);
        let tt2 = gg.wrapping_add(h).wrapping_add(ss1).wrapping_add(w[j as usize]);
        d = c;
        c = rotl(b, 9);
        b = a;
        a = tt1;
        h = g;
        g = rotl(f, 19);
        f = e;
        e = sm3_p0(tt2);
    }
    v[0] ^= a;
    v[1] ^= b;
    v[2] ^= c;
    v[3] ^= d;
    v[4] ^= e;
    v[5] ^= f;
    v[6] ^= g;
    v[7] ^= h;
}

/// SM3 摘要（GB/T 32905-2016），返回 32 字节。
///
/// 消息填充：追加 `0x80` → 补零至 `len ≡ 56 (mod 64)` → 追加 64 位大端**比特**长度。
pub fn sm3(data: &[u8]) -> [u8; 32] {
    let mut v = SM3_IV;
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        sm3_compress(&mut v, block);
    }

    let mut out = [0u8; 32];
    for (i, word) in v.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// 嵌套双哈希：`SM3(SM3(data))`
pub fn sm3_nested(data: &[u8]) -> [u8; 32] {
    sm3(&sm3(data))
}

/// 计算豆包 URL 哈希三字节，按 `a_bogus` payload 的落地顺序返回 `[h[18], h[3], h[9]]`。
///
/// 其中 `h = SM3(SM3(<params> + "dhzx"))`（嵌套双哈希、固定盐）。入参 `params` 为
/// 「query 去掉 `a_bogus=` 之后」的原样字符串（**保持原百分号编码与参数顺序，不要重编码**）。
pub fn url_hash_triplet(params: &str) -> [u8; 3] {
    let mut input = Vec::with_capacity(params.len() + URL_HASH_SALT.len());
    input.extend_from_slice(params.as_bytes());
    input.extend_from_slice(URL_HASH_SALT);
    let h = sm3_nested(&input);
    [h[18], h[3], h[9]]
}

/// 用 uuid v4 字节流填满 `out`（随机源派生：uuid 已在依赖树，不新增 crate）。
fn random_fill(out: &mut [u8]) {
    let mut filled = 0usize;
    while filled < out.len() {
        let bytes = *uuid::Uuid::new_v4().as_bytes();
        let take = bytes.len().min(out.len() - filled);
        out[filled..filled + take].copy_from_slice(&bytes[..take]);
        filled += take;
    }
}

// ---------------- a_bogus 生成器（账户模板 → 按 query 生成签名） ----------------

/// 真机 payload 里**定长域**的长度（偏移 `0..49`；除 9/12/15 外不被服务端校验）。
const PREFIX50_LEN: usize = 50;
/// payload 末尾的 ASCII 数字位数（真机 3 位，服务端不校验）。
const TAIL_LEN: usize = 3;
/// 结构校验要求的设备串最小长度（真机 43 字符，这里只做保守下限）。
const MIN_DEVICE_LEN: usize = 3;

/// 账户级生成模板（由代理抓包得到；**不含任何凭据**，只是结构复用）。
///
/// 其中 `prefix` / `version` / `trailer` 随账号/版本/请求段固定，`prefix50` 是
/// payload 前 50 字节的「定长域」模板，`device` 是**与账号/机器绑定**的设备串
/// （真机为形如 `…|Win32` 的那段数字串，必须原样复用才能通过服务端校验）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ABogusTemplate {
    /// 前 4 字节头（随请求变化，原样透传）
    pub prefix: [u8; 4],
    /// 8 字节版本段
    pub version: [u8; 8],
    /// RC4 段末尾残字节（长度守恒用；真机 2 字节）
    pub trailer: Vec<u8>,
    /// 定长域模板（长度恒为 50；其中 9/12/15 会被 [`generate`] 覆盖为本次 query 的哈希三字节）
    pub prefix50: [u8; 50],
    /// 设备串（与账号指纹绑定，例如含 `…|Win32` 的那一段）
    pub device: String,
}

/// 由一条真实 `a_bogus` 反推出模板：`prefix`/`version`/`trailer` 取 [`decode`] 结果；
/// `prefix50` 取 `payload[0..50]`；`device` 取 `payload[50..len-3]`。
///
/// 结构校验：`payload` 须满足「定长域 50 + 设备串(≥3) + 尾部 3」的最短长度，
/// 且设备串须为合法 UTF-8；否则返回 `Err`（中文错误）。
pub fn extract_template(a_bogus: &str) -> Result<ABogusTemplate, String> {
    let parts = decode(a_bogus)?;
    let min_len = PREFIX50_LEN + MIN_DEVICE_LEN + TAIL_LEN;
    if parts.payload.len() < min_len {
        return Err(format!(
            "payload 过短，不符合『定长域{}+设备串+尾部{}』结构：{} < {}",
            PREFIX50_LEN,
            TAIL_LEN,
            parts.payload.len(),
            min_len
        ));
    }

    let mut prefix50 = [0u8; PREFIX50_LEN];
    prefix50.copy_from_slice(&parts.payload[..PREFIX50_LEN]);

    let device_end = parts.payload.len() - TAIL_LEN;
    let device = String::from_utf8(parts.payload[PREFIX50_LEN..device_end].to_vec())
        .map_err(|_| "设备串不是合法 UTF-8".to_string())?;

    Ok(ABogusTemplate {
        prefix: parts.prefix,
        version: parts.version,
        trailer: parts.trailer,
        prefix50,
        device,
    })
}

/// 按本次请求的 query 参数生成 `a_bogus`。
///
/// `params`：query 去掉 `a_bogus=` 之后的原样字符串（**保持原百分号编码与顺序，勿再编码**）。
/// `now_ms`：当前毫秒时间戳，仅用于生成末尾 3 位数字（该字段服务端不校验，故只要 3 位十进制即可）。
///
/// 组装：`payload = prefix50`（其中 `payload[9]=t[0]; payload[12]=t[1]; payload[15]=t[2]`，
/// `t = url_hash_triplet(params)`）`++ device.as_bytes() ++ 3位十进制`，
/// 再用 [`encode`] 输出（payload 非 3 倍数时按既有补齐语义处理；真机 device 为 43 字符时正好 96）。
pub fn generate(params: &str, tpl: &ABogusTemplate, now_ms: i64) -> String {
    let t = url_hash_triplet(params);
    let mut payload = tpl.prefix50.to_vec();
    payload[9] = t[0];
    payload[12] = t[1];
    payload[15] = t[2];
    payload.extend_from_slice(tpl.device.as_bytes());
    // `now_ms & 0xff` 恒为 0..=255（i64 按位与后非负），`{:03}` 即得 3 位十进制
    let tail = format!("{:03}", (now_ms & 0xff) as u8);
    payload.extend_from_slice(tail.as_bytes());

    encode(&ABogusParts {
        prefix: tpl.prefix,
        version: tpl.version,
        payload,
        trailer: tpl.trailer.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合成字节（不依赖任何真实样本）：按偏移做伪随机填充
    fn synthetic(len: usize, seed: u8) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed)).collect()
    }

    /// 仿造真机 `1|2|3|…|Win32` 形态的**合成**设备串（不含任何真实指纹）。
    /// 长度调整到 `≡1 (mod 3)`，使 `payload = 50 + device + 3` 为 3 的倍数
    /// （真机 43 字符同样满足该同余，故 decode 不带补齐、往返无损）。
    fn synthetic_device() -> String {
        let mut nums: Vec<String> = (1..=8).map(|n| n.to_string()).collect();
        let mut dev = format!("{}|Win32", nums.join("|"));
        while dev.len() % 3 != 1 {
            nums.push("0".to_string());
            dev = format!("{}|Win32", nums.join("|"));
        }
        dev
    }

    /// 合成模板：prefix50 全部非零但**不含任何真实数据**；device 为合成串。
    fn synthetic_template() -> ABogusTemplate {
        let mut prefix50 = [0u8; PREFIX50_LEN];
        for (i, slot) in prefix50.iter_mut().enumerate() {
            // `| 1` 保证每字节非零（真机该域除 9/12/15 外本就不被校验）
            *slot = (i as u8).wrapping_mul(7).wrapping_add(1) | 1;
        }
        ABogusTemplate {
            prefix: [0x21, 0x43, 0x65, 0x87],
            version: [0x0A, 0x1B, 0x2C, 0x3D, 0x4E, 0x5F, 0x60, 0x71],
            trailer: vec![0xC3, 0x3C],
            prefix50,
            device: synthetic_device(),
        }
    }

    /// 百分号解码（仅诊断测试用）：MITM 明细日志记录的是 URL 编码形态的 a_bogus
    fn pct_decode(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'%' && i + 2 < b.len() {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
            out.push(b[i]);
            i += 1;
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// 两张自定义表都必须是 64 个互异字符——这是 `STD[i] → table[i]` 可逆的前提。
    ///
    /// 注意 s4 在标准表索引 27（标准字符集中的 `b`）上放的是 `-`：s4 的字符集并不等于
    /// 标准字符集（它缺 `+`、多 `-`），但只要 64 个字符互异，置换仍是双射、编解码仍可逆，
    /// 所以这里只校验互异性与「`=` 不被占用（`=` 是补齐符）」。
    #[test]
    fn tables_have_distinct_chars() {
        for table in [S4_TABLE, S3_TABLE] {
            assert_eq!(table.len(), 64);
            let mut seen = [false; 256];
            for &c in table.iter() {
                assert_ne!(c, b'=', "补齐符 '=' 不能占用表内位置");
                assert!(!seen[c as usize], "自定义表存在重复字符");
                seen[c as usize] = true;
            }
        }
    }

    /// 掩码前提：三组互补且两两相与为 0（反解正确性的基础）
    #[test]
    fn masks_are_complementary() {
        assert_eq!(MASK_A | MASK_B, 255);
        assert_eq!(MASK_C | MASK_D, 255);
        assert_eq!(MASK_E | MASK_F, 255);
        assert_eq!(MASK_A & MASK_B, 0);
        assert_eq!(MASK_C & MASK_D, 0);
        assert_eq!(MASK_E & MASK_F, 0);
    }

    /// s4 / s3 编解码互逆：覆盖长度 0/1/2/3 与较长数据
    #[test]
    fn table_encode_decode_roundtrip() {
        for (table_encode, table_decode) in [
            (s4_encode as fn(&[u8]) -> String, s4_decode as fn(&str) -> Option<Vec<u8>>),
            (s3_encode as fn(&[u8]) -> String, s3_decode as fn(&str) -> Option<Vec<u8>>),
        ] {
            for len in [0usize, 1, 2, 3, 4, 96, 142] {
                let data = synthetic(len, 0x5A);
                let text = table_encode(&data);
                let back = table_decode(&text).expect("合法字符应能解码");
                assert_eq!(back, data, "长度 {len} 往返应一致");
            }
        }
    }

    /// 非法字符集与非法长度应返回 None
    #[test]
    fn table_decode_rejects_garbage() {
        // '?' 不在 s4 表内
        assert!(s4_decode("????").is_none());
        // 长度非 4 的倍数（标准 base64 解码会失败）
        assert!(s4_decode("Dkdp").is_some());
        assert!(s4_decode("Dkd").is_none());
        // 空串是合法的空数据
        assert_eq!(s4_decode("").unwrap_or_default(), Vec::<u8>::new());
    }

    /// garble / ungarble 互逆：payload 长度取 3 的倍数，
    /// 多次调用（随机注入不同）都必须还原同一 data
    #[test]
    fn garble_ungarble_roundtrip() {
        for len in [0usize, 3, 9, 96, 141] {
            let data = synthetic(len, 0x11);
            for _ in 0..8 {
                let garbled = garble_3to4(&data);
                assert_eq!(garbled.len(), len / 3 * 4);
                assert_eq!(ungarble_3to4(&garbled), data);
            }
        }
    }

    /// RC4 变体自反：rc4(rc4(x)) == x
    #[test]
    fn rc4_variant_is_involutive() {
        for len in [0usize, 1, 8, 130, 200] {
            let data = synthetic(len, 0x3C);
            let once = rc4_variant(&data, &[RC4_KEY]);
            let twice = rc4_variant(&once, &[RC4_KEY]);
            assert_eq!(twice, data);
        }
    }

    /// 核心验收：整体 encode → decode 后三段 + trailer 一致；
    /// 再 encode(decode) → decode 仍收敛到同一结果（garble 随机注入只改变字符串）
    #[test]
    fn abogus_encode_decode_stable() {
        // payload 取 3 的倍数（真机为 96），保证不带补齐、往返无损
        let parts = ABogusParts {
            prefix: [0x01, 0x02, 0x03, 0x04],
            version: [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
            payload: synthetic(96, 0x77),
            trailer: vec![0x5A, 0xA5], // 与真机同款 2 字节
        };

        let first = encode(&parts);
        // 真机同款组合：4 + 8 + 128 + 2 = 142 字节 → 192 字符
        assert_eq!(first.len(), 192);
        let decoded = decode(&first).expect("自产 a_bogus 应可解码");
        assert_eq!(decoded.prefix, parts.prefix);
        assert_eq!(decoded.version, parts.version);
        assert_eq!(decoded.payload, parts.payload);
        assert_eq!(decoded.trailer, parts.trailer);

        // 重编码（随机位变化）后再解码，四段仍逐一相等、长度不变
        let second = encode(&decoded);
        assert_eq!(second.len(), first.len());
        let decoded2 = decode(&second).expect("重编码结果应可解码");
        assert_eq!(decoded2.prefix, parts.prefix);
        assert_eq!(decoded2.version, parts.version);
        assert_eq!(decoded2.payload, parts.payload);
        assert_eq!(decoded2.trailer, parts.trailer);

        // reencode 便捷函数等价于 encode(decode(..))，且长度守恒
        let reencoded = reencode(&first).expect("reencode 应成功");
        assert_eq!(reencoded.len(), first.len());
        assert_eq!(decode(&reencoded).expect("应可解码"), decoded);
    }

    /// 长度守恒：对多组合成 parts（trailer 0~3 字节、payload 为 3 的倍数），
    /// `encode(decode(s)).len() == s.len()`，保证用它替换真机请求时长度不失真
    #[test]
    fn abogus_length_is_conserved() {
        for trailer_len in 0usize..=3 {
            for payload_len in [0usize, 3, 48, 96, 141] {
                let parts = ABogusParts {
                    prefix: [0x11, 0x22, 0x33, 0x44],
                    version: [0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF],
                    payload: synthetic(payload_len, 0x6D),
                    trailer: synthetic(trailer_len, 0xC3),
                };
                let text = encode(&parts);
                let decoded = decode(&text).expect("应可解码");
                assert_eq!(decoded.trailer, parts.trailer, "trailer 应完整保留");
                assert_eq!(decoded.payload, parts.payload, "payload 应无损往返");
                let again = encode(&decoded);
                assert_eq!(
                    again.len(),
                    text.len(),
                    "trailer={trailer_len} payload={payload_len} 时长度应守恒"
                );
            }
        }
    }

    /// 三段 + trailer 全等往返：`decode(encode(decode(s)))` 与首次 decode 逐一相等
    #[test]
    fn abogus_parts_stable_through_reencode() {
        for trailer_len in 0usize..=3 {
            let parts = ABogusParts {
                prefix: [0x0F, 0x1E, 0x2D, 0x3C],
                version: [0xAA, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10, 0x11],
                payload: synthetic(96, 0x42),
                trailer: synthetic(trailer_len, 0x24),
            };
            let text = encode(&parts);
            let first = decode(&text).expect("应可解码");
            let second_text = encode(&first);
            let second = decode(&second_text).expect("应可解码");
            assert_eq!(second.prefix, first.prefix);
            assert_eq!(second.version, first.version);
            assert_eq!(second.payload, first.payload);
            assert_eq!(second.trailer, first.trailer);
            assert_eq!(second_text.len(), text.len());
        }
    }

    /// 合成 a_bogus：先 encode 合成 parts 再 decode，断言四段一致
    #[test]
    fn abogus_synthetic_roundtrip() {
        let parts = ABogusParts {
            prefix: [0xAA, 0xBB, 0xCC, 0xDD],
            version: [1, 2, 3, 4, 5, 6, 7, 8],
            payload: synthetic(48, 0x9E), // 48 是 3 的倍数
            trailer: synthetic(3, 0x1F),
        };
        let text = encode(&parts);
        let back = decode(&text).expect("应可解码");
        assert_eq!(back, parts);
    }

    /// decode 的健壮性：非法字符集报错，过短数据报错
    #[test]
    fn decode_rejects_bad_input() {
        assert!(decode("????").is_err());
        // 4 个 s4 字符 = 3 字节 < 12 字节下限
        assert!(decode(&s4_encode(&[0, 1, 2])).is_err());
    }

    /// 临时诊断（`#[ignore]`，仅本机跑）：用真机黄金样本校验 Rust 解码与逆向结论一致。
    /// 从 MITM 明细日志取一条 `/chat/completion` 的 a_bogus，解码后打印 payload 长度与
    /// ASCII 段（**只打印非敏感元数据/结构，不落盘**）。验收基准：payload=96、trailer=2，
    /// ASCII 段应含 `1237|808|...|Win32`。
    /// 运行：`cargo test --no-default-features doubao_abogus::tests::golden_sample_decode -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn golden_sample_decode() {
        let dir = std::env::var("APPDATA").expect("APPDATA 缺失");
        let path = std::path::Path::new(&dir)
            .join("AIWorkAssistant")
            .join("logs")
            .join("proxy_req_2026-10-16.log");
        let raw = std::fs::read(&path).expect("读取代理明细日志失败");
        let text = String::from_utf8_lossy(&raw);
        let mut found = 0usize;
        let mut nonstd = 0usize;
        for line in text.lines() {
            if !line.contains("chat/completion?") {
                continue;
            }
            let Some(q) = line.split_once("chat/completion?").map(|x| x.1) else {
                continue;
            };
            let Some(ab_raw) = q
                .split(|c: char| c == '&' || c.is_whitespace())
                .find_map(|kv| kv.strip_prefix("a_bogus="))
            else {
                continue;
            };
            // 日志里 a_bogus 是 URL 编码形态（含 %2F 等），先百分号解码再按字符集解码
            let ab = pct_decode(ab_raw);
            if ab.len() != 192 {
                nonstd += 1;
                if nonstd <= 6 {
                    println!("非 192 长度样本: raw={} decoded={}", ab_raw.len(), ab.len());
                }
                continue;
            }
            match decode(&ab) {
                Ok(p) => {
                    found += 1;
                    let ascii: String = p
                        .payload
                        .iter()
                        .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
                        .collect();
                    println!(
                        "样本#{found} payload={} trailer={} version={:02x?} ascii={}",
                        p.payload.len(),
                        p.trailer.len(),
                        p.version,
                        ascii
                    );
                }
                Err(e) => println!("样本#{found} decode 失败: {e}"),
            }
        }
        println!("共解析 {found} 条 192 字符样本");
        assert!(found > 0, "未从日志解析到 192 字符的 a_bogus 样本");
    }

    /// 从 query（原样百分号编码）中整体移除 `&a_bogus=<原样百分号编码串>` 参数段，
    /// 返回 `(params, a_bogus 原样编码值)`。
    ///
    /// 按「整体移除该参数（不先解码再移除）」处理：保留其余参数的原始编码与顺序。
    /// 值以「下一个 `&` 或串尾」为界（a_bogus 值本身为百分号编码，不含裸 `&`）。
    fn strip_abogus_param(query: &str) -> Option<(String, String)> {
        let (start, val_start) = if let Some(i) = query.find("&a_bogus=") {
            (i, i + "&a_bogus=".len())
        } else if let Some(v) = query.strip_prefix("a_bogus=") {
            (0, query.len() - v.len())
        } else {
            return None;
        };
        let val_end = query[val_start..]
            .find('&')
            .map(|i| val_start + i)
            .unwrap_or(query.len());
        let ab_raw = query[val_start..val_end].to_string();
        let params = format!("{}{}", &query[..start], &query[val_end..]);
        Some((params, ab_raw))
    }

    /// SM3 标准空串向量（GB/T 32905-2016）：`sm3(b"")` 的十六进制应等于权威向量。
    #[test]
    fn sm3_empty_vector() {
        let hex: String = sm3(b"").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            hex, "1ab21d8355cfa17f8e61194831e81a8f22bec8c728fefb747ed035eb5082aa2b"
        );
    }

    /// URL 哈希真机比对（`#[ignore]`，仅本机跑）：从 MITM 明细日志取真机 `/chat/completion`
    /// 样本，按「移除 a_bogus 参数后的 params」算 `url_hash_triplet`，断言三个落地字节分别
    /// 等于 payload 的 `[9]`、`[12]`、`[15]`。要求至少 3 条样本全中。
    ///
    /// 只打印偏移与是否相等这类**非敏感元数据**，**绝不打印 params / a_bogus / 哈希值**。
    /// 运行：`cargo test --no-default-features doubao_abogus::tests::url_hash_matches_golden -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn url_hash_matches_golden() {
        let dir = std::env::var("APPDATA").expect("APPDATA 缺失");
        let path = std::path::Path::new(&dir)
            .join("AIWorkAssistant")
            .join("logs")
            .join("proxy_req_2026-10-16.log");
        let raw = std::fs::read(&path).expect("读取代理明细日志失败");
        let text = String::from_utf8_lossy(&raw);
        let mut decoded_samples = 0usize;
        let mut matched = 0usize;
        for line in text.lines() {
            if !line.contains("chat/completion?") {
                continue;
            }
            let Some(q) = line.split_once("chat/completion?").map(|x| x.1) else {
                continue;
            };
            // 移除 a_bogus 参数后得到喂给 SM3 的 params（保持原样百分号编码）
            let Some((params, ab_raw)) = strip_abogus_param(q) else {
                continue;
            };
            // 日志里 a_bogus 是 URL 编码形态：先百分号解码，长度须为真机的 192
            let ab = pct_decode(&ab_raw);
            if ab.len() != 192 {
                continue;
            }
            let Ok(parts) = decode(&ab) else {
                continue;
            };
            if parts.payload.len() < 16 {
                continue;
            }
            decoded_samples += 1;
            let h = url_hash_triplet(&params);
            let ok9 = parts.payload[9] == h[0];
            let ok12 = parts.payload[12] == h[1];
            let ok15 = parts.payload[15] == h[2];
            println!(
                "样本#{decoded_samples} 偏移9(←h18)={ok9} 偏移12(←h3)={ok12} 偏移15(←h9)={ok15}"
            );
            assert!(ok9, "偏移 9 与 h[18] 不一致");
            assert!(ok12, "偏移 12 与 h[3] 不一致");
            assert!(ok15, "偏移 15 与 h[9] 不一致");
            matched += 1;
        }
        println!("共比对 {matched} 条样本全部命中（解码样本 {decoded_samples} 条）");
        assert!(matched >= 3, "命中样本数 {matched} < 3，真机比对未通过");
    }

    /// generate 落地正确性：哈希三字节写入 9/12/15、设备串原样拼接、尾部 3 位 ASCII 数字，
    /// 且 payload 长度等于「定长域 50 + 设备串 + 尾部 3」。
    #[test]
    fn generate_fills_hash_and_device() {
        let tpl = synthetic_template();
        let params = "foo=1&bar=2&baz=abc";
        let now_ms: i64 = 1_700_000_000_123;

        let ab = generate(params, &tpl, now_ms);
        let parts = decode(&ab).expect("自产 a_bogus 应可解码");

        assert_eq!(parts.prefix, tpl.prefix);
        assert_eq!(parts.version, tpl.version);
        assert_eq!(parts.trailer, tpl.trailer);
        assert_eq!(
            parts.payload.len(),
            PREFIX50_LEN + tpl.device.len() + TAIL_LEN,
            "payload 长度应为 定长域50 + 设备串 + 尾部3"
        );

        let t = url_hash_triplet(params);
        assert_eq!(parts.payload[9], t[0], "偏移 9 应等于 h[18]");
        assert_eq!(parts.payload[12], t[1], "偏移 12 应等于 h[3]");
        assert_eq!(parts.payload[15], t[2], "偏移 15 应等于 h[9]");

        let dev_start = PREFIX50_LEN;
        let dev_end = dev_start + tpl.device.len();
        assert_eq!(
            &parts.payload[dev_start..dev_end],
            tpl.device.as_bytes(),
            "设备串应原样拼接在定长域之后"
        );
        for &b in &parts.payload[dev_end..] {
            assert!((b'0'..=b'9').contains(&b), "末尾 3 字节应为 ASCII 数字");
        }
    }

    /// extract_template 与 generate 的往返：`generate` 产出 → `extract_template` → 再 `generate`
    /// 同参，两次 decode 的三段 + trailer 一致（a_bogus 字符串因 garble 随机位可不同）。
    #[test]
    fn generate_and_extract_template_roundtrip() {
        let tpl = synthetic_template();
        let params = "q=rust&page=1";
        let now_ms: i64 = 1_600_000_000_000;

        let first = generate(params, &tpl, now_ms);
        let tpl2 = extract_template(&first).expect("应能从合成样本反推模板");

        // 反推模板应与原模板各字段一致
        assert_eq!(tpl2.prefix, tpl.prefix);
        assert_eq!(tpl2.version, tpl.version);
        assert_eq!(tpl2.trailer, tpl.trailer);
        assert_eq!(tpl2.device, tpl.device);
        // prefix50 只应在 9/12/15 三处不同（generate 已把本次哈希写入），其余相同
        for i in 0..PREFIX50_LEN {
            if i == 9 || i == 12 || i == 15 {
                continue;
            }
            assert_eq!(tpl2.prefix50[i], tpl.prefix50[i], "偏移 {i} 应保持不变");
        }

        let second = generate(params, &tpl2, now_ms);
        let a = decode(&first).expect("应可解码");
        let b = decode(&second).expect("应可解码");
        assert_eq!(a.prefix, b.prefix);
        assert_eq!(a.version, b.version);
        assert_eq!(a.payload, b.payload);
        assert_eq!(a.trailer, b.trailer);
    }

    /// extract_template 对结构不符的输入返回 Err：payload 裁到 < 56 字节、或字符集非法。
    #[test]
    fn extract_template_rejects_malformed() {
        // 结构不符：payload 只留 54 字节（50 定长 + 1 设备 + 3 尾部亦不足，< 最短 56）
        let parts = ABogusParts {
            prefix: [0x01, 0x02, 0x03, 0x04],
            version: [0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80],
            payload: synthetic(54, 0x5E), // 54 为 3 的倍数，decode 不带补齐
            trailer: vec![0xAA],
        };
        let ab = encode(&parts);
        assert!(extract_template(&ab).is_err(), "payload 过短应返回 Err");

        // 非法字符集同样报错
        assert!(extract_template("????").is_err(), "非法字符集应返回 Err");
    }
}
