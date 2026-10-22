//! 网关工具代执行（T5.5/F-64）
//!
//! 「上游不支持的工具调用在代理侧补齐」范式（muskke/trae-api-proxy 设计吸收，
//! learn-the-design 不抄码）。客户端（Codex /v1/responses）声明 `type:"web_search"`
//! 等上游不支持的工具时：
//! 1. 代理向 chat body 注入同义 function 工具（web_search / open_url）+ system 提示；
//! 2. 上游模型发起 function 调用 → 代理本地执行（**Bing HTML 搜索，DDG 回退** /
//!    页面抓取剥离，零新增依赖，ureq 已有）；
//! 3. 结果以 tool 消息回喂上游继续，直至最终回复（上限 MAX_ROUNDS 轮防积分失控）；
//! 4. 响应投影：历史调用轮以原生 `web_search_call` 输出项返回（Responses 协议），
//!    最终回复按既有 completion_to_responses 投影。
//!
//! 红线：仅代理注入的这两个工具会被代执行；客户端声明的真实 function 工具
//! 照常透传上游，由客户端自行执行。搜索请求单次单发、不重试。

use serde_json::{json, Value};

pub const TOOL_SEARCH: &str = "web_search";
pub const TOOL_OPEN_URL: &str = "open_url";
/// 代执行轮数上限（每轮 = 一次上游请求，积分成本有界）
pub const MAX_ROUNDS: usize = 3;
/// open_url 页面正文截断（字符）
const PAGE_TEXT_LIMIT: usize = 4000;
/// 搜索结果条数上限
const SEARCH_RESULTS_LIMIT: usize = 5;
/// 抓取响应体读取上限（2MB）：恶意/异常超大页面不得拖垮代理内存与模型上下文
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Responses 请求是否声明了上游不支持的 web_search 类工具
pub fn responses_declares_web_search(body: &Value) -> bool {
    body.get("tools")
        .and_then(|t| t.as_array())
        .map_or(false, |arr| {
            arr.iter()
                .any(|t| matches!(t.get("type").and_then(|v| v.as_str()), Some("web_search") | Some("web_search_preview")))
        })
}

/// 向 chat body（responses_to_chat 产物）注入代理 function 工具 + system 提示。
/// 返回是否可代执行：客户端已声明同名 function 工具时返回 false 且不做任何
/// 注入（红线：仅代理注入的工具可代执行——同名工具属客户端自有语义，代理
/// 不得按名劫持，调用方须跳过代执行循环，工具调用照常透传客户端自行执行）
pub fn inject_proxy_tools(chat_body: &mut Value) -> bool {
    let Some(obj) = chat_body.as_object_mut() else { return false };
    let tools = obj.entry("tools".to_string()).or_insert_with(|| json!([]));
    if let Some(arr) = tools.as_array_mut() {
        let has = |arr: &[Value], name: &str| -> bool {
            arr.iter()
                .any(|t| t.pointer("/function/name").and_then(|n| n.as_str()) == Some(name))
        };
        // P1-6 劫持防护：任一同名冲突即整体退出（部分注入会造成「半个代理」的
        // 混合语义，且未注入的那个仍会被 extract_proxy_calls 按名劫持）
        if has(arr, TOOL_SEARCH) || has(arr, TOOL_OPEN_URL) {
            return false;
        }
        arr.push(json!({
            "type": "function",
            "function": {
                "name": TOOL_SEARCH,
                "description": "Search the web for up-to-date information. Returns a numbered list of results (title, url, snippet).",
                "parameters": {
                    "type": "object",
                    "properties": { "query": { "type": "string", "description": "Search query" } },
                    "required": ["query"],
                },
            },
        }));
        arr.push(json!({
            "type": "function",
            "function": {
                "name": TOOL_OPEN_URL,
                "description": "Fetch a web page by URL and return its readable text content.",
                "parameters": {
                    "type": "object",
                    "properties": { "url": { "type": "string", "description": "Absolute http(s) URL" } },
                    "required": ["url"],
                },
            },
        }));
    }
    // system 提示：插在首个非 system 消息之前（保留既有 instructions 语义）
    if let Some(msgs) = obj.get_mut("messages").and_then(|m| m.as_array_mut()) {
        let hint = json!({
            "role": "system",
            "content": "Web research tools (web_search, open_url) are executed by the serving proxy. Call web_search with a concise query when current information is needed, and open_url to read a specific page. Wait for tool results before answering.",
        });
        let insert_at = msgs
            .iter()
            .position(|m| m.get("role").and_then(|r| r.as_str()) != Some("system"))
            .unwrap_or(msgs.len());
        msgs.insert(insert_at, hint);
    }
    true
}

/// 代执行记录（供 Responses web_search_call 输出项投影）
#[derive(Debug, Clone)]
pub struct SearchRecord {
    pub tool: String,
    pub query: String,
    /// 执行是否成功（保留字段：调试与后续 usage 归因扩展用）
    #[allow(dead_code)]
    pub ok: bool,
}

/// 从聚合 completion 提取代理工具调用：(call_id, name, arguments)
pub fn extract_proxy_calls(completion: &Value) -> Vec<(String, String, String)> {
    completion
        .pointer("/choices/0/message/tool_calls")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|tc| {
                    let name = tc.pointer("/function/name").and_then(|n| n.as_str())?;
                    if name != TOOL_SEARCH && name != TOOL_OPEN_URL {
                        return None;
                    }
                    let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                    let args = tc
                        .pointer("/function/arguments")
                        .and_then(|a| a.as_str())
                        .unwrap_or("{}")
                        .to_string();
                    Some((id, name.to_string(), args))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 执行代理工具调用；失败以文本错误返回（回喂模型自行决策，不中断轮次）
pub fn execute(name: &str, arguments: &str) -> (String, bool) {
    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    match name {
        TOOL_SEARCH => {
            let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("").trim();
            if q.is_empty() {
                return ("error: empty query".into(), false);
            }
            let r = web_search(q);
            let ok = r.is_ok();
            (r.unwrap_or_else(|e| format!("error: {e}")), ok)
        }
        TOOL_OPEN_URL => {
            let u = args.get("url").and_then(|v| v.as_str()).unwrap_or("").trim();
            if u.is_empty() {
                return ("error: empty url".into(), false);
            }
            let r = open_url(u);
            let ok = r.is_ok();
            (r.unwrap_or_else(|e| format!("error: {e}")), ok)
        }
        _ => (format!("error: unknown proxy tool {name}"), false),
    }
}

/// 极简 percent-encode（查询串安全子集之外全编码）
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ==================== SSRF 防护（抓取前 host 校验） ====================

/// 抓取 URL 校验（SSRF 防护）：仅 http/https；host 判定基于 **WHATWG 解析后**的
/// host（与 ureq 请求层同源，审查 F1）——不能用字符串切分：`http://127.0.0.1\@example.com`
/// 在 WHATWG 里 host 是 `127.0.0.1`（`\` 等价 `/`），`rsplit('@')` 却会误读成
/// `example.com`，形成校验与请求不一致的绕过；`localhost.` 带尾点同理。
pub fn url_allowed(url: &str) -> Result<(), String> {
    let u = url::Url::parse(url.trim()).map_err(|_| "URL 格式无效".to_string())?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err("仅支持 http/https URL".into());
    }
    if u.host().map(host_allowed).unwrap_or(false) {
        Ok(())
    } else {
        Err("拒绝访问受限地址（SSRF 防护）".into())
    }
}

/// host 白名单判定（解析后 host）：true = 允许抓取。
/// 域名：拦 localhost / *.localhost / *.local / *.internal（含尾点归一）；
/// IP 字面量按解析后的 Ipv4Addr / Ipv6Addr 网段精确比对——IPv4 变体
///（十六进制 0x7f.1 / 八进制 0177.0.0.1 / 缺段 127.1 / 整型 2130706433）
/// 由 WHATWG 解析统一归一，天然覆盖；解析失败 fail-closed
fn host_allowed(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            !(d == "localhost"
                || d.ends_with(".localhost")
                || d.ends_with(".local")
                || d.ends_with(".internal"))
        }
        url::Host::Ipv4(ip) => ipv4_allowed_ip(ip),
        url::Host::Ipv6(ip) => ipv6_allowed_ip(ip),
    }
}

/// IPv4 私网/环回/链路本地/CGNAT/未指定判定（解析后按八位组精确比对）
fn ipv4_allowed_ip(ip: std::net::Ipv4Addr) -> bool {
    let o = ip.octets();
    !(o[0] == 0
        || o[0] == 10
        || o[0] == 127
        || (o[0] == 100 && (64..=127).contains(&o[1]))  // CGNAT 100.64/10（Tailscale 等内网段）
        || (o[0] == 169 && o[1] == 254)
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 172 && (16..=31).contains(&o[1])))
}

/// IPv6 环回/未指定/唯一本地/链路本地/IPv4 映射环回判定
fn ipv6_allowed_ip(ip: std::net::Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return false;
    }
    let s = ip.segments();
    // fc00::/7 唯一本地（fc** / fd**）
    if (s[0] & 0xfe00) == 0xfc00 {
        return false;
    }
    // fe80::/10 链路本地
    if (s[0] & 0xffc0) == 0xfe80 {
        return false;
    }
    // ::ffff:0:0/96 IPv4 映射：内嵌 IPv4 复检
    if let Some(v4) = ip.to_ipv4_mapped() {
        return ipv4_allowed_ip(v4);
    }
    true
}

/// 限量读取响应体：累计达 MAX_BODY_BYTES 即截断；无效 UTF-8 宽容替换
fn read_body_limited(resp: ureq::Response) -> Result<String, String> {
    use std::io::Read;
    let mut reader = resp.into_reader();
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let remain = MAX_BODY_BYTES.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(remain)]);
                if buf.len() >= MAX_BODY_BYTES {
                    break; // 截断
                }
            }
            Err(e) => return Err(format!("响应读取失败: {e}")),
        }
    }
    Ok(String::from_utf8_lossy(&buf).to_string())
}

/// 联网搜索：**Bing 优先，DuckDuckGo 回退**（单次请求内串行，不重试单源）。
///
/// 为什么换主源（2026-10-21 实测）：DuckDuckGo 的 html/lite 两个端点在
/// 国内网络环境下均 **连接超时**（本机 8s 超时未握手），即原实现实际不可用——
/// Codex 等客户端声明 web_search 后只会拿到「搜索请求失败」。
/// Bing 可达（200 / ~100KB / 稳定 10 条结果），且结果块结构稳定：
/// `<li class="b_algo"> … <h2><a href=…>标题</a></h2> … <p class="b_lineclamp2">摘要`。
///
/// 保留 DDG 回退的原因：Bing 对高频抓取可能返回验证页（无 b_algo 块）；
/// 而 DDG 在能连通的环境（海外/代理）质量更好。两个都失败才明示失败。
fn web_search(query: &str) -> Result<String, String> {
    let mut errors: Vec<String> = Vec::new();

    match search_bing(query) {
        Ok(results) if !results.is_empty() => return format_results(&results),
        Ok(_) => errors.push("Bing 无结果".into()),
        Err(e) => errors.push(format!("Bing: {e}")),
    }

    match search_ddg(query) {
        Ok(results) if !results.is_empty() => return format_results(&results),
        Ok(_) => errors.push("DuckDuckGo 无结果".into()),
        Err(e) => errors.push(format!("DuckDuckGo: {e}")),
    }

    Err(format!(
        "搜索不可用（{}）。可稍后重试，或直接回答已知信息",
        errors.join("；")
    ))
}

/// 统一搜索结果输出格式（供上游模型阅读）
fn format_results(results: &[(String, String, String)]) -> Result<String, String> {
    if results.is_empty() {
        return Err("搜索无结果".into());
    }
    let mut out = String::new();
    for (i, (title, url, snippet)) in results.iter().take(SEARCH_RESULTS_LIMIT).enumerate() {
        out.push_str(&format!("{}. {}\n   {}\n   {}\n\n", i + 1, title, url, snippet));
    }
    Ok(out)
}

/// Bing 搜索（HTML 版；单发不重试）
fn search_bing(query: &str) -> Result<Vec<(String, String, String)>, String> {
    // 必须用 **cn.bing.com**：`www.bing.com` 在国内返回 302「Object moved」
    // （2026-10-21 实测，body 仅 200 字节），而本函数为了 SSRF 防护用
    // `redirects(0)` 不跟随跳转，等于每次都只拿到空壳页。
    // cn.bing.com 直返 200 / ~100KB / 11 个 b_algo 块。
    //
    // `setlang=zh-CN` + `cc=CN`：中文结果优先，与用户语言一致
    let url = format!(
        "https://cn.bing.com/search?q={}&setlang=zh-CN&cc=CN",
        url_encode(query)
    );
    url_allowed(&url)?;
    // redirects(0) 与 open_url 同款（SSRF 补强）：302 可被引导至内网盲探测面
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(15))
        .redirects(0)
        .build();
    let resp = agent
        .get(&url)
        .set("User-Agent", BROWSE_UA)
        .set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .call()
        .map_err(|e| format!("{e}"))?;
    let html = read_body_limited(resp)?;
    let results = parse_bing_results(&html);
    if results.is_empty() {
        // 验证页/限流的典型表现是「页面正常但无结果块」，此处无法与「真的没结果」区分
        return Err("无结果块（可能触发人机验证）".into());
    }
    Ok(results)
}

/// DuckDuckGo HTML 搜索（海外/代理环境可用；单发不重试）
fn search_ddg(query: &str) -> Result<Vec<(String, String, String)>, String> {
    let url = format!("https://html.duckduckgo.com/html/?q={}", url_encode(query));
    url_allowed(&url)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(15))
        .redirects(0)
        .build();
    let resp = agent
        .get(&url)
        .set("User-Agent", BROWSE_UA)
        .set("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
        .call()
        .map_err(|e| format!("{e}"))?;
    let html = read_body_limited(resp)?;
    let results = parse_ddg_results(&html);
    if results.is_empty() {
        return Err("无结果（可能限流）".into());
    }
    Ok(results)
}

/// 浏览器 UA：Bing 对非浏览器 UA 直接回验证页，必须伪装
const BROWSE_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

/// 从 Bing HTML 提取 (title, url, snippet)。
///
/// 结构（2026-10-21 实测）：每个结果块以 `<li class="b_algo"` 开头，
/// 内部 `<h2><a href="真实URL">标题</a></h2>`，摘要常见于
/// `b_lineclamp*` / `b_caption` 包裹的 `<p>`。
/// 注意：`b_algo` 之后紧跟若干 `<link rel=stylesheet>`，故必须用 `[\s\S]*?`
/// 跨过它们再找 `h2`，且限定在同一结果块内（以 `</li>` 为界）。
fn parse_bing_results(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while out.len() < SEARCH_RESULTS_LIMIT {
        let Some(rel) = html[from..].find("<li class=\"b_algo\"") else { break };
        let block_start = from + rel;
        // 块尾：下一个结果块或列表结束。用下一个 b_algo 作为上界更稳
        let block_end = html[block_start + 18..]
            .find("<li class=\"b_algo\"")
            .map(|p| block_start + 18 + p)
            .unwrap_or(html.len());
        let block = &html[block_start..block_end];

        // h2 内首个 <a href> 即结果链接
        if let Some(h2pos) = block.find("<h2") {
            let seg = &block[h2pos..];
            if let Some(href) = extract_attr(seg, "href") {
                // 只接受 http(s) 绝对链接：Bing 有时给 /ck/a?... 跳转串，需还原
                let real = normalize_bing_url(&href);
                if !real.is_empty() {
                    let title = seg
                        .find('>')
                        .and_then(|p| seg[p + 1..].find("</a>").map(|e| &seg[p + 1..p + 1 + e]))
                        .map(strip_tags)
                        .unwrap_or_default();
                    let snippet = extract_bing_snippet(block);
                    if !title.is_empty() {
                        out.push((title, real, snippet));
                    }
                }
            }
        }
        from = block_end;
        if block_end >= html.len() {
            break;
        }
    }
    out
}

/// 提取 Bing 摘要：优先 `b_lineclamp*`，其次 `b_caption` 内的 `<p>`
fn extract_bing_snippet(block: &str) -> String {
    for marker in ["b_lineclamp", "b_caption"] {
        if let Some(p) = block.find(marker) {
            let seg = &block[p..];
            if let Some(lt) = seg.find("<p") {
                let s = &seg[lt..];
                if let Some(gt) = s.find('>') {
                    if let Some(end) = s[gt..].find("</p>") {
                        let text = strip_tags(&s[gt + 1..gt + end]);
                        if !text.trim().is_empty() {
                            return text;
                        }
                    }
                }
            }
        }
    }
    String::new()
}

/// Bing 跳转链接还原：`/ck/a?...&u=a1aHR0cHM6...` 的 `u` 参数是 base64url 编码的真实 URL
fn normalize_bing_url(href: &str) -> String {
    let h = href.trim();
    if h.starts_with("http://") || h.starts_with("https://") {
        return h.to_string();
    }
    // 相对跳转串：形如 /ck/a?tid=1&u=a1<base64url>&rut=…
    // `a1` 是 Bing 的格式标记（其后才是 base64url 本体），必须连标记一起跳过，
    // 故从 `p + 4`（"u=a1" 共 4 字节）开始取编码串
    if let Some(p) = h.find("u=a1") {
        let raw = &h[p + 4..];
        let end = raw.find('&').unwrap_or(raw.len());
        let encoded = &raw[..end];
        if let Some(decoded) = base64url_decode(encoded) {
            if decoded.starts_with("http://") || decoded.starts_with("https://") {
                return decoded;
            }
        }
    }
    String::new()
}

/// 极简 base64url 解码（Bing 的 `a1` 前缀后即为 base64url，不含 padding）
fn base64url_decode(s: &str) -> Option<String> {
    let val = |c: u8| -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    };
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits = 0u32;
    for &c in bytes {
        let v = val(c)?;
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    String::from_utf8(out).ok()
}

/// 从 DDG HTML 提取 (title, url, snippet)：仅做字符串定位，不引 HTML 解析器
fn parse_ddg_results(html: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(pos) = html[from..].find("result__a") {
        let anchor_start = html[from + pos..].find("<a").map(|p| from + pos + p).unwrap_or(usize::MAX);
        if anchor_start == usize::MAX {
            break;
        }
        let seg = &html[anchor_start..];
        let Some(anchor_end) = seg.find("</a>") else { break };
        let anchor = &seg[..anchor_end];
        let title = strip_tags(anchor);
        // href 提取（uddg 参数内含真实 url，形如 //duckduckgo.com/l/?uddg=<enc>)
        let link = extract_attr(anchor, "href").unwrap_or_default();
        let real_url = decode_uddg(&link);
        // 紧随其后的 snippet
        let after = &html[anchor_start + anchor_end..];
        let snippet = after
            .find("result__snippet")
            .and_then(|p| {
                let s = &after[p..];
                let a = s.find("<a").map(|x| &s[x..])?;
                let e = a.find("</a>")?;
                Some(strip_tags(&a[..e]))
            })
            .unwrap_or_default();
        if !title.is_empty() {
            out.push((title, real_url, snippet));
        }
        from = anchor_start + anchor_end + 4;
        if out.len() >= SEARCH_RESULTS_LIMIT + 3 {
            break;
        }
    }
    out
}

/// uddg 参数解码：//duckduckgo.com/l/?uddg=<percent-encoded>&rut=...
fn decode_uddg(link: &str) -> String {
    let Some(p) = link.find("uddg=") else {
        return if link.starts_with("//") { format!("https:{link}") } else { link.to_string() };
    };
    let raw = &link[p + 5..];
    let end = raw.find('&').unwrap_or(raw.len());
    let enc = &raw[..end];
    percent_decode(enc)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let (Some(h), Some(l)) = (hex_val(bytes.get(i + 1).copied()), hex_val(bytes.get(i + 2).copied())) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex_val(b: Option<u8>) -> Option<u8> {
    match b? {
        c @ b'0'..=b'9' => Some(c - b'0'),
        c @ b'a'..=b'f' => Some(c - b'a' + 10),
        c @ b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn extract_attr(seg: &str, attr: &str) -> Option<String> {
    let key = format!("{attr}=\"");
    let p = seg.find(&key)?;
    let rest = &seg[p + key.len()..];
    let e = rest.find('"')?;
    Some(rest[..e].to_string())
}

/// 剥离 HTML 标签 + 实体最小解码（标题/摘要清洗）
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut depth = 0usize;
    for ch in html.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

/// 页面抓取 → 可读文本（SSRF host 校验 + script/style 剔除 + 剥标签 + 截断）
fn open_url(url: &str) -> Result<String, String> {
    // 审查修复（SSRF 补强）：此前仅校验首跳 host，ureq 默认跟随 5 次重定向——
    // 302 跳 127.0.0.1 等内网地址可绕过白名单。改为 redirects(0) + 逐跳校验
    //（最多 3 跳）：每一跳都重新过 url_allowed，Location 指向受限地址即中止。
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(20))
        .redirects(0)
        .build();
    let mut current = url.trim().to_string();
    for _hop in 0..3 {
        url_allowed(&current)?;
        let _ = match agent
            .get(&current)
            .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .call()
        {
            Ok(resp) => return finish_open_url(resp),
            Err(ureq::Error::Status(code, resp)) if (300..400).contains(&code) => {
                let loc = resp
                    .header("location")
                    .ok_or_else(|| format!("重定向响应缺少 Location（HTTP {code}）"))?
                    .to_string();
                current = resolve_redirect_target(&current, &loc)?;
            }
            Err(e) => return Err(format!("页面请求失败: {e}")),
        };
    }
    Err("重定向次数超过上限（3 跳），已中止".into())
}

/// 重定向目标解析：绝对 URL 直接采用；相对路径以当前 URL 的 scheme+host 为基拼接；
/// 其余形态拒绝（fail-closed，下一跳仍有 url_allowed 白名单兜底）
fn resolve_redirect_target(current: &str, location: &str) -> Result<String, String> {
    let loc = location.trim();
    if loc.starts_with("http://") || loc.starts_with("https://") {
        return Ok(loc.to_string());
    }
    // 协议相对（//host/x）：沿用当前 scheme 换 authority（复审补充）
    if let Some(rest_loc) = loc.strip_prefix("//") {
        let scheme = &current[..current.find("://").unwrap_or(0)];
        return Ok(format!("{scheme}://{rest_loc}"));
    }
    if let Some(rest) = current.strip_prefix("http://").or_else(|| current.strip_prefix("https://")) {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        if !authority.is_empty() {
            if loc.starts_with('/') {
                let scheme = &current[..current.find("://").unwrap_or(0)];
                return Ok(format!("{scheme}://{authority}{loc}"));
            }
        }
    }
    Err(format!("不支持的重定向 Location 形态: {}", &loc.chars().take(80).collect::<String>()))
}

fn finish_open_url(resp: ureq::Response) -> Result<String, String> {
    let html = read_body_limited(resp)?;
    // script/style 块整体剔除（审查修复：删除死变量 cleaned——整页 to_lowercase 白耗分配）
    let body = remove_blocks(&html, "<script", "</script>");
    let body = remove_blocks(&body, "<style", "</style>");
    let mut text = strip_tags(&body);
    text = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        return Err("页面无可读文本".into());
    }
    if text.chars().count() > PAGE_TEXT_LIMIT {
        text = text.chars().take(PAGE_TEXT_LIMIT).collect();
    }
    Ok(text)
}

/// 移除 <tag ...>...</tag> 块（大小写不敏感，块级剔除）
fn remove_blocks(html: &str, open: &str, close: &str) -> String {
    let lower = html.to_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut from = 0;
    loop {
        match lower[from..].find(open) {
            Some(p) => {
                let start = from + p;
                out.push_str(&html[from..start]);
                match lower[start..].find(close) {
                    Some(e) => from = start + e + close.len(),
                    None => break, // 未闭合：丢弃剩余
                }
            }
            None => {
                out.push_str(&html[from..]);
                break;
            }
        }
    }
    out
}

/// 代执行历史 → Responses web_search_call 输出项
pub fn search_call_items(records: &[SearchRecord], resp_id: &str) -> Vec<Value> {
    records
        .iter()
        .filter(|r| r.tool == TOOL_SEARCH)
        .enumerate()
        .map(|(i, r)| {
            json!({
                "id": format!("ws_{}_{}", resp_id, i),
                "type": "web_search_call",
                "status": "completed",
                "action": { "type": "search", "query": r.query },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bing 结果解析（2026-10-21 按真实 HTML 骨架裁剪）。
    ///
    /// 特意保留两个真实坑：
    /// 1. `b_algo` 后紧跟一串 `<link rel=stylesheet>`，标题在它们之后；
    /// 2. 摘要在 `b_lineclamp2` 包裹的 `<p>` 里，不是紧跟标题。
    #[test]
    fn parses_bing_results() {
        let html = r#"
<li class="b_algo" data-id iid=SERP.1>
  <link rel="stylesheet" href="/rp/a.css"/>
  <link rel="stylesheet" href="/rp/b.css"/>
  <h2><a href="https://www.tianqi.com/beijing/today/" h="ID=SERP,1">【北京今天天气预报】北京天气</a></h2>
  <div class="b_caption"><p class="b_lineclamp2">北京今天晴，最高 36℃。</p></div>
</li>
<li class="b_algo" data-id iid=SERP.2>
  <h2><a href="https://www.weather.com.cn/weather/101010100.shtml">北京天气预报</a></h2>
  <div class="b_caption"><p class="b_lineclamp3">7 天预报。</p></div>
</li>
"#;
        let r = parse_bing_results(html);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "【北京今天天气预报】北京天气");
        assert_eq!(r[0].1, "https://www.tianqi.com/beijing/today/");
        assert_eq!(r[0].2, "北京今天晴，最高 36℃。");
        assert_eq!(r[1].1, "https://www.weather.com.cn/weather/101010100.shtml");
        assert_eq!(r[1].2, "7 天预报。");
    }

    /// 验证页（无结果块）必须返回空而不是 panic 或脏数据
    #[test]
    fn bing_verify_page_yields_nothing() {
        let html = "<html><body><form id=\"captcha\">请验证你是人类</form></body></html>";
        assert!(parse_bing_results(html).is_empty());
        assert!(parse_bing_results("").is_empty());
    }

    /// Bing 跳转链接还原：/ck/a?...&u=a1<base64url> 应解出真实 URL
    #[test]
    fn decodes_bing_redirect_url() {
        // "a1" 之后为 base64url("https://example.com/x")
        let real = "https://example.com/x";
        let encoded = base64url_encode_for_test(real.as_bytes());
        let href = format!("/ck/a?tid=1&u=a1{encoded}&rut=abc");
        assert_eq!(normalize_bing_url(&href), real);
        // 绝对链接原样返回
        assert_eq!(normalize_bing_url("https://a.b/c"), "https://a.b/c");
        // 无法还原的返回空串（宁可丢弃这条，也不给模型一个假链接）
        assert_eq!(normalize_bing_url("/ck/a?tid=1"), "");
    }

    /// 真实联网：验证 Bing 主源在国内网络下确实可达并能解析出结果。
    ///
    /// 这是本次换源的核心证据——旧源 DDG 在此环境超时，必须实测才能确认修复有效。
    /// 跑法：cargo test -- --ignored live_search
    #[test]
    #[ignore = "需要外网；手动执行验证真实可达性"]
    fn live_search_returns_results() {
        let out = web_search("北京今天天气").expect("搜索应成功（Bing 或 DDG 任一可达）");
        println!("[live] 搜索结果前 300 字：\n{}", out.chars().take(300).collect::<String>());
        assert!(out.contains("1."), "应有编号结果：{}", out);
        assert!(out.len() > 40, "结果内容过短");
    }

    fn base64url_encode_for_test(data: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            let chars = (chunk.len() * 8 + 5) / 6;
            for i in 0..chars {
                out.push(T[((n >> (18 - i * 6)) & 0x3F) as usize] as char);
            }
        }
        out
    }

    #[test]
    fn detects_web_search_tools() {
        let body = json!({"tools":[{"type":"web_search"},{"type":"function","name":"shell"}]});
        assert!(responses_declares_web_search(&body));
        let body2 = json!({"tools":[{"type":"web_search_preview"}]});
        assert!(responses_declares_web_search(&body2));
        let none = json!({"tools":[{"type":"function","name":"shell"}]});
        assert!(!responses_declares_web_search(&none));
        assert!(!responses_declares_web_search(&json!({})));
    }

    #[test]
    fn injects_tools_and_system_hint() {
        let mut body = json!({
            "messages": [
                {"role": "system", "content": "inst"},
                {"role": "user", "content": "hi"}
            ]
        });
        inject_proxy_tools(&mut body);
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["function"]["name"], json!("web_search"));
        assert_eq!(tools[1]["function"]["name"], json!("open_url"));
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["role"], json!("system"), "提示插在 instructions 之后、首条用户消息之前");
        // 重复注入：同名冲突 → 返回 false 且不再改动（工具数不变）
        assert!(!inject_proxy_tools(&mut body));
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
    }

    /// P1-6 劫持防护：客户端已声明同名 function 工具 → 不注入、返回 false，
    /// 客户端自有 web_search/open_url 语义不被代理按名劫持
    #[test]
    fn inject_rejects_client_owned_same_name_tools() {
        let mut body = json!({
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [
                {"type": "function", "function": {"name": "web_search", "parameters": {"type": "object", "properties": {}}}},
                {"type": "function", "function": {"name": "shell", "parameters": {"type": "object", "properties": {}}}}
            ]
        });
        assert!(!inject_proxy_tools(&mut body));
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2, "不得注入代理工具（保持客户端原声明）");
        assert_eq!(tools[0]["function"]["name"], json!("web_search"));
        assert_eq!(tools[1]["function"]["name"], json!("shell"));
        // open_url 单独冲突同样拒绝（部分注入会留下按名劫持的半代理语义）
        let mut body2 = json!({
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "open_url", "parameters": {"type": "object", "properties": {}}}}]
        });
        assert!(!inject_proxy_tools(&mut body2));
        assert_eq!(body2["tools"].as_array().unwrap().len(), 1);
        // 无冲突时正常注入且返回 true
        let mut body3 = json!({
            "messages": [{"role": "user", "content": "hi"}],
            "tools": [{"type": "function", "function": {"name": "shell", "parameters": {"type": "object", "properties": {}}}}]
        });
        assert!(inject_proxy_tools(&mut body3));
        assert_eq!(body3["tools"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn extracts_only_proxy_tool_calls() {
        let c = json!({
            "choices": [{"message": {"tool_calls": [
                {"id": "c1", "function": {"name": "web_search", "arguments": "{\"query\":\"rust\"}"}},
                {"id": "c2", "function": {"name": "shell", "arguments": "{}"}},
                {"id": "c3", "function": {"name": "open_url", "arguments": "{\"url\":\"https://a\"}"}}
            ]}}]
        });
        let calls = extract_proxy_calls(&c);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "c1");
        assert_eq!(calls[0].1, "web_search");
        assert_eq!(calls[1].1, "open_url");
    }

    #[test]
    fn search_call_items_projection() {
        let records = vec![
            SearchRecord { tool: TOOL_SEARCH.into(), query: "rust 1.0".into(), ok: true },
            SearchRecord { tool: TOOL_OPEN_URL.into(), query: "https://a".into(), ok: true },
            SearchRecord { tool: TOOL_SEARCH.into(), query: "axum".into(), ok: false },
        ];
        let items = search_call_items(&records, "resp_1");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["type"], json!("web_search_call"));
        assert_eq!(items[0]["action"]["query"], json!("rust 1.0"));
        assert_eq!(items[1]["action"]["query"], json!("axum"));
    }

    #[test]
    fn url_encode_and_percent_decode_roundtrip() {
        let enc = url_encode("a b/c?d=中文");
        assert!(!enc.contains(' '));
        assert_eq!(percent_decode(&enc), "a b/c?d=中文");
        assert_eq!(percent_decode("a+b"), "a b");
    }

    #[test]
    fn strip_tags_and_blocks() {
        assert_eq!(strip_tags("<a href=\"x\" class=\"y\">标题</a>"), "标题");
        assert_eq!(strip_tags("a &amp; b"), "a & b");
        let html = "<html><script>bad()</script><style>.x{}</style><body>正文</body></html>";
        let b = remove_blocks(html, "<script", "</script>");
        let b = remove_blocks(&b, "<style", "</style>");
        assert_eq!(strip_tags(&b), "正文");
    }

    #[test]
    fn unknown_tool_errors_cleanly() {
        let (out, ok) = execute("shell", "{}");
        assert!(!ok);
        assert!(out.contains("unknown proxy tool"));
    }

    /// SSRF 防护：host 校验纯函数——私网/环回/链路本地/localhost/非 http(s) 全拒
    #[test]
    fn url_allowed_blocks_private_and_loopback_hosts() {
        assert!(url_allowed("https://example.com/page?q=1").is_ok());
        assert!(url_allowed("https://EXAMPLE.com:8443/x").is_ok());
        assert!(url_allowed("https://user@example.com/").is_ok());
        // 非 http/https
        assert!(url_allowed("ftp://example.com").is_err());
        assert!(url_allowed("file:///etc/passwd").is_err());
        assert!(url_allowed("javascript:alert(1)").is_err());
        // IPv4 私网/环回/链路本地/未指定
        for host in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.9",
            "172.16.0.1",
            "172.31.255.1",
            "169.254.169.254",
            "0.0.0.0",
        ] {
            assert!(url_allowed(&format!("http://{host}/")).is_err(), "应拒绝 {host}");
        }
        // 172.32 属公网 → 放行
        assert!(url_allowed("http://172.32.0.1/").is_ok());
        // localhost（含带端口/子域）
        assert!(url_allowed("http://localhost/").is_err());
        assert!(url_allowed("http://localhost:8080/x").is_err());
        assert!(url_allowed("http://api.localhost/").is_err());
        // IPv6 环回/未指定/唯一本地/链路本地/IPv4 映射环回
        for host in ["[::1]", "[::]", "[fc00::1]", "[fd12::a]", "[fe80::1]", "[::ffff:127.0.0.1]"] {
            assert!(url_allowed(&format!("http://{host}/")).is_err(), "应拒绝 {host}");
        }
        // 纯数字整型 IP 字面量（2130706433 = 127.0.0.1）与空 host
        assert!(url_allowed("http://2130706433/").is_err());
        assert!(url_allowed("http://").is_err());
        // 审查 P2：WHATWG IPv4 变体（十六进制/八进制/缺段）——旧前缀匹配可被绕过
        for host in ["0x7f000001", "0177.0.0.1", "127.1", "0x7f.1", "0x7f.0.0.1"] {
            assert!(url_allowed(&format!("http://{host}/")).is_err(), "应拒绝 {host}");
        }
        // 审查 P2：非 IP 域名不因 IP 样式前缀被误伤
        for host in ["10.orbit.com", "127.example.com", "192.168.example.org"] {
            assert!(url_allowed(&format!("http://{host}/")).is_ok(), "应放行 {host}");
        }
        // 审查 F1：解析差异绕过——WHATWG 把 '\' 当路径分隔，真实 host 是 127.0.0.1/
        // 169.254.169.254，字符串 rsplit('@') 却会误读成 example.com（旧实现的洞）
        for url in [
            "http://127.0.0.1\\@example.com/",
            "http://localhost\\@example.com/",
            "http://169.254.169.254\\.example.com/",
            // 尾点变体：host == "localhost." 未命中旧实现的 == 判定
            "http://localhost./",
            "http://127.0.0.1./",
            "http://[::ffff:127.0.0.1]/",
            "http://0x7f.1/",
            "http://2130706433/",
            "http://100.64.0.1/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
        ] {
            assert!(url_allowed(url).is_err(), "应拒绝 {url}");
        }
        // 尾点公网域名应正常放行（归一只影响受限名，不误伤正常站点）
        assert!(url_allowed("http://example.com./").is_ok());
        assert!(url_allowed("http://api.example.com:8443/x").is_ok());
        // userinfo 语义保留：普通 user@host 剥离凭据后按 host 判定
        assert!(url_allowed("https://user@example.com/").is_ok());
        assert!(url_allowed("https://user:pass@example.com/").is_ok());
    }

    /// open_url 抓取受限地址在发起请求前即被拒绝（不触网）
    #[test]
    fn open_url_rejects_restricted_url_before_fetch() {
        let (out, ok) = execute(TOOL_OPEN_URL, r#"{"url":"http://127.0.0.1:9000/admin"}"#);
        assert!(!ok);
        assert!(out.contains("受限"));
        let (out, ok) = execute(TOOL_OPEN_URL, r#"{"url":"http://169.254.169.254/latest/meta-data"}"#);
        assert!(!ok);
        assert!(out.contains("受限"));
    }
}
