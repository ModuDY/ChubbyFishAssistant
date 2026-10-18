export function redact(value, aliases = new Map()) {
  let result = String(value);
  for (const [identity, alias] of [...aliases].sort((left, right) => right[0].length - left[0].length)) {
    result = result.split(identity).join(alias);
  }
  return result.replace(/Bearer\s+\S+/gi, 'Bearer [redacted]')
    .replace(/(["']?(?:access[_-]?token|refresh[_-]?token|password|authorization|cookie)["']?\s*[:=]\s*)(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,}]+)/gi, '$1[redacted]')
    .replace(/[A-Za-z0-9_+./=-]{35,}/g, '[redacted]')
    .replace(/\bwb-[0-9a-f]{8,}\b/gi, '[other-account]')
    .replace(/\b[0-9a-f]{8}-[0-9a-f-]{27,}\b/gi, '[uid]')
    .replace(/\b\d{10,}\b/g, '[numeric-id]')
    .replace(/[\w.+-]+@[\w.-]+\.[a-z]{2,}/gi, '[email]');
}

export function splitLogEntries(line) {
  return line.replace(/^\uFEFF/, '').split(/(?=\[\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\])/)
    .map(entry => {
      const match = entry.match(/^\[?(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\]?\s*(.*)$/);
      return match ? { time: match[1], message: match[2] } : null;
    }).filter(Boolean);
}

export function proxyHeader(line) {
  const match = line.match(/^\[(\d{4}-\d{2}-\d{2} [\d:]+)\] [A-Z]+ ([^\s?]+)/);
  if (!match) return null;
  const endpoint = match[2];
  const host = endpoint.split('/')[0];
  return { time: match[1], endpoint, host,
    buddy: /^(?:[^.]+\.)*(?:workbuddy\.ai|codebuddy\.(?:cn|ai|com)|copilot\.tencent\.com)(?::\d+)?$/i.test(host) };
}
