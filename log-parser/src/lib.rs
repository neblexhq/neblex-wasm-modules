//! Neblex sandboxed module: parse log lines into records: Apache/nginx access logs, syslog
//! (RFC 3164 and RFC 5424), key=value (logfmt) lines and JSON lines, detected line by line.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the formats and fields.

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

/// Parses the text in `_options.field` (default `text`, one entry per line) and adds `records`,
/// `record_count`, `formats` (lines per format) and `errors` (lines that matched no format).
fn parse_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("text").to_owned();
    let year = opt.get("year").and_then(Value::as_u64).map(|y| y as u32);
    let only = opt.get("format").and_then(Value::as_str).map(str::to_owned);
    let Some(text) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    if !opt.get("keep_source").and_then(Value::as_bool).unwrap_or(false) {
        m.remove(&field);
    }
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let mut formats = Map::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line, year, only.as_deref()) {
            Some(mut r) => {
                let f = r["_format"].as_str().unwrap_or("").to_owned();
                let n = formats.get(&f).and_then(Value::as_u64).unwrap_or(0);
                formats.insert(f, json!(n + 1));
                r.as_object_mut().unwrap().insert("_line".into(), json!(i + 1));
                records.push(r);
            }
            None => errors.push(json!({ "line": i + 1, "text": line.chars().take(200).collect::<String>() })),
        }
    }
    m.insert("record_count".into(), json!(records.len()));
    m.insert("records".into(), Value::Array(records));
    m.insert("formats".into(), Value::Object(formats));
    m.insert("errors".into(), Value::Array(errors));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

pub fn parse_line(line: &str, year: Option<u32>, only: Option<&str>) -> Option<Value> {
    let want = |f: &str| only.is_none_or(|o| o == f);
    if want("json") && line.trim_start().starts_with('{') {
        if let Ok(Value::Object(mut o)) = serde_json::from_str::<Value>(line.trim()) {
            o.insert("_format".into(), json!("json"));
            return Some(Value::Object(o));
        }
    }
    if want("access") {
        if let Some(r) = access(line) {
            return Some(r);
        }
    }
    if want("syslog") {
        if let Some(r) = syslog(line, year) {
            return Some(r);
        }
    }
    if want("logfmt") {
        if let Some(r) = logfmt(line) {
            return Some(r);
        }
    }
    None
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

fn month(s: &str) -> Option<u32> {
    MONTHS.iter().position(|m| m.eq_ignore_ascii_case(s)).map(|i| i as u32 + 1)
}

/// Apache/nginx common and combined formats:
/// 203.0.113.9 - jane [09/Nov/2026:10:15:00 -0500] "GET /api/orders?id=7 HTTP/1.1" 200 512 "https://ref/" "curl/8.4"
fn access(line: &str) -> Option<Value> {
    let (ip, rest) = line.split_once(' ')?;
    let (_ident, rest) = rest.split_once(' ')?;
    let (user, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('[')?;
    let (ts, rest) = rest.split_once("] ")?;
    let rest = rest.strip_prefix('"')?;
    let (request, rest) = rest.split_once("\" ")?;
    let mut nums = rest.splitn(3, ' ');
    let status: u16 = nums.next()?.parse().ok()?;
    let size_raw = nums.next().unwrap_or("-");
    let tail = nums.next().unwrap_or("");
    let quoted: Vec<&str> = tail.split('"').skip(1).step_by(2).collect();
    let mut parts = request.splitn(3, ' ');
    let (method, path, proto) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (p, query) = path.split_once('?').map(|(a, b)| (a, Some(b))).unwrap_or((path, None));
    let dash = |s: &str| if s == "-" { Value::Null } else { json!(s) };
    Some(json!({
        "_format": "access",
        "ip": ip,
        "user": dash(user),
        "time": access_time(ts).map(Value::String).unwrap_or(json!(ts)),
        "method": method,
        "path": p,
        "query": query,
        "protocol": dash(proto),
        "status": status,
        "bytes": size_raw.parse::<u64>().ok(),
        "referrer": quoted.first().map(|r| dash(r)).unwrap_or(Value::Null),
        "user_agent": quoted.get(1).map(|u| dash(u)).unwrap_or(Value::Null),
    }))
}

/// "09/Nov/2026:10:15:00 -0500" -> "2026-11-09T10:15:00-05:00".
fn access_time(ts: &str) -> Option<String> {
    let (dt, zone) = ts.split_once(' ').unwrap_or((ts, "+0000"));
    let mut p = dt.splitn(4, [':', '/']);
    let (d, mo, y) = (p.next()?, p.next()?, p.next()?);
    let hms = p.next()?;
    let m = month(mo)?;
    let off = if zone.len() == 5 { format!("{}:{}", &zone[..3], &zone[3..]) } else { "Z".into() };
    Some(format!("{}-{:02}-{:02}T{}{}", y, m, d.parse::<u32>().ok()?, hms, off))
}

/// RFC 5424: <34>1 2026-11-09T10:15:00.003Z host app 1234 ID47 - message
/// RFC 3164: <34>Nov  9 10:15:00 host app[1234]: message (the priority is optional)
fn syslog(line: &str, year: Option<u32>) -> Option<Value> {
    let (pri, rest) = match line.strip_prefix('<') {
        Some(r) => {
            let (n, rest) = r.split_once('>')?;
            (Some(n.parse::<u32>().ok()?), rest)
        }
        None => (None, line),
    };
    let sev_fac = |p: Option<u32>| match p {
        Some(p) => {
            const SEV: [&str; 8] = ["emergency", "alert", "critical", "error", "warning", "notice", "info", "debug"];
            (json!(p / 8), json!(SEV[(p % 8) as usize]))
        }
        None => (Value::Null, Value::Null),
    };
    let (facility, severity) = sev_fac(pri);
    // RFC 5424.
    if let Some(r) = rest.strip_prefix("1 ") {
        let mut f = r.splitn(6, ' ');
        let (ts, host, app, pid, msgid) = (f.next()?, f.next()?, f.next()?, f.next()?, f.next()?);
        let rest = f.next().unwrap_or("");
        // Structured data is "-" or one or more [ ... ] blocks.
        let msg = if let Some(m) = rest.strip_prefix("- ") {
            m
        } else if rest == "-" {
            ""
        } else if rest.starts_with('[') {
            let mut depth = 0;
            let mut end = rest.len();
            for (i, c) in rest.char_indices() {
                match c {
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 && !rest[i + 1..].starts_with('[') {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest[end..].trim_start()
        } else {
            rest
        };
        if !ts.contains('T') && ts != "-" {
            return None;
        }
        let dash = |s: &str| if s == "-" { Value::Null } else { json!(s) };
        return Some(json!({
            "_format": "syslog", "facility": facility, "severity": severity, "time": dash(ts), "host": dash(host),
            "app": dash(app), "pid": dash(pid), "msgid": dash(msgid), "message": msg.trim_start_matches('\u{feff}')
        }));
    }
    // RFC 3164.
    let mo = month(rest.get(..3)?)?;
    let after = rest.get(4..)?.trim_start();
    let (day, after) = after.split_once(' ')?;
    let (hms, after) = after.split_once(' ')?;
    if hms.len() != 8 || hms.as_bytes()[2] != b':' {
        return None;
    }
    let (host, after) = after.split_once(' ')?;
    let (tag, msg) = match after.split_once(": ") {
        Some((t, m)) if !t.contains(' ') => (t, m),
        _ => ("", after),
    };
    let (app, pid) = match tag.split_once('[') {
        Some((a, p)) => (a, Some(p.trim_end_matches(']'))),
        None => (tag, None),
    };
    let d: u32 = day.parse().ok()?;
    let time = match year {
        Some(y) => json!(format!("{y}-{mo:02}-{d:02}T{hms}")),
        None => json!(format!("{} {:>2} {}", MONTHS[mo as usize - 1], d, hms)),
    };
    Some(json!({
        "_format": "syslog", "facility": facility, "severity": severity, "time": time, "host": host,
        "app": if app.is_empty() { Value::Null } else { json!(app) }, "pid": pid, "message": msg
    }))
}

/// key=value pairs (logfmt): level=info msg="order saved" order_id=7 took=12ms. At least two
/// pairs are needed for a line to count as logfmt. Numbers and true/false become typed values.
fn logfmt(line: &str) -> Option<Value> {
    let mut o = Map::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut stray = 0;
    while i < chars.len() {
        while i < chars.len() && chars[i] == ' ' {
            i += 1;
        }
        let ks = i;
        while i < chars.len() && chars[i] != '=' && chars[i] != ' ' {
            i += 1;
        }
        let key: String = chars[ks..i].iter().collect();
        if i >= chars.len() || chars[i] != '=' {
            if !key.is_empty() {
                stray += 1;
            }
            continue;
        }
        i += 1;
        let value: String = if chars.get(i) == Some(&'"') {
            i += 1;
            let mut v = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                }
                v.push(chars[i]);
                i += 1;
            }
            i += 1;
            v
        } else {
            let vs = i;
            while i < chars.len() && chars[i] != ' ' {
                i += 1;
            }
            chars[vs..i].iter().collect()
        };
        if key.is_empty() {
            return None;
        }
        let typed = if value == "true" || value == "false" {
            json!(value == "true")
        } else if let Ok(n) = value.parse::<i64>() {
            json!(n)
        } else if let Some(f) = value.parse::<f64>().ok().filter(|f| f.is_finite() && value.contains('.')) {
            json!(f)
        } else {
            json!(value)
        };
        o.insert(key, typed);
    }
    if o.len() < 2 || stray > o.len() {
        return None;
    }
    o.insert("_format".into(), json!("logfmt"));
    Some(Value::Object(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "203.0.113.9 - jane [09/Nov/2026:10:15:00 -0500] \"GET /api/orders?id=7 HTTP/1.1\" 200 512 \"https://example.com/\" \"curl/8.4.0\"\n\
198.51.100.4 - - [09/Nov/2026:10:15:01 -0500] \"POST /login HTTP/1.1\" 401 -\n\
<34>1 2026-11-09T15:15:02.003Z web01 sshd 4321 AUTH - Failed password for root from 192.0.2.7\n\
<86>Nov  9 10:15:03 web01 cron[999]: (root) CMD (run-parts /etc/cron.hourly)\n\
level=error msg=\"order save failed\" order_id=7 took=12.5 retry=true\n\
{\"level\":\"info\",\"msg\":\"worker started\",\"workers\":4}\n\
\n\
this line is not a log line\n";

    #[test]
    fn mixed_formats() {
        let out = run(serde_json::to_vec(&json!({ "text": LOG, "_options": { "year": 2026 } })).unwrap().as_slice());
        let r = out["records"].as_array().unwrap();
        assert_eq!(out["record_count"], json!(6));
        assert_eq!(out["formats"], json!({ "access": 2, "syslog": 2, "logfmt": 1, "json": 1 }));
        assert_eq!(r[0]["time"], json!("2026-11-09T10:15:00-05:00"));
        assert_eq!(r[0]["path"], json!("/api/orders"));
        assert_eq!(r[0]["query"], json!("id=7"));
        assert_eq!(r[0]["user_agent"], json!("curl/8.4.0"));
        assert_eq!(r[1]["user"], Value::Null);
        assert_eq!(r[1]["bytes"], Value::Null);
        assert_eq!(r[1]["referrer"], Value::Null);
        assert_eq!(r[2]["severity"], json!("critical"));
        assert_eq!(r[2]["facility"], json!(4));
        assert_eq!(r[2]["app"], json!("sshd"));
        assert_eq!(r[2]["message"], json!("Failed password for root from 192.0.2.7"));
        assert_eq!(r[3]["time"], json!("2026-11-09T10:15:03"));
        assert_eq!(r[3]["app"], json!("cron"));
        assert_eq!(r[3]["pid"], json!("999"));
        assert_eq!(r[3]["severity"], json!("info"));
        assert_eq!(r[4]["msg"], json!("order save failed"));
        assert_eq!(r[4]["took"], json!(12.5));
        assert_eq!(r[4]["retry"], json!(true));
        assert_eq!(r[5]["workers"], json!(4));
        assert_eq!(out["errors"], json!([{ "line": 8, "text": "this line is not a log line" }]));
    }

    #[test]
    fn syslog_without_year_or_priority_and_structured_data() {
        let r = parse_line("Nov 19 08:01:02 db1 postgres[12]: checkpoint complete", None, None).unwrap();
        assert_eq!(r["time"], json!("Nov 19 08:01:02"));
        assert_eq!(r["severity"], Value::Null);
        let s = parse_line("<165>1 2026-11-09T22:14:15Z host app - ID1 [exampleSDID@32473 iut=\"3\"][x@1 a=\"b\"] An event", None, None).unwrap();
        assert_eq!(s["message"], json!("An event"));
        assert_eq!(s["pid"], Value::Null);
    }

    #[test]
    fn plain_prose_is_not_logfmt_and_format_filter() {
        assert!(parse_line("a=1 is the answer to everything here", None, None).is_none());
        assert!(parse_line("level=info msg=ok", None, Some("access")).is_none());
        assert_eq!(run(br#"{"other": 1}"#)["ok"], json!(false));
    }
}
