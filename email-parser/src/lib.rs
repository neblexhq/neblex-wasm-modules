//! Neblex sandboxed module: parse a raw email (RFC 5322 / MIME) into structured fields.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the output fields.

use serde_json::{json, Map, Value};

#[no_mangle]
pub extern "C" fn alloc(len: i32) -> i32 {
    let mut buf: Vec<u8> = Vec::with_capacity(len.max(0) as usize);
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf);
    ptr as i32
}

#[no_mangle]
pub extern "C" fn invoke(ptr: i32, len: i32) -> i64 {
    let input = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let out = run(input);
    let out = serde_json::to_vec(&out).unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"serialize\"}".to_vec()).leak();
    ((out.as_ptr() as i64) << 32) | (out.len() as i64)
}

pub fn run(input: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(input) {
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(parse_record).collect()),
        Ok(v) => parse_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// Reads the raw message from `_options.field` (default `email`; set `"base64": true` when it
/// is base64-encoded) and adds the parsed fields. `_options` is removed.
fn parse_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let flag = |k: &str| opt.get(k).and_then(Value::as_bool).unwrap_or(false);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("email").to_owned();
    let Some(text) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    let raw: Vec<u8> = if flag("base64") {
        match b64(text.as_bytes()) {
            Some(b) => b,
            None => {
                m.insert("ok".into(), json!(false));
                m.insert("error".into(), json!("field is not valid base64"));
                return Value::Object(m);
            }
        }
    } else {
        text.into_bytes()
    };
    if !flag("keep_source") {
        m.remove(&field);
    }
    let parsed = parse(&raw, flag("include_html"), flag("include_attachments"));
    m.extend(parsed.as_object().unwrap().clone());
    Value::Object(m)
}

pub fn parse(raw: &[u8], include_html: bool, include_attachments: bool) -> Value {
    let part = Part::parse(raw);
    if part.headers.is_empty() {
        return json!({ "ok": false, "error": "no email headers found" });
    }
    let mut acc = Acc::default();
    walk(&part, &mut acc, include_attachments);
    let text = acc.text.clone().or_else(|| acc.html.as_deref().map(html_to_text)).unwrap_or_default().replace("\r\n", "\n");
    let (reply, quoted, signature) = split_reply(&text);
    let h = |k: &str| part.header(k).map(|v| decode_words(&v));
    let mut out = Map::new();
    out.insert("ok".into(), json!(true));
    out.insert("from".into(), addresses(&h("from").unwrap_or_default()).into_iter().next().unwrap_or(Value::Null));
    out.insert("to".into(), json!(addresses(&h("to").unwrap_or_default())));
    out.insert("cc".into(), json!(addresses(&h("cc").unwrap_or_default())));
    out.insert("reply_to".into(), addresses(&h("reply-to").unwrap_or_default()).into_iter().next().unwrap_or(Value::Null));
    out.insert("subject".into(), json!(h("subject").unwrap_or_default().trim()));
    let date = h("date");
    out.insert("date".into(), json!(date.as_deref().and_then(rfc2822)));
    out.insert("date_raw".into(), json!(date));
    out.insert("message_id".into(), json!(h("message-id").map(|v| v.trim().trim_matches(['<', '>']).to_owned())));
    out.insert("in_reply_to".into(), json!(h("in-reply-to").map(|v| v.trim().trim_matches(['<', '>']).to_owned())));
    let refs: Vec<String> = h("references").unwrap_or_default().split_whitespace().map(|r| r.trim_matches(['<', '>']).to_owned()).collect();
    out.insert("references".into(), json!(refs));
    out.insert("body_text".into(), json!(text.trim()));
    out.insert("reply_text".into(), json!(reply));
    out.insert("quoted_text".into(), json!(quoted));
    out.insert("signature".into(), json!(signature));
    if include_html {
        out.insert("body_html".into(), json!(acc.html));
    }
    out.insert("attachments".into(), Value::Array(acc.attachments));
    Value::Object(out)
}

struct Part {
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Part {
    fn parse(raw: &[u8]) -> Part {
        // Headers end at the first empty line (CRLF or LF).
        let mut i = 0;
        let mut lines: Vec<String> = Vec::new();
        let mut body_start = raw.len();
        while i < raw.len() {
            let end = raw[i..].iter().position(|&b| b == b'\n').map(|p| i + p).unwrap_or(raw.len());
            let line = String::from_utf8_lossy(&raw[i..end]).trim_end_matches('\r').to_owned();
            if line.is_empty() {
                body_start = (end + 1).min(raw.len());
                break;
            }
            if line.starts_with([' ', '\t']) && !lines.is_empty() {
                let last = lines.last_mut().unwrap();
                last.push(' ');
                last.push_str(line.trim());
            } else {
                lines.push(line);
            }
            i = end + 1;
        }
        let headers: Vec<(String, String)> = lines
            .iter()
            .filter_map(|l| {
                let (k, v) = l.split_once(':')?;
                (!k.is_empty() && !k.contains(' ')).then(|| (k.trim().to_lowercase(), v.trim().to_owned()))
            })
            .collect();
        Part { headers, body: raw[body_start.min(raw.len())..].to_vec() }
    }

    fn header(&self, k: &str) -> Option<String> {
        self.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    /// Content type and its parameters, lowercased type; defaults to text/plain.
    fn content_type(&self) -> (String, Vec<(String, String)>) {
        let v = self.header("content-type").unwrap_or_else(|| "text/plain".into());
        let (t, params) = params(&v);
        (t.to_lowercase(), params)
    }

    fn decoded(&self) -> Vec<u8> {
        match self.header("content-transfer-encoding").map(|v| v.trim().to_lowercase()).as_deref() {
            Some("base64") => b64(&self.body).unwrap_or_default(),
            Some("quoted-printable") => qp(&self.body, false),
            _ => self.body.clone(),
        }
    }
}

fn params(v: &str) -> (String, Vec<(String, String)>) {
    let mut parts = split_unquoted(v, ';').into_iter();
    let first = parts.next().unwrap_or_default().trim().to_owned();
    let ps = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim().to_lowercase().trim_end_matches('*').to_owned(), v.trim().trim_matches('"').to_owned()))
        })
        .collect();
    (first, ps)
}

fn split_unquoted(s: &str, sep: char) -> Vec<String> {
    let mut out = vec![String::new()];
    let (mut q, mut angle) = (false, false);
    for c in s.chars() {
        match c {
            '"' => q = !q,
            '<' if !q => angle = true,
            '>' if !q => angle = false,
            _ => {}
        }
        if c == sep && !q && !angle {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

#[derive(Default)]
struct Acc {
    text: Option<String>,
    html: Option<String>,
    attachments: Vec<Value>,
}

fn walk(p: &Part, acc: &mut Acc, include_attachments: bool) {
    let (ctype, ps) = p.content_type();
    let param = |k: &str| ps.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let disp = p.header("content-disposition").map(|d| params(&d));
    let filename = disp.as_ref().and_then(|(_, dp)| dp.iter().find(|(n, _)| n == "filename").map(|(_, v)| v.clone())).or_else(|| param("name")).map(|f| decode_words(&ext_value(&f)));
    let is_attachment = disp.as_ref().is_some_and(|(d, _)| d.eq_ignore_ascii_case("attachment")) || filename.is_some();
    if ctype.starts_with("multipart/") {
        let Some(boundary) = param("boundary") else { return };
        for sub in split_multipart(&p.body, &boundary) {
            walk(&Part::parse(&sub), acc, include_attachments);
        }
        return;
    }
    if ctype == "message/rfc822" && !is_attachment {
        walk(&Part::parse(&p.decoded()), acc, include_attachments);
        return;
    }
    let data = p.decoded();
    if is_attachment {
        let mut a = json!({ "filename": filename, "content_type": ctype, "size": data.len() });
        if let Some(id) = p.header("content-id") {
            a["content_id"] = json!(id.trim_matches(['<', '>']));
        }
        if include_attachments {
            a["content_base64"] = json!(b64_encode(&data));
        }
        acc.attachments.push(a);
        return;
    }
    let text = charset(&data, &param("charset").unwrap_or_default());
    if ctype == "text/plain" && acc.text.is_none() {
        acc.text = Some(text);
    } else if ctype == "text/html" && acc.html.is_none() {
        acc.html = Some(text);
    }
}

fn split_multipart(body: &[u8], boundary: &str) -> Vec<Vec<u8>> {
    let delim = format!("--{boundary}");
    let s = String::from_utf8_lossy(body);
    // Work on lines so CRLF and LF both work; lossy is fine because parts re-decode bytes below.
    let mut parts = Vec::new();
    let mut cur: Option<Vec<u8>> = None;
    let mut pos = 0;
    for line in s.split_inclusive('\n') {
        let start = pos;
        pos += line.len();
        let t = line.trim_end();
        if t == delim || t == format!("{delim}--") {
            if let Some(c) = cur.take() {
                parts.push(c);
            }
            if t.ends_with("--") && t != delim {
                break;
            }
            cur = Some(Vec::new());
            continue;
        }
        if let Some(c) = cur.as_mut() {
            c.extend_from_slice(&body[start.min(body.len())..pos.min(body.len())]);
        }
    }
    if let Some(c) = cur {
        parts.push(c);
    }
    parts
}

/// RFC 2231 extended values like utf-8''na%C3%AFve.pdf.
fn ext_value(v: &str) -> String {
    let Some((cs, rest)) = v.split_once("''") else { return v.to_owned() };
    let mut bytes = Vec::new();
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                bytes.push(x);
                i += 3;
                continue;
            }
        }
        bytes.push(b[i]);
        i += 1;
    }
    charset(&bytes, cs)
}

/// Decodes RFC 2047 encoded words (=?charset?B?...?= and =?charset?Q?...?=).
fn decode_words(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    let mut last_was_word = false;
    while let Some(st) = rest.find("=?") {
        let (before, after) = rest.split_at(st);
        let decoded = (|| {
            let inner = &after[2..];
            let (cs, inner) = inner.split_once('?')?;
            let (enc, inner) = inner.split_once('?')?;
            let end = inner.find("?=")?;
            let data = &inner[..end];
            let bytes = match enc.to_ascii_uppercase().as_str() {
                "B" => b64(data.as_bytes())?,
                "Q" => qp(data.as_bytes(), true),
                _ => return None,
            };
            Some((charset(&bytes, cs), 2 + cs.len() + 1 + enc.len() + 1 + end + 2))
        })();
        match decoded {
            Some((text, used)) => {
                // Whitespace between two adjacent encoded words is dropped.
                if !(last_was_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&text);
                rest = &after[used..];
                last_was_word = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &after[2..];
                last_was_word = false;
            }
        }
    }
    out.push_str(rest);
    out
}

fn addresses(v: &str) -> Vec<Value> {
    split_unquoted(v, ',')
        .into_iter()
        .filter_map(|a| {
            let a = a.trim();
            if a.is_empty() {
                return None;
            }
            let (name, addr) = match (a.rfind('<'), a.rfind('>')) {
                (Some(l), Some(r)) if l < r => (a[..l].trim().trim_matches('"').trim(), a[l + 1..r].trim()),
                _ => ("", a.trim_matches(['"', ' '])),
            };
            addr.contains('@').then(|| json!({ "name": if name.is_empty() { Value::Null } else { json!(name) }, "address": addr.to_lowercase() }))
        })
        .collect()
}

/// RFC 2822 date ("Mon, 9 Nov 2026 10:15:00 -0500") to ISO 8601 with offset.
fn rfc2822(s: &str) -> Option<String> {
    let s = s.split('(').next()?.trim();
    let s = s.split_once(',').map(|(_, r)| r).unwrap_or(s);
    let t: Vec<&str> = s.split_whitespace().collect();
    if t.len() < 4 {
        return None;
    }
    let d: u32 = t[0].parse().ok()?;
    let mon = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| t[1].to_lowercase().starts_with(m))? + 1;
    let mut y: u32 = t[2].parse().ok()?;
    if y < 100 {
        y += if y < 50 { 2000 } else { 1900 };
    }
    let hms: Vec<&str> = t[3].split(':').collect();
    let (hh, mm, ss) = (hms.first()?.parse::<u32>().ok()?, hms.get(1)?.parse::<u32>().ok()?, hms.get(2).and_then(|x| x.parse::<u32>().ok()).unwrap_or(0));
    let zone = t.get(4).copied().unwrap_or("+0000");
    let off = match zone.to_uppercase().as_str() {
        "GMT" | "UT" | "UTC" | "Z" => "+00:00".to_owned(),
        "EST" => "-05:00".into(),
        "EDT" => "-04:00".into(),
        "CST" => "-06:00".into(),
        "CDT" => "-05:00".into(),
        "MST" => "-07:00".into(),
        "MDT" => "-06:00".into(),
        "PST" => "-08:00".into(),
        "PDT" => "-07:00".into(),
        z if z.len() == 5 && (z.starts_with('+') || z.starts_with('-')) => format!("{}:{}", &z[..3], &z[3..]),
        _ => return None,
    };
    if !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(format!("{y:04}-{mon:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}{off}"))
}

/// Splits the new message from quoted history and the signature. Cuts at the first
/// "On ... wrote:", "-----Original Message-----", an Outlook "From:" header block, or a run of
/// ">" lines; the signature starts at "-- " or a short sign-off block before that point.
fn split_reply(text: &str) -> (String, String, Value) {
    let lines: Vec<&str> = text.lines().collect();
    let mut cut = lines.len();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        let low = t.to_lowercase();
        let next = lines.get(i + 1).map(|x| x.trim().to_lowercase()).unwrap_or_default();
        let wrote = low.starts_with("on ") && (low.ends_with("wrote:") || (next.ends_with("wrote:") && !next.is_empty()));
        let original = low.contains("original message") && low.starts_with("-----");
        let outlook = low.starts_with("from:") && lines[i + 1..].iter().take(4).any(|x| { let x = x.trim().to_lowercase(); x.starts_with("sent:") || x.starts_with("date:") || x.starts_with("to:") || x.starts_with("subject:") }) && i > 0;
        let quoted = t.starts_with('>') && lines[i..].iter().take_while(|x| x.trim().is_empty() || x.trim().starts_with('>')).count() >= 1 && i > 0;
        if wrote || original || outlook || quoted {
            cut = i;
            break;
        }
    }
    let quoted: String = lines[cut..].join("\n").trim().to_owned();
    let mut body: Vec<&str> = lines[..cut].to_vec();
    let mut signature = Value::Null;
    if let Some(sig) = body.iter().rposition(|l| *l == "-- " || l.trim() == "--") {
        signature = json!(body[sig + 1..].join("\n").trim());
        body.truncate(sig);
    } else if let Some(p) = body.iter().rposition(|l| {
        let t = l.trim().to_lowercase();
        t.starts_with("sent from my") || t.starts_with("get outlook for")
    }) {
        signature = json!(body[p..].join("\n").trim());
        body.truncate(p);
    }
    (body.join("\n").trim().to_owned(), quoted, signature)
}

fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut i = 0;
    let b = html.as_bytes();
    // ASCII lowercasing keeps byte offsets identical to `html`.
    let low = html.to_ascii_lowercase();
    while i < b.len() {
        if b[i] == b'<' {
            let end = html[i..].find('>').map(|p| i + p + 1).unwrap_or(b.len());
            let tag = &low[i..end];
            let skipped = ["<style", "<script", "<head"]
                .iter()
                .find(|s| tag.starts_with(**s) && tag[s.len()..].starts_with(['>', ' ', '\t', '\n', '\r', '/']));
            if let Some(skip) = skipped {
                let close = format!("</{}", &skip[1..]);
                let e = low[end..].find(&close).map(|p| end + p).unwrap_or(b.len());
                i = low[e..].find('>').map(|p| e + p + 1).unwrap_or(b.len());
                continue;
            }
            if tag.starts_with("<br") || tag.starts_with("</p") || tag.starts_with("</div") || tag.starts_with("</tr") || tag.starts_with("</h") || tag.starts_with("</li") {
                out.push('\n');
            }
            i = end;
            continue;
        }
        let next = html[i..].find('<').map(|p| i + p).unwrap_or(b.len());
        out.push_str(&entities(&html[i..next]));
        i = next;
    }
    let lines: Vec<String> = out.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    let mut res: Vec<String> = Vec::new();
    for l in lines {
        if l.is_empty() && res.last().is_some_and(|x| x.is_empty()) {
            continue;
        }
        res.push(l);
    }
    res.join("\n").trim().to_owned()
}

fn entities(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('&') {
        out.push_str(&rest[..a]);
        let tail = &rest[a..];
        let Some(semi) = tail[..tail.len().min(10)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..semi];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
            n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

const CP1252: [char; 32] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
];

/// UTF-8 (default), ISO-8859-1 and Windows-1252; anything else is read as UTF-8, replacing invalid bytes.
fn charset(bytes: &[u8], cs: &str) -> String {
    match cs.trim().to_lowercase().as_str() {
        "iso-8859-1" | "latin1" | "iso8859-1" | "us-ascii" | "ascii" if !bytes.iter().any(|b| *b >= 0x80) => String::from_utf8_lossy(bytes).into_owned(),
        "iso-8859-1" | "latin1" | "iso8859-1" => bytes.iter().map(|&b| b as char).collect(),
        "windows-1252" | "cp1252" => bytes.iter().map(|&b| if (0x80..0xa0).contains(&b) { CP1252[(b - 0x80) as usize] } else { b as char }).collect(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn qp(data: &[u8], header: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b'=' if i + 1 < data.len() && (data[i + 1] == b'\n' || (data[i + 1] == b'\r' && data.get(i + 2) == Some(&b'\n'))) => {
                i += if data[i + 1] == b'\r' { 3 } else { 2 };
            }
            b'=' if i + 2 < data.len() => match u8::from_str_radix(std::str::from_utf8(&data[i + 1..i + 3]).unwrap_or("zz"), 16) {
                Ok(b) => {
                    out.push(b);
                    i += 3;
                }
                Err(_) => {
                    out.push(b'=');
                    i += 1;
                }
            },
            b'_' if header => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for &c in data {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                out.push(B64[(n >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI: &str = "From: \"Doe, Jane\" <Jane.Doe@Example.com>\r\n\
To: support@neblex.com, Ops <ops@neblex.com>\r\n\
Cc: =?UTF-8?B?SsO2cmc=?= <jorg@example.de>\r\n\
Subject: =?UTF-8?Q?Re:_Order_45-1180_=E2=80=93_damaged?=\r\n\
Date: Mon, 9 Nov 2026 10:15:00 -0500\r\n\
Message-ID: <abc123@mail.example.com>\r\n\
In-Reply-To: <prev@neblex.com>\r\n\
References: <first@neblex.com>\r\n <prev@neblex.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
--outer\r\n\
Content-Type: multipart/alternative; boundary=\"inner\"\r\n\
\r\n\
--inner\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Hi, the box arrived crushed =E2=80=93 photos attached.\r\n\
Can you send a replacement?\r\n\
\r\n\
--=20\r\n\
Jane Doe | Acme\r\n\
\r\n\
On Fri, Nov 6, 2026 at 9:00 AM Support <support@neblex.com> wrote:\r\n\
> Your order shipped today.\r\n\
--inner\r\n\
Content-Type: text/html; charset=\"utf-8\"\r\n\
\r\n\
<p>Hi, the box arrived crushed</p>\r\n\
--inner--\r\n\
--outer\r\n\
Content-Type: image/jpeg; name=\"box.jpg\"\r\n\
Content-Disposition: attachment; filename=\"box.jpg\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
/9j/4AAQ\r\n\
--outer--\r\n";

    #[test]
    fn multipart_message() {
        let v = parse(MULTI.as_bytes(), false, true);
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["from"], json!({ "name": "Doe, Jane", "address": "jane.doe@example.com" }));
        assert_eq!(v["to"].as_array().unwrap().len(), 2);
        assert_eq!(v["cc"][0], json!({ "name": "Jörg", "address": "jorg@example.de" }));
        assert_eq!(v["subject"], json!("Re: Order 45-1180 – damaged"));
        assert_eq!(v["date"], json!("2026-11-09T10:15:00-05:00"));
        assert_eq!(v["message_id"], json!("abc123@mail.example.com"));
        assert_eq!(v["references"], json!(["first@neblex.com", "prev@neblex.com"]));
        assert_eq!(v["reply_text"], json!("Hi, the box arrived crushed – photos attached.\nCan you send a replacement?"));
        assert_eq!(v["signature"], json!("Jane Doe | Acme"));
        assert!(!v["body_text"].as_str().unwrap().contains('\r'));
        assert!(v["quoted_text"].as_str().unwrap().starts_with("On Fri, Nov 6"));
        assert_eq!(v["attachments"][0]["filename"], json!("box.jpg"));
        assert_eq!(v["attachments"][0]["size"], json!(6));
        assert_eq!(v["attachments"][0]["content_base64"], json!("/9j/4AAQ"));
    }

    #[test]
    fn html_only_and_outlook_reply() {
        let raw = "From: ops@acme.com\nSubject: PO update\nContent-Type: text/html; charset=windows-1252\n\n\
<html><head><style>p{}</style></head><body><p>PO 4411 is approved &amp; ready.</p><div>Thanks<br>Sam</div>\
<p>From: Buyer</p><p>Sent: Friday</p><p>To: Sam</p><p>old text</p></body></html>";
        let v = parse(raw.as_bytes(), false, false);
        assert_eq!(v["from"]["address"], json!("ops@acme.com"));
        assert_eq!(v["reply_text"], json!("PO 4411 is approved & ready.\nThanks\nSam"));
        assert!(v["quoted_text"].as_str().unwrap().starts_with("From: Buyer"));
        assert_eq!(v["date"], Value::Null);
    }

    #[test]
    fn header_tag_is_not_head_and_rfc2231_filenames() {
        assert_eq!(html_to_text("<header>Top</header><p>Body</p>"), "Top\nBody");
        assert_eq!(ext_value("utf-8''na%C3%AFve%20plan.pdf"), "naïve plan.pdf");
    }

    #[test]
    fn record_interface_and_errors() {
        let out = run(br#"{"raw": "Subject: hi\nFrom: a@b.co\n\nbody", "_options": {"field": "raw"}}"#);
        assert_eq!(out["subject"], json!("hi"));
        assert_eq!(out["reply_text"], json!("body"));
        assert!(out.get("raw").is_none());
        assert_eq!(run(br#"{"email": "just some text without headers"}"#)["ok"], json!(false));
        let enc = b64_encode(b"Subject: x\nFrom: a@b.co\n\nhello");
        let out = run(serde_json::to_vec(&json!({ "email": enc, "_options": { "base64": true } })).unwrap().as_slice());
        assert_eq!(out["reply_text"], json!("hello"));
    }
}
