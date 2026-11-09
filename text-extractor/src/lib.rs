//! Neblex sandboxed module: pull structured values out of free text without an AI model.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the value types and options.

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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(extract_record).collect()),
        Ok(v) => extract_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

const ALL: [&str; 8] = ["email", "url", "phone", "date", "amount", "iban", "tracking", "reference"];

struct Ref {
    name: String,
    labels: Vec<String>,
    digits_only: bool,
    min: usize,
    max: usize,
}

struct Options {
    field: String,
    types: Vec<String>,
    date_order: String,
    references: Vec<Ref>,
    context: usize,
}

fn options(v: &Value) -> Options {
    let types = v
        .get("types")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
        .unwrap_or_else(|| ALL.iter().map(|s| s.to_string()).collect());
    let references = v
        .get("references")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(name, r)| Ref {
                    name: name.clone(),
                    labels: r.get("labels").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_lowercase)).collect()).unwrap_or_default(),
                    digits_only: r.get("digits_only").and_then(Value::as_bool).unwrap_or(false),
                    min: r.get("min").and_then(Value::as_u64).unwrap_or(3) as usize,
                    max: r.get("max").and_then(Value::as_u64).unwrap_or(30) as usize,
                })
                .collect()
        })
        .unwrap_or_else(|| {
            vec![
                Ref { name: "invoice_number".into(), labels: vec!["invoice no".into(), "invoice number".into(), "invoice #".into(), "invoice:".into(), "inv #".into(), "inv no".into()], digits_only: false, min: 3, max: 30 },
                Ref { name: "po_number".into(), labels: vec!["purchase order".into(), "po number".into(), "po no".into(), "p.o.".into(), "po #".into(), "po:".into(), "po".into()], digits_only: false, min: 3, max: 30 },
                Ref { name: "order_number".into(), labels: vec!["order number".into(), "order no".into(), "order #".into(), "order:".into()], digits_only: false, min: 3, max: 30 },
            ]
        });
    Options {
        field: v.get("field").and_then(Value::as_str).unwrap_or("text").to_owned(),
        types,
        date_order: v.get("date_order").and_then(Value::as_str).unwrap_or("").to_lowercase(),
        references,
        context: v.get("context").and_then(Value::as_u64).unwrap_or(30) as usize,
    }
}

/// Adds `entities` (every match in order, with position and context) and `found`
/// (unique normalized values per type) to the record. `_options` is removed.
fn extract_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = options(&m.remove("_options").unwrap_or(Value::Null));
    let Some(text) = m.get(&opt.field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{}' is missing or not text", opt.field)));
        return Value::Object(m);
    };
    let ents = extract(&text, &opt);
    let mut found = Map::new();
    for e in &ents {
        let key = e.kind.clone();
        let arr = found.entry(key).or_insert_with(|| json!([])).as_array_mut().unwrap();
        if !arr.contains(&e.normalized) {
            arr.push(e.normalized.clone());
        }
    }
    let chars: Vec<char> = text.chars().collect();
    let list: Vec<Value> = ents
        .iter()
        .map(|e| {
            let lo = e.start.saturating_sub(opt.context);
            let hi = (e.end + opt.context).min(chars.len());
            let ctx: String = chars[lo..hi].iter().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ");
            let mut o = json!({ "type": e.kind, "text": e.raw, "value": e.normalized, "start": e.start, "end": e.end, "context": ctx });
            if let Some(extra) = &e.extra {
                o.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            }
            o
        })
        .collect();
    m.insert("entities".into(), Value::Array(list));
    m.insert("found".into(), Value::Object(found));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

pub struct Ent {
    pub kind: String,
    pub raw: String,
    pub normalized: Value,
    pub start: usize,
    pub end: usize,
    pub extra: Option<Value>,
}

pub fn extract(text: &str, opt: &Options) -> Vec<Ent> {
    let s: Vec<char> = text.chars().collect();
    let on = |t: &str| opt.types.iter().any(|x| x == t);
    let mut out: Vec<Ent> = Vec::new();
    // Most specific first; later scanners skip spans already taken.
    if on("email") {
        emails(&s, &mut out);
    }
    if on("url") {
        urls(&s, &mut out);
    }
    if on("iban") {
        ibans(&s, &mut out);
    }
    if on("tracking") {
        tracking(&s, &mut out);
    }
    if on("reference") {
        references(&s, opt, &mut out);
    }
    if on("date") {
        dates(&s, opt, &mut out);
    }
    if on("amount") {
        amounts(&s, &mut out);
    }
    if on("phone") {
        phones(&s, &mut out);
    }
    out.sort_by_key(|e| e.start);
    out
}

fn taken(out: &[Ent], a: usize, b: usize) -> bool {
    out.iter().any(|e| a < e.end && e.start < b)
}

fn push(out: &mut Vec<Ent>, s: &[char], kind: &str, a: usize, b: usize, normalized: Value, extra: Option<Value>) {
    if !taken(out, a, b) {
        out.push(Ent { kind: kind.into(), raw: s[a..b].iter().collect(), normalized, start: a, end: b, extra });
    }
}

fn boundary(s: &[char], i: usize) -> bool {
    i == 0 || !s[i - 1].is_alphanumeric()
}

fn end_boundary(s: &[char], i: usize) -> bool {
    i >= s.len() || !s[i].is_alphanumeric()
}

fn trim_punct(s: &[char], a: usize, mut b: usize) -> usize {
    while b > a && matches!(s[b - 1], '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '\'' | '"' | '>') {
        b -= 1;
    }
    b
}

fn emails(s: &[char], out: &mut Vec<Ent>) {
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
        let b = trim_punct(s, at + 1, b);
        let domain: String = s[at + 1..b].iter().collect();
        let tld_ok = domain.rsplit('.').next().is_some_and(|t| t.len() >= 2 && t.chars().all(char::is_alphabetic));
        if a < at && domain.contains('.') && tld_ok && !domain.contains("..") {
            let raw: String = s[a..b].iter().collect();
            push(out, s, "email", a, b, json!(raw.to_lowercase()), None);
        }
    }
}

fn urls(s: &[char], out: &mut Vec<Ent>) {
    let text: String = s.iter().collect::<String>().to_lowercase();
    let low: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < s.len() {
        let rest: String = low[i..(i + 8).min(low.len())].iter().collect();
        let starts = (rest.starts_with("http://") || rest.starts_with("https://") || rest.starts_with("www.")) && boundary(s, i);
        if !starts {
            i += 1;
            continue;
        }
        let mut b = i;
        while b < s.len() && !s[b].is_whitespace() && !matches!(s[b], '<' | '>' | '"') {
            b += 1;
        }
        let b = trim_punct(s, i, b);
        let raw: String = s[i..b].iter().collect();
        if raw.len() > 10 {
            let norm = if raw.to_lowercase().starts_with("www.") { format!("https://{raw}") } else { raw.clone() };
            push(out, s, "url", i, b, json!(norm), None);
        }
        i = b.max(i + 1);
    }
}

fn mod97(s: &str) -> bool {
    let re: String = format!("{}{}", &s[4..], &s[..4]);
    let mut r: u32 = 0;
    for c in re.chars() {
        let v = c.to_digit(36).unwrap_or(99);
        if v == 99 {
            return false;
        }
        r = if v >= 10 { (r * 100 + v) % 97 } else { (r * 10 + v) % 97 };
    }
    r == 1
}

fn ibans(s: &[char], out: &mut Vec<Ent>) {
    for a in 0..s.len() {
        if !(boundary(s, a) && s[a].is_ascii_uppercase() && s.get(a + 1).is_some_and(|c| c.is_ascii_uppercase()) && s.get(a + 2).is_some_and(|c| c.is_ascii_digit()) && s.get(a + 3).is_some_and(|c| c.is_ascii_digit())) {
            continue;
        }
        let mut b = a;
        let mut compact = String::new();
        while b < s.len() && compact.len() < 34 {
            if s[b].is_ascii_alphanumeric() {
                compact.push(s[b].to_ascii_uppercase());
                b += 1;
            } else if s[b] == ' ' && s.get(b + 1).is_some_and(|c| c.is_ascii_alphanumeric()) && compact.len() % 4 == 0 {
                b += 1;
            } else {
                break;
            }
        }
        // Back off one group at a time until the check digits work (stops before trailing words).
        let mut end = b;
        while compact.len() >= 15 {
            if mod97(&compact) && end_boundary(s, end) {
                push(out, s, "iban", a, end, json!(compact), Some(json!({ "country": &compact[..2] })));
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

/// UPS 1Z numbers (check digit verified) and USPS 20/22-digit numbers starting 9 (mod 10 check).
fn tracking(s: &[char], out: &mut Vec<Ent>) {
    for a in 0..s.len() {
        if !boundary(s, a) {
            continue;
        }
        if s[a].to_ascii_uppercase() == '1' && s.get(a + 1).is_some_and(|c| c.to_ascii_uppercase() == 'Z') {
            let tok: String = s[a..(a + 18).min(s.len())].iter().collect::<String>().to_uppercase();
            if tok.len() == 18 && end_boundary(s, a + 18) && tok.chars().all(|c| c.is_ascii_alphanumeric()) && ups_ok(&tok) {
                push(out, s, "tracking", a, a + 18, json!(tok), Some(json!({ "carrier": "UPS" })));
            }
            continue;
        }
        if s[a] == '9' {
            let mut b = a;
            let mut digits = String::new();
            while b < s.len() && (s[b].is_ascii_digit() || (s[b] == ' ' && s.get(b + 1).is_some_and(char::is_ascii_digit))) {
                if s[b].is_ascii_digit() {
                    digits.push(s[b]);
                }
                b += 1;
            }
            if (digits.len() == 22 || digits.len() == 20) && end_boundary(s, b) && luhn_usps(&digits) {
                push(out, s, "tracking", a, b, json!(digits), Some(json!({ "carrier": "USPS" })));
            }
        }
    }
}

fn ups_ok(t: &str) -> bool {
    let body: Vec<u32> = t[2..17]
        .chars()
        .map(|c| if c.is_ascii_digit() { c.to_digit(10).unwrap() } else { ((c as u32 - 'A' as u32) + 2) % 10 })
        .collect();
    let sum: u32 = body.iter().enumerate().map(|(i, v)| if i % 2 == 1 { v * 2 } else { *v }).sum();
    let check = (10 - sum % 10) % 10;
    t[17..].chars().next().and_then(|c| c.to_digit(10)) == Some(check)
}

/// USPS IMpb check digit: from the right, excluding the check digit, odd positions x3.
fn luhn_usps(d: &str) -> bool {
    let digits: Vec<u32> = d.chars().map(|c| c.to_digit(10).unwrap()).collect();
    let (body, check) = digits.split_at(digits.len() - 1);
    let sum: u32 = body.iter().rev().enumerate().map(|(i, v)| if i % 2 == 0 { v * 3 } else { *v }).sum();
    (10 - sum % 10) % 10 == check[0]
}

fn references(s: &[char], opt: &Options, out: &mut Vec<Ent>) {
    let low: Vec<char> = s.iter().map(|c| c.to_ascii_lowercase()).collect();
    for r in &opt.references {
        for label in &r.labels {
            let lab: Vec<char> = label.chars().collect();
            let mut i = 0;
            while i + lab.len() <= low.len() {
                if low[i..i + lab.len()] != lab[..] || !boundary(s, i) || (lab.last().is_some_and(|c| c.is_alphanumeric()) && !end_boundary(s, i + lab.len())) {
                    i += 1;
                    continue;
                }
                let mut j = i + lab.len();
                // Separators between a label and its value: spaces, '#', ':', '.', '-', "no", "number".
                loop {
                    while j < s.len() && matches!(s[j], ' ' | '#' | ':' | '.' | '-' | '\t') {
                        j += 1;
                    }
                    let rest: String = low[j..(j + 7).min(low.len())].iter().collect();
                    if rest.starts_with("number") {
                        j += 6;
                    } else if rest.starts_with("no") && !s.get(j + 2).is_some_and(|c| c.is_alphanumeric()) {
                        j += 2;
                    } else {
                        break;
                    }
                }
                let a = j;
                while j < s.len() && (s[j].is_ascii_alphanumeric() || (matches!(s[j], '-' | '/' | '_') && s.get(j + 1).is_some_and(char::is_ascii_alphanumeric))) {
                    j += 1;
                }
                let val: String = s[a..j].iter().collect();
                let ok = val.len() >= r.min && val.len() <= r.max && val.chars().any(|c| c.is_ascii_digit()) && (!r.digits_only || val.chars().all(|c| c.is_ascii_digit()));
                if ok {
                    push(out, s, "reference", a, j, json!(val), Some(json!({ "name": r.name })));
                }
                i = j.max(i + 1);
            }
        }
    }
}

const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

fn month(word: &str) -> Option<u32> {
    let w = word.to_lowercase();
    let w = w.trim_end_matches('.');
    if w.len() < 3 {
        return None;
    }
    let full = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
    MONTHS.iter().position(|m| w == *m || (w == "sept" && *m == "sep") || full.iter().any(|f| f.starts_with(w) && f.starts_with(m) && w.len() >= 3)).map(|i| i as u32 + 1)
}

fn valid_date(y: u32, m: u32, d: u32) -> bool {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    (1900..=2200).contains(&y) && (1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1]
}

/// ISO dates, dates with month names, and numeric dates. A numeric date like 04/03/2026 is
/// only resolved when one part is over 12 or `date_order` ("mdy" or "dmy") is set;
/// otherwise it is returned with "ambiguous": true and no ISO value.
fn dates(s: &[char], opt: &Options, out: &mut Vec<Ent>) {
    let mut tokens: Vec<(usize, usize, String)> = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if s[i].is_alphanumeric() {
            let a = i;
            while i < s.len() && (s[i].is_alphanumeric() || (matches!(s[i], '/' | '-' | '.') && s.get(i + 1).is_some_and(|c| c.is_alphanumeric()))) {
                i += 1;
            }
            tokens.push((a, i, s[a..i].iter().collect()));
        } else {
            i += 1;
        }
    }
    let num =|t: &str| -> Option<u32> { t.trim_end_matches(|c: char| c.is_alphabetic()).parse().ok().filter(|_| t.chars().next().is_some_and(|c| c.is_ascii_digit())) };
    for k in 0..tokens.len() {
        let (a, b, t) = &tokens[k];
        let parts: Vec<&str> = t.split(['/', '-', '.']).collect();
        if parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
            let n: Vec<u32> = parts.iter().map(|p| p.parse().unwrap()).collect();
            if parts[0].len() == 4 {
                if valid_date(n[0], n[1], n[2]) {
                    push(out, s, "date", *a, *b, json!(format!("{:04}-{:02}-{:02}", n[0], n[1], n[2])), None);
                }
                continue;
            }
            if parts[2].len() != 4 && parts[2].len() != 2 {
                continue;
            }
            let y = if parts[2].len() == 2 { if n[2] < 70 { 2000 + n[2] } else { 1900 + n[2] } } else { n[2] };
            let (md, dm) = (valid_date(y, n[0], n[1]), valid_date(y, n[1], n[0]));
            let pick = match (md, dm, opt.date_order.as_str()) {
                (true, false, _) => Some((n[0], n[1])),
                (false, true, _) => Some((n[1], n[0])),
                (true, true, "mdy") => Some((n[0], n[1])),
                (true, true, "dmy") => Some((n[1], n[0])),
                (true, true, _) if n[0] == n[1] => Some((n[0], n[1])),
                (true, true, _) => {
                    push(out, s, "date", *a, *b, Value::Null, Some(json!({ "ambiguous": true })));
                    continue;
                }
                _ => None,
            };
            if let Some((m, d)) = pick {
                push(out, s, "date", *a, *b, json!(format!("{y:04}-{m:02}-{d:02}")), None);
            }
            continue;
        }
        // Month-name forms: "March 5, 2026", "Mar 5 2026", "5 March 2026".
        if let (Some(m), Some((_, b1, t1)), Some((_, b2, t2))) = (month(t), tokens.get(k + 1), tokens.get(k + 2)) {
            if let (Some(d), Ok(y)) = (num(t1), t2.parse::<u32>()) {
                if t2.len() == 4 && valid_date(y, m, d) && *b1 < *b2 {
                    push(out, s, "date", *a, *b2, json!(format!("{y:04}-{m:02}-{d:02}")), None);
                    continue;
                }
            }
        }
        if let (Some(d), Some((_, _, t1)), Some((_, b2, t2))) = (num(t), tokens.get(k + 1), tokens.get(k + 2)) {
            if let (Some(m), Ok(y)) = (month(t1), t2.parse::<u32>()) {
                if t2.len() == 4 && t.len() <= 4 && valid_date(y, m, d) {
                    push(out, s, "date", *a, *b2, json!(format!("{y:04}-{m:02}-{d:02}")), None);
                }
            }
        }
    }
}

const CODES: [&str; 10] = ["USD", "EUR", "GBP", "CAD", "AUD", "CHF", "JPY", "MXN", "BRL", "INR"];

fn symbol(c: char) -> Option<&'static str> {
    match c {
        '$' => Some("USD"),
        '€' => Some("EUR"),
        '£' => Some("GBP"),
        '¥' => Some("JPY"),
        '₹' => Some("INR"),
        _ => None,
    }
}

/// Parses "1,234.56", "1.234,56", "1 234,56", "1250" or "1250.5" to a number.
/// The last separator is decimal when 1 or 2 digits follow it.
pub fn parse_amount(raw: &str) -> Option<f64> {
    let t: String = raw.chars().filter(|c| !c.is_whitespace() && *c != '\'' && *c != '\u{a0}' && *c != '\u{202f}').collect();
    let last = t.rfind(['.', ',']);
    let (int, frac) = match last {
        Some(p) if t.len() - p - 1 <= 2 && t.len() - p - 1 >= 1 => (&t[..p], &t[p + 1..]),
        _ => (&t[..], ""),
    };
    let int: String = int.chars().filter(|c| c.is_ascii_digit()).collect();
    if int.is_empty() && frac.is_empty() {
        return None;
    }
    format!("{}.{}", if int.is_empty() { "0" } else { &int }, if frac.is_empty() { "0" } else { frac }).parse().ok()
}

fn amounts(s: &[char], out: &mut Vec<Ent>) {
    let mut i = 0;
    while i < s.len() {
        // Currency marker: a symbol, or a 3-letter code followed by space, before or after the number.
        let (cur, num_start) = if let Some(c) = symbol(s[i]) {
            (Some(c), i + 1)
        } else if i + 3 <= s.len() && boundary(s, i) {
            let w: String = s[i..i + 3].iter().collect();
            if CODES.contains(&w.as_str()) && end_boundary(s, i + 3) {
                (Some(CODES[CODES.iter().position(|c| *c == w).unwrap()]), i + 3)
            } else {
                (None, i)
            }
        } else {
            (None, i)
        };
        let Some(code) = cur else {
            i += 1;
            continue;
        };
        let mut a = num_start;
        while a < s.len() && s[a] == ' ' {
            a += 1;
        }
        let neg = s.get(a) == Some(&'-');
        if neg {
            a += 1;
        }
        let mut b = a;
        while b < s.len() && (s[b].is_ascii_digit() || (matches!(s[b], ',' | '.' | ' ' | '\'') && s.get(b + 1).is_some_and(char::is_ascii_digit) && b > a)) {
            b += 1;
        }
        if b > a {
            let raw: String = s[a..b].iter().collect();
            if let Some(v) = parse_amount(&raw) {
                let v = if neg { -v } else { v };
                push(out, s, "amount", i, b, json!(v), Some(json!({ "currency": code })));
                i = b;
                continue;
            }
        }
        i += 1;
    }
    // Number then code: "1.250,00 EUR".
    let mut i = 0;
    while i < s.len() {
        if !(s[i].is_ascii_digit() && boundary(s, i)) {
            i += 1;
            continue;
        }
        let a = i;
        let mut b = a;
        while b < s.len() && (s[b].is_ascii_digit() || (matches!(s[b], ',' | '.' | '\'') && s.get(b + 1).is_some_and(char::is_ascii_digit))) {
            b += 1;
        }
        let mut c = b;
        while c < s.len() && s[c] == ' ' {
            c += 1;
        }
        let code = if c + 3 <= s.len() { s[c..c + 3].iter().collect::<String>() } else { String::new() };
        let sym = s.get(c).and_then(|ch| symbol(*ch));
        let found = if CODES.contains(&code.as_str()) && end_boundary(s, c + 3) { Some((code.clone(), c + 3)) } else { sym.map(|x| (x.to_owned(), c + 1)) };
        if let Some((cur, end)) = found {
            let raw: String = s[a..b].iter().collect();
            if let Some(v) = parse_amount(&raw) {
                push(out, s, "amount", a, end, json!(v), Some(json!({ "currency": cur })));
            }
        }
        i = b.max(i + 1);
    }
}

/// Phone numbers: 10 to 15 digits with common separators. A leading + gives E.164;
/// 10 digits (or 11 starting with 1) that fit the North American plan are given +1.
fn phones(s: &[char], out: &mut Vec<Ent>) {
    let mut i = 0;
    while i < s.len() {
        if !((s[i] == '+' || s[i] == '(' || s[i].is_ascii_digit()) && boundary(s, i)) {
            i += 1;
            continue;
        }
        let a = i;
        let mut b = i;
        let mut digits = String::new();
        while b < s.len() && (s[b].is_ascii_digit() || matches!(s[b], '+' | '(' | ')' | '-' | '.' | ' ')) {
            if s[b].is_ascii_digit() {
                digits.push(s[b]);
            }
            if s[b] == '+' && b != a {
                break;
            }
            if s[b] == ' ' && !s.get(b + 1).is_some_and(|c| c.is_ascii_digit() || *c == '(') {
                break;
            }
            b += 1;
        }
        let b = {
            let mut e = b;
            while e > a && !s[e - 1].is_ascii_digit() {
                e -= 1;
            }
            e
        };
        let plus = s[a] == '+';
        let nanp = |d: &str| d.len() == 10 && !d.starts_with(['0', '1']) && !d[3..].starts_with(['0', '1']);
        let e164 = if plus && (8..=15).contains(&digits.len()) {
            Some(format!("+{digits}"))
        } else if nanp(&digits) {
            Some(format!("+1{digits}"))
        } else if digits.len() == 11 && digits.starts_with('1') && nanp(&digits[1..]) {
            Some(format!("+{digits}"))
        } else {
            None
        };
        let seps = s[a..b].iter().filter(|c| !c.is_ascii_digit()).count();
        if let Some(e) = e164 {
            if end_boundary(s, b) && (plus || seps > 0 || digits.len() == 10) {
                push(out, s, "phone", a, b, json!(e), None);
            }
        }
        i = b.max(i + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Options {
        options(&Value::Null)
    }

    fn found(text: &str) -> Value {
        let out = run(serde_json::to_vec(&json!({ "text": text })).unwrap().as_slice());
        out["found"].clone()
    }

    #[test]
    fn support_email() {
        let text = "Hi team, PO# 45-1180 arrived damaged. Invoice No: INV-2026-0091 for $1,250.00 was paid on March 5, 2026. \
                    Please call me at (415) 555-0132 or email Jane.Doe@Example.com. Return label: 1Z999AA10123456784. \
                    Refund to DE89 3704 0044 0532 0130 00. Details: https://example.com/claims/77?ref=a.";
        let f = found(text);
        assert_eq!(f["email"], json!(["jane.doe@example.com"]));
        assert_eq!(f["url"], json!(["https://example.com/claims/77?ref=a"]));
        assert_eq!(f["phone"], json!(["+14155550132"]));
        assert_eq!(f["date"], json!(["2026-03-05"]));
        assert_eq!(f["amount"], json!([1250.0]));
        assert_eq!(f["iban"], json!(["DE89370400440532013000"]));
        assert_eq!(f["tracking"], json!(["1Z999AA10123456784"]));
        assert_eq!(f["reference"], json!(["45-1180", "INV-2026-0091"]));
    }

    #[test]
    fn european_amounts_and_dates() {
        let f = found("Betrag 1.234,56 EUR, fällig 25/03/2026; bezahlt €99,90 am 2026-04-01. Ref 04/03/2026.");
        assert_eq!(f["amount"], json!([1234.56, 99.9]));
        assert_eq!(f["date"], json!(["2026-03-25", "2026-04-01", null]));
    }

    #[test]
    fn date_order_resolves_ambiguity() {
        let o = Options { date_order: "dmy".into(), ..opts() };
        let e = extract("due 04/03/2026", &o);
        assert_eq!(e[0].normalized, json!("2026-03-04"));
    }

    #[test]
    fn no_false_positives_on_plain_numbers() {
        let f = found("We shipped 12 units in 2026 to 3 sites; room 1201, version 4.2.1, zip 94107.");
        assert!(f.as_object().unwrap().is_empty(), "{f}");
    }

    #[test]
    fn checks_reject_bad_numbers() {
        let f = found("Bad UPS 1Z999AA10123456785 and bad IBAN DE89 3704 0044 0532 0130 01");
        assert!(f.get("tracking").is_none() && f.get("iban").is_none(), "{f}");
    }

    #[test]
    fn usps_check_digit() {
        // Build a valid 22-digit number from a 21-digit body.
        let body = "940011189922319742849";
        let d: Vec<u32> = body.chars().map(|c| c.to_digit(10).unwrap()).collect();
        let sum: u32 = d.iter().rev().enumerate().map(|(i, v)| if i % 2 == 0 { v * 3 } else { *v }).sum();
        let full = format!("{body}{}", (10 - sum % 10) % 10);
        let f = found(&format!("USPS tracking {full}."));
        assert_eq!(f["tracking"], json!([full]));
    }

    #[test]
    fn custom_references_and_types() {
        let out = run(br#"{"body": "Case CS-00412 re: ticket #88812", "_options": {"field": "body", "types": ["reference"],
            "references": {"case_id": {"labels": ["case"]}, "ticket": {"labels": ["ticket"], "digits_only": true}}}}"#);
        let names: Vec<_> = out["entities"].as_array().unwrap().iter().map(|e| (e["name"].clone(), e["value"].clone())).collect();
        assert_eq!(names, vec![(json!("case_id"), json!("CS-00412")), (json!("ticket"), json!("88812"))]);
    }
}
