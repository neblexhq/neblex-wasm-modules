//! Neblex sandboxed module: parse iCalendar (.ics) data, such as meeting invites attached to
//! emails or calendar feeds, into event records.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the fields.

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

/// One content line: NAME;PARAM=value;PARAM="quoted":value
pub struct Prop {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl Prop {
    fn param(&self, k: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

/// Lines with folding undone (a line starting with a space or tab continues the previous one).
pub fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if (line.starts_with(' ') || line.starts_with('\t')) && !out.is_empty() {
            out.last_mut().unwrap().push_str(&line[1..]);
        } else if !line.is_empty() {
            out.push(line.to_owned());
        }
    }
    out
}

pub fn parse_prop(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let head = &line[..colon];
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in head.chars() {
        match c {
            '"' => q = !q,
            ';' if !q => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    let name = parts.remove(0).to_uppercase();
    let params = parts
        .into_iter()
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.trim().to_uppercase(), v.trim().to_owned()),
            None => (p.trim().to_uppercase(), String::new()),
        })
        .collect();
    Some(Prop { name, params, value: line[colon + 1..].to_owned() })
}

/// TEXT value escapes: \n \N \, \; \\
pub fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut it = v.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 20261109T151500Z -> 2026-11-09T15:15:00Z, 20261109T101500 -> 2026-11-09T10:15:00, 20261109 -> 2026-11-09.
pub fn ical_time(v: &str) -> Option<String> {
    let v = v.trim();
    let digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    let (d, t) = match v.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (v, None),
    };
    if d.len() != 8 || !digits(d) {
        return None;
    }
    let date = format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]);
    match t {
        None => Some(date),
        Some(t) => {
            let (t, z) = match t.strip_suffix('Z') {
                Some(x) => (x, "Z"),
                None => (t, ""),
            };
            if t.len() < 6 || !digits(&t[..6]) {
                return None;
            }
            Some(format!("{date}T{}:{}:{}{z}", &t[..2], &t[2..4], &t[4..6]))
        }
    }
}

fn person(p: &Prop) -> Value {
    let email = p.value.trim().strip_prefix("mailto:").or_else(|| p.value.trim().strip_prefix("MAILTO:")).unwrap_or(p.value.trim()).to_lowercase();
    let mut o = Map::new();
    o.insert("name".into(), p.param("CN").map(|n| json!(n.trim_matches('"'))).unwrap_or(Value::Null));
    o.insert("email".into(), json!(email));
    if p.name == "ATTENDEE" {
        o.insert("role".into(), json!(p.param("ROLE").unwrap_or("REQ-PARTICIPANT")));
        o.insert("status".into(), json!(p.param("PARTSTAT").unwrap_or("NEEDS-ACTION")));
        o.insert("rsvp".into(), json!(p.param("RSVP").map(|r| r.eq_ignore_ascii_case("TRUE")).unwrap_or(false)));
    }
    Value::Object(o)
}

fn time_fields(o: &mut Map<String, Value>, key: &str, p: &Prop) {
    let t = ical_time(&p.value);
    o.insert(key.into(), t.clone().map(Value::String).unwrap_or(json!(p.value)));
    if key == "start" {
        let all_day = p.param("VALUE") == Some("DATE") || t.as_deref().is_some_and(|x| x.len() == 10);
        o.insert("all_day".into(), json!(all_day));
        let tz = if t.as_deref().is_some_and(|x| x.ends_with('Z')) { Some("UTC".to_owned()) } else { p.param("TZID").map(str::to_owned) };
        o.insert("timezone".into(), tz.map(Value::String).unwrap_or(Value::Null));
    }
}

/// Parses all VEVENT blocks in the calendar text.
pub fn parse(text: &str) -> (Option<String>, Vec<Value>, usize) {
    let mut method = None;
    let mut events = Vec::new();
    let mut todos = 0;
    let mut cur: Option<Map<String, Value>> = None;
    let mut depth_other = 0;
    for line in unfold(text) {
        let Some(p) = parse_prop(&line) else { continue };
        match (p.name.as_str(), p.value.trim().to_uppercase().as_str()) {
            ("BEGIN", "VEVENT") => {
                let mut o = Map::new();
                o.insert("attendees".into(), json!([]));
                cur = Some(o);
                continue;
            }
            ("END", "VEVENT") => {
                if let Some(o) = cur.take() {
                    events.push(Value::Object(o));
                }
                continue;
            }
            ("BEGIN", "VTODO") => todos += 1,
            ("BEGIN", "VALARM") | ("BEGIN", "VTIMEZONE") => depth_other += 1,
            ("END", "VALARM") | ("END", "VTIMEZONE") => depth_other -= 1,
            _ => {}
        }
        if p.name == "METHOD" && cur.is_none() {
            method = Some(p.value.trim().to_uppercase());
        }
        let Some(o) = cur.as_mut() else { continue };
        if depth_other > 0 {
            continue;
        }
        match p.name.as_str() {
            "UID" => {
                o.insert("uid".into(), json!(p.value.trim()));
            }
            "SUMMARY" => {
                o.insert("summary".into(), json!(unescape(&p.value)));
            }
            "DESCRIPTION" => {
                o.insert("description".into(), json!(unescape(&p.value)));
            }
            "LOCATION" => {
                o.insert("location".into(), json!(unescape(&p.value)));
            }
            "STATUS" => {
                o.insert("status".into(), json!(p.value.trim().to_uppercase()));
            }
            "URL" => {
                o.insert("url".into(), json!(p.value.trim()));
            }
            "SEQUENCE" => {
                o.insert("sequence".into(), json!(p.value.trim().parse::<i64>().unwrap_or(0)));
            }
            "RRULE" => {
                o.insert("recurrence".into(), json!(p.value.trim()));
            }
            "DURATION" => {
                o.insert("duration".into(), json!(p.value.trim()));
            }
            "DTSTART" => time_fields(o, "start", &p),
            "DTEND" => time_fields(o, "end", &p),
            "CREATED" => time_fields(o, "created", &p),
            "LAST-MODIFIED" => time_fields(o, "last_modified", &p),
            "ORGANIZER" => {
                o.insert("organizer".into(), person(&p));
            }
            "ATTENDEE" => {
                o.get_mut("attendees").unwrap().as_array_mut().unwrap().push(person(&p));
            }
            _ => {}
        }
    }
    (method, events, todos)
}

/// Reads the calendar text from `_options.field` (default `ics`; base64 accepted with
/// `"base64": true`) and adds `method`, `events` and `event_count`.
fn parse_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("ics").to_owned();
    let Some(raw) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    let text = if opt.get("base64").and_then(Value::as_bool).unwrap_or(false) {
        match b64(raw.as_bytes()) {
            Some(b) => String::from_utf8_lossy(&b).into_owned(),
            None => {
                m.insert("ok".into(), json!(false));
                m.insert("error".into(), json!("field is not valid base64"));
                return Value::Object(m);
            }
        }
    } else {
        raw
    };
    if !opt.get("keep_source").and_then(Value::as_bool).unwrap_or(false) {
        m.remove(&field);
    }
    if !text.to_uppercase().contains("BEGIN:VCALENDAR") {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!("no BEGIN:VCALENDAR found"));
        return Value::Object(m);
    }
    let (method, events, todos) = parse(&text);
    m.insert("method".into(), json!(method));
    m.insert("event_count".into(), json!(events.len()));
    m.insert("events".into(), Value::Array(events));
    if todos > 0 {
        m.insert("todo_count".into(), json!(todos));
    }
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

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

#[cfg(test)]
mod tests {
    use super::*;

    const INVITE: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Example//EN\r\nMETHOD:REQUEST\r\n\
BEGIN:VTIMEZONE\r\nTZID:America/New_York\r\nBEGIN:STANDARD\r\nDTSTART:20261101T020000\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\n\
BEGIN:VEVENT\r\nUID:abc-123@example.com\r\nSEQUENCE:1\r\nDTSTAMP:20261109T120000Z\r\n\
DTSTART;TZID=America/New_York:20261112T100000\r\nDTEND;TZID=America/New_York:20261112T103000\r\n\
SUMMARY:Order 45-1180 review\\, with Globex\r\nDESCRIPTION:Agenda:\\n1. Damaged box\\n2. Replacement\r\n\
LOCATION:https://meet.example.com/abc\r\nSTATUS:CONFIRMED\r\nRRULE:FREQ=WEEKLY;COUNT=3\r\n\
ORGANIZER;CN=\"Doe, Jane\":mailto:Jane.Doe@Example.com\r\n\
ATTENDEE;CN=Sam Lee;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;RSVP=TRUE:mailto:sam@globex.com\r\n\
ATTENDEE;CN=Ops Team;ROLE=OPT-PARTICIPANT;PARTSTAT=NEEDS-ACTION:mailto:ops@\r\n example.com\r\n\
BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nDESCRIPTION:Reminder\r\nEND:VALARM\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:holiday-1\r\nDTSTART;VALUE=DATE:20261126\r\nDTEND;VALUE=DATE:20261127\r\nSUMMARY:Office closed\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn invite() {
        let out = run(serde_json::to_vec(&json!({ "ics": INVITE })).unwrap().as_slice());
        assert_eq!(out["method"], json!("REQUEST"));
        assert_eq!(out["event_count"], json!(2));
        let e = &out["events"][0];
        assert_eq!(e["uid"], json!("abc-123@example.com"));
        assert_eq!(e["summary"], json!("Order 45-1180 review, with Globex"));
        assert_eq!(e["description"], json!("Agenda:\n1. Damaged box\n2. Replacement"));
        assert_eq!(e["start"], json!("2026-11-12T10:00:00"));
        assert_eq!(e["end"], json!("2026-11-12T10:30:00"));
        assert_eq!(e["timezone"], json!("America/New_York"));
        assert_eq!(e["all_day"], json!(false));
        assert_eq!(e["recurrence"], json!("FREQ=WEEKLY;COUNT=3"));
        assert_eq!(e["sequence"], json!(1));
        assert_eq!(e["organizer"], json!({ "name": "Doe, Jane", "email": "jane.doe@example.com" }));
        assert_eq!(e["attendees"][0], json!({ "name": "Sam Lee", "email": "sam@globex.com", "role": "REQ-PARTICIPANT", "status": "ACCEPTED", "rsvp": true }));
        assert_eq!(e["attendees"][1]["email"], json!("ops@example.com"));
        let h = &out["events"][1];
        assert_eq!(h["start"], json!("2026-11-26"));
        assert_eq!(h["all_day"], json!(true));
        assert!(out.get("ics").is_none());
    }

    #[test]
    fn utc_times_base64_and_errors() {
        let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20261109T151500Z\nSUMMARY:Call\nEND:VEVENT\nEND:VCALENDAR\n";
        let enc = {
            const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let d = ics.as_bytes();
            let mut s = String::new();
            for c in d.chunks(3) {
                let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                for k in 0..4 {
                    s.push(if k <= c.len() { T[(n >> (18 - 6 * k) & 63) as usize] as char } else { '=' });
                }
            }
            s
        };
        let out = run(serde_json::to_vec(&json!({ "file": enc, "_options": { "field": "file", "base64": true } })).unwrap().as_slice());
        assert_eq!(out["events"][0]["start"], json!("2026-11-09T15:15:00Z"));
        assert_eq!(out["events"][0]["timezone"], json!("UTC"));
        assert_eq!(out["method"], Value::Null);
        assert_eq!(run(br#"{"ics": "not a calendar"}"#)["ok"], json!(false));
    }
}
