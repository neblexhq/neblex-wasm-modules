//! Neblex sandboxed module: redact personal data inside free text before it reaches an AI step, a log or a ticket.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the value types and modes.

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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(redact_record).collect()),
        Ok(v) => redact_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

const ALL: [&str; 6] = ["email", "phone", "card", "ssn", "iban", "ip"];

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Placeholder,
    Mask,
    Remove,
}

struct Options {
    fields: Vec<String>,
    types: Vec<String>,
    mode: Mode,
    terms: Vec<(String, String)>,
    return_map: bool,
}

fn options(v: &Value) -> Options {
    let strs = |k: &str| v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect::<Vec<_>>());
    // `terms` is a list of strings (labelled NAME) or { "label": [strings] }.
    let terms = match v.get("terms") {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(|t| ("name".to_owned(), t.to_owned())).collect(),
        Some(Value::Object(m)) => m
            .iter()
            .flat_map(|(label, list)| {
                let label = label.to_lowercase();
                list.as_array().into_iter().flatten().filter_map(|x| x.as_str()).map(move |t| (label.clone(), t.to_owned()))
            })
            .collect(),
        _ => Vec::new(),
    };
    Options {
        fields: strs("fields").or_else(|| v.get("field").and_then(Value::as_str).map(|f| vec![f.to_owned()])).unwrap_or_else(|| vec!["text".into()]),
        types: strs("types").unwrap_or_else(|| ALL.iter().map(|s| s.to_string()).collect()),
        mode: match v.get("mode").and_then(Value::as_str) {
            Some("mask") => Mode::Mask,
            Some("remove") => Mode::Remove,
            _ => Mode::Placeholder,
        },
        terms,
        return_map: v.get("return_map").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Redacts each field named in `_options.fields` (default `text`) in place and adds
/// `redactions` (type, field, placeholder, position) and `redaction_count`. With
/// `return_map`, also `redaction_map` from placeholder to the original value.
fn redact_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = options(&m.remove("_options").unwrap_or(Value::Null));
    // Numbering is shared across fields, so the same value gets the same placeholder everywhere.
    let mut numbering: Vec<(String, String, String)> = Vec::new();
    let mut log = Vec::new();
    for field in &opt.fields {
        let Some(text) = m.get(field).and_then(Value::as_str).map(str::to_owned) else { continue };
        let (out, found) = redact(&text, &opt, &mut numbering);
        for f in found {
            log.push(json!({ "type": f.kind, "field": field, "placeholder": f.replacement, "start": f.start, "end": f.end }));
        }
        m.insert(field.clone(), json!(out));
    }
    m.insert("redaction_count".into(), json!(log.len()));
    m.insert("redactions".into(), Value::Array(log));
    if opt.return_map {
        let map: Map<String, Value> = numbering.iter().map(|(_, orig, ph)| (ph.clone(), json!(orig))).collect();
        m.insert("redaction_map".into(), Value::Object(map));
    }
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

struct Found {
    kind: String,
    start: usize,
    end: usize,
    replacement: String,
}

fn redact(text: &str, opt: &Options, numbering: &mut Vec<(String, String, String)>) -> (String, Vec<Found>) {
    let s: Vec<char> = text.chars().collect();
    let on = |t: &str| opt.types.iter().any(|x| x == t);
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    // Most specific first; later scanners skip spans already taken.
    if !opt.terms.is_empty() {
        terms(&s, &opt.terms, &mut spans);
    }
    if on("email") {
        emails(&s, &mut spans);
    }
    if on("iban") {
        ibans(&s, &mut spans);
    }
    if on("card") {
        digit_runs(&s, &mut spans, "card");
    }
    if on("ssn") {
        ssns(&s, &mut spans);
    }
    if on("ip") {
        ips(&s, &mut spans);
    }
    if on("phone") {
        digit_runs(&s, &mut spans, "phone");
    }
    spans.sort_by_key(|x| x.0);
    let mut out = String::new();
    let mut found = Vec::new();
    let mut last = 0;
    for (a, b, kind) in spans {
        out.extend(&s[last..a]);
        let original: String = s[a..b].iter().collect();
        let replacement = match opt.mode {
            Mode::Remove => "[REDACTED]".to_owned(),
            Mode::Mask => mask(&kind, &original),
            Mode::Placeholder => {
                let key = normalize(&kind, &original);
                match numbering.iter().find(|(k, n, _)| *k == kind && normalize(k, n) == key) {
                    Some((_, _, ph)) => ph.clone(),
                    None => {
                        let n = numbering.iter().filter(|(k, ..)| *k == kind).count() + 1;
                        let ph = format!("[{}_{n}]", kind.to_uppercase());
                        numbering.push((kind.clone(), original.clone(), ph.clone()));
                        ph
                    }
                }
            }
        };
        if opt.mode != Mode::Placeholder && !numbering.iter().any(|(_, o, _)| *o == original) {
            numbering.push((kind.clone(), original.clone(), replacement.clone()));
        }
        out.push_str(&replacement);
        found.push(Found { kind, start: a, end: b, replacement });
        last = b;
    }
    out.extend(&s[last..]);
    (out, found)
}

/// The comparison key for "same value": lowercase emails and terms, digits only for numbers.
fn normalize(kind: &str, v: &str) -> String {
    match kind {
        "email" | "ip" => v.to_lowercase(),
        "phone" | "card" | "ssn" => v.chars().filter(char::is_ascii_digit).collect(),
        "iban" => v.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase(),
        _ => v.to_lowercase(),
    }
}

/// Keeps a little context: the email's first letter and domain, the last 4 digits of
/// numbers, the first octet of an IP address. Everything else becomes asterisks.
fn mask(kind: &str, v: &str) -> String {
    match kind {
        "email" => match v.split_once('@') {
            Some((local, domain)) => format!("{}***@{domain}", local.chars().next().unwrap_or('*')),
            None => "***".into(),
        },
        "phone" | "card" | "ssn" | "iban" => {
            let keep: String = v.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            let tail: String = keep.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            format!("****{tail}")
        }
        "ip" => format!("{}.*.*.*", v.split('.').next().unwrap_or("*")),
        _ => "*".repeat(v.chars().count().min(12)),
    }
}

fn taken(spans: &[(usize, usize, String)], a: usize, b: usize) -> bool {
    spans.iter().any(|(x, y, _)| a < *y && *x < b)
}

fn add(spans: &mut Vec<(usize, usize, String)>, a: usize, b: usize, kind: &str) {
    if !taken(spans, a, b) {
        spans.push((a, b, kind.to_owned()));
    }
}

fn boundary(s: &[char], i: usize) -> bool {
    i == 0 || !s[i - 1].is_alphanumeric()
}

fn end_boundary(s: &[char], i: usize) -> bool {
    i >= s.len() || !s[i].is_alphanumeric()
}

/// Exact terms (customer names, project code names), matched case-insensitively on word boundaries.
fn terms(s: &[char], list: &[(String, String)], spans: &mut Vec<(usize, usize, String)>) {
    let low: Vec<char> = s.iter().flat_map(|c| c.to_lowercase()).collect();
    if low.len() != s.len() {
        return; // lowercasing changed the length; skip rather than misplace spans
    }
    let mut sorted: Vec<&(String, String)> = list.iter().filter(|(_, t)| !t.trim().is_empty()).collect();
    sorted.sort_by_key(|(_, t)| std::cmp::Reverse(t.chars().count()));
    for (label, term) in sorted {
        let t: Vec<char> = term.to_lowercase().chars().collect();
        let mut i = 0;
        while i + t.len() <= low.len() {
            if low[i..i + t.len()] == t[..] && boundary(s, i) && end_boundary(s, i + t.len()) {
                add(spans, i, i + t.len(), label);
                i += t.len();
            } else {
                i += 1;
            }
        }
    }
}

fn emails(s: &[char], spans: &mut Vec<(usize, usize, String)>) {
    let local_ok = |c: char| c.is_alphanumeric() || "._%+-'".contains(c);
    let dom_ok = |c: char| c.is_alphanumeric() || c == '-' || c == '.';
    for at in 0..s.len() {
        if s[at] != '@' {
            continue;
        }
        let mut a = at;
        while a > 0 && local_ok(s[a - 1]) {
            a -= 1;
        }
        while a < at && !s[a].is_alphanumeric() {
            a += 1;
        }
        let mut b = at + 1;
        while b < s.len() && dom_ok(s[b]) {
            b += 1;
        }
        while b > at + 1 && matches!(s[b - 1], '.' | '-') {
            b -= 1;
        }
        let domain: String = s[at + 1..b].iter().collect();
        let tld_ok = domain.rsplit('.').next().is_some_and(|t| t.len() >= 2 && t.chars().all(char::is_alphabetic));
        if a < at && domain.contains('.') && tld_ok {
            add(spans, a, b, "email");
        }
    }
}

fn mod97(s: &str) -> bool {
    let re = format!("{}{}", &s[4..], &s[..4]);
    let mut r: u32 = 0;
    for c in re.chars() {
        let Some(v) = c.to_digit(36) else { return false };
        r = if v >= 10 { (r * 100 + v) % 97 } else { (r * 10 + v) % 97 };
    }
    r == 1
}

fn ibans(s: &[char], spans: &mut Vec<(usize, usize, String)>) {
    for a in 0..s.len() {
        let head = boundary(s, a)
            && s[a].is_ascii_uppercase()
            && s.get(a + 1).is_some_and(|c| c.is_ascii_uppercase())
            && s.get(a + 2).is_some_and(char::is_ascii_digit)
            && s.get(a + 3).is_some_and(char::is_ascii_digit);
        if !head {
            continue;
        }
        let mut b = a;
        let mut compact = String::new();
        while b < s.len() && compact.len() < 34 {
            if s[b].is_ascii_alphanumeric() {
                compact.push(s[b].to_ascii_uppercase());
                b += 1;
            } else if s[b] == ' ' && compact.len() % 4 == 0 && s.get(b + 1).is_some_and(|c| c.is_ascii_alphanumeric()) {
                b += 1;
            } else {
                break;
            }
        }
        let mut end = b;
        while compact.len() >= 15 {
            if mod97(&compact) && end_boundary(s, end) {
                add(spans, a, end, "iban");
                break;
            }
            let drop = if compact.len() % 4 == 0 { 4 } else { compact.len() % 4 };
            compact.truncate(compact.len() - drop);
            end -= drop;
            while end > a && s[end - 1] == ' ' {
                end -= 1;
            }
        }
    }
}

fn luhn(d: &str) -> bool {
    let sum: u32 = d
        .chars()
        .rev()
        .filter_map(|c| c.to_digit(10))
        .enumerate()
        .map(|(i, v)| if i % 2 == 1 { if v * 2 > 9 { v * 2 - 9 } else { v * 2 } } else { v })
        .sum();
    sum % 10 == 0
}

/// Runs of digits with spaces, dashes, dots or brackets between them. As "card": 13 to 19
/// digits passing the Luhn check. As "phone": 10 to 15 digits with separators or a leading +
/// (a bare 10-digit run counts only with a separator, so order numbers aren't hidden).
fn digit_runs(s: &[char], spans: &mut Vec<(usize, usize, String)>, kind: &str) {
    let mut i = 0;
    while i < s.len() {
        let starts = (s[i].is_ascii_digit() || (kind == "phone" && (s[i] == '+' || s[i] == '('))) && boundary(s, i);
        if !starts {
            i += 1;
            continue;
        }
        let a = i;
        let mut b = i;
        let mut digits = String::new();
        while b < s.len() {
            let c = s[b];
            if c.is_ascii_digit() {
                digits.push(c);
            } else if matches!(c, ' ' | '-' | '.' | '(' | ')') || (c == '+' && b == a) {
                if c == ' ' && !s.get(b + 1).is_some_and(|n| n.is_ascii_digit() || *n == '(') {
                    break;
                }
            } else {
                break;
            }
            b += 1;
        }
        while b > a && !s[b - 1].is_ascii_digit() {
            b -= 1;
        }
        let seps = s[a..b].iter().filter(|c| !c.is_ascii_digit()).count();
        let ok = end_boundary(s, b)
            && match kind {
                "card" => (13..=19).contains(&digits.len()) && luhn(&digits) && !digits.chars().all(|c| c == '0'),
                _ => (10..=15).contains(&digits.len()) && (s[a] == '+' || seps > 0),
            };
        if ok {
            add(spans, a, b, kind);
        }
        i = b.max(i + 1);
    }
}

/// US Social Security numbers written as 123-45-6789 (area not 000, 666 or 9xx; group and serial not zero).
fn ssns(s: &[char], spans: &mut Vec<(usize, usize, String)>) {
    for a in 0..s.len().saturating_sub(10) {
        if !boundary(s, a) || !end_boundary(s, a + 11) {
            continue;
        }
        let t: String = s[a..a + 11].iter().collect();
        let b = t.as_bytes();
        let shape = b.iter().enumerate().all(|(i, c)| if i == 3 || i == 6 { *c == b'-' } else { c.is_ascii_digit() });
        if !shape {
            continue;
        }
        let (area, group, serial) = (&t[..3], &t[4..6], &t[7..]);
        if area != "000" && area != "666" && !area.starts_with('9') && group != "00" && serial != "0000" {
            add(spans, a, a + 11, "ssn");
        }
    }
}

/// IPv4 addresses (four 0-255 parts). IPv6 is not covered.
fn ips(s: &[char], spans: &mut Vec<(usize, usize, String)>) {
    let mut i = 0;
    while i < s.len() {
        if !(s[i].is_ascii_digit() && boundary(s, i)) {
            i += 1;
            continue;
        }
        let mut b = i;
        while b < s.len() && (s[b].is_ascii_digit() || s[b] == '.') {
            b += 1;
        }
        while b > i && s[b - 1] == '.' {
            b -= 1;
        }
        let t: String = s[i..b].iter().collect();
        let parts: Vec<&str> = t.split('.').collect();
        if parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u16>().is_ok_and(|n| n <= 255)) && end_boundary(s, b) {
            add(spans, i, b, "ip");
        }
        i = b.max(i + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    const TEXT: &str = "Hi, I'm Jane Doe (jane.doe@example.com, +1 415-555-0132). Card 4111 1111 1111 1111 was charged twice. \
SSN 123-45-6789. Refund to DE89 3704 0044 0532 0130 00. Logged from 203.0.113.42. Email JANE.DOE@example.com again. Order 4411520091.";

    #[test]
    fn placeholders_are_consistent() {
        let out = go(json!({ "text": TEXT, "_options": { "terms": ["Jane Doe"], "return_map": true } }));
        assert_eq!(
            out["text"],
            json!("Hi, I'm [NAME_1] ([EMAIL_1], [PHONE_1]). Card [CARD_1] was charged twice. SSN [SSN_1]. Refund to [IBAN_1]. Logged from [IP_1]. Email [EMAIL_1] again. Order 4411520091.")
        );
        assert_eq!(out["redaction_count"], json!(8));
        assert_eq!(out["redaction_map"]["[EMAIL_1]"], json!("jane.doe@example.com"));
        assert_eq!(out["redaction_map"]["[CARD_1]"], json!("4111 1111 1111 1111"));
    }

    #[test]
    fn mask_and_remove_modes() {
        let m = go(json!({ "text": "jane.doe@example.com paid with 4111-1111-1111-1111 from 203.0.113.42", "_options": { "mode": "mask" } }));
        assert_eq!(m["text"], json!("j***@example.com paid with ****1111 from 203.*.*.*"));
        let r = go(json!({ "text": "call (415) 555-0132", "_options": { "mode": "remove" } }));
        assert_eq!(r["text"], json!("call [REDACTED]"));
    }

    #[test]
    fn leaves_ordinary_numbers_alone() {
        let out = go(json!({ "text": "PO 4411520091, invoice 1234567890123 (fails Luhn), SSN-like 000-12-3456, version 1.2.3.4.5, zip 94107, total 1,250.00" }));
        assert_eq!(out["redaction_count"], json!(0), "{out}");
    }

    #[test]
    fn several_fields_share_numbering_and_types_filter() {
        let out = go(json!({
            "subject": "Refund for jane@example.com",
            "body": "Please refund jane@example.com and copy bob@example.com. Call 415-555-0132.",
            "_options": { "fields": ["subject", "body"], "types": ["email"] }
        }));
        assert_eq!(out["subject"], json!("Refund for [EMAIL_1]"));
        assert_eq!(out["body"], json!("Please refund [EMAIL_1] and copy [EMAIL_2]. Call 415-555-0132."));
    }

    #[test]
    fn labelled_terms() {
        let out = go(json!({ "text": "Acme Corp asked about Project Falcon.", "_options": { "types": [], "terms": { "customer": ["Acme Corp"], "project": ["Project Falcon"] } } }));
        assert_eq!(out["text"], json!("[CUSTOMER_1] asked about [PROJECT_1]."));
    }
}
