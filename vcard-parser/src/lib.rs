//! Neblex sandboxed module: parse vCard (.vcf) contact cards, versions 2.1, 3.0 and 4.0, into
//! contact records, such as cards attached to emails or exported from phones and address books.
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

struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, k: &str) -> Option<&str> {
        self.params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    /// Types from TYPE=work,voice (3.0/4.0) or bare parameters such as ;WORK;CELL (2.1), lowercased.
    fn types(&self) -> Vec<String> {
        let mut t: Vec<String> = Vec::new();
        for (k, v) in &self.params {
            let vals: Vec<&str> = if k == "TYPE" { v.split(',').collect() } else if v.is_empty() { vec![k.as_str()] } else { vec![] };
            for x in vals {
                let x = x.trim().trim_matches('"').to_lowercase();
                if !x.is_empty() && x != "internet" && x != "x400" && !t.contains(&x) {
                    t.push(x);
                }
            }
        }
        t
    }
}

/// Lines with folding undone, including vCard 2.1 quoted-printable soft breaks (a line ending in "=").
fn unfold(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut qp_continue = false;
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if qp_continue && !out.is_empty() {
            let last = out.last_mut().unwrap();
            last.pop(); // the trailing "="
            last.push_str(line);
        } else if (line.starts_with(' ') || line.starts_with('\t')) && !out.is_empty() {
            out.last_mut().unwrap().push_str(&line[1..]);
        } else if !line.is_empty() {
            out.push(line.to_owned());
        }
        let last = out.last().map(String::as_str).unwrap_or("");
        qp_continue = last.to_uppercase().contains("QUOTED-PRINTABLE") && last.ends_with('=');
    }
    out
}

fn parse_prop(line: &str) -> Option<Prop> {
    let mut quoted = false;
    let colon = line.char_indices().find(|&(_, c)| {
        if c == '"' {
            quoted = !quoted;
        }
        c == ':' && !quoted
    })?.0;
    let head = &line[..colon];
    let mut parts: Vec<String> = Vec::new();
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
    // Group prefixes such as "item1.EMAIL" are dropped.
    let first = parts.remove(0);
    let name = first.rsplit('.').next().unwrap_or(&first).to_uppercase();
    let params = parts
        .into_iter()
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (k.trim().to_uppercase(), v.trim().to_owned()),
            None => (p.trim().to_uppercase(), String::new()),
        })
        .collect();
    Some(Prop { name, params, value: line[colon + 1..].to_owned() })
}

fn qp_decode(v: &str) -> Vec<u8> {
    let b = v.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&v[i + 1..i + 3], 16) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The property value as text: quoted-printable decoded (UTF-8 or Latin-1), escapes removed.
fn text_value(p: &Prop) -> String {
    if p.param("ENCODING").is_some_and(|e| e.eq_ignore_ascii_case("QUOTED-PRINTABLE")) || p.params.iter().any(|(k, _)| k == "QUOTED-PRINTABLE") {
        let bytes = qp_decode(&p.value);
        let latin1 = p.param("CHARSET").is_some_and(|c| c.eq_ignore_ascii_case("ISO-8859-1") || c.eq_ignore_ascii_case("WINDOWS-1252"));
        if latin1 { bytes.iter().map(|&b| b as char).collect() } else { String::from_utf8_lossy(&bytes).into_owned() }
    } else {
        p.value.clone()
    }
}

/// Splits a structured value on unescaped ";" and unescapes each part.
fn components(v: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut it = v.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some('n') | Some('N') => out.last_mut().unwrap().push('\n'),
                Some(o) => out.last_mut().unwrap().push(o),
                None => {}
            },
            ';' => out.push(String::new()),
            c => out.last_mut().unwrap().push(c),
        }
    }
    out.into_iter().map(|s| s.trim().to_owned()).collect()
}

fn unescape(v: &str) -> String {
    components(&v.replace(';', "\\;")).join(";")
}

fn opt(s: &str) -> Value {
    if s.is_empty() { Value::Null } else { json!(s) }
}

fn contact(props: &[Prop]) -> Value {
    let mut o = Map::new();
    let (mut emails, mut phones, mut addresses, mut urls) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for p in props {
        let v = text_value(p);
        match p.name.as_str() {
            "VERSION" => {
                o.insert("version".into(), json!(v.trim()));
            }
            "FN" => {
                o.insert("full_name".into(), json!(unescape(&v)));
            }
            "N" => {
                let c = components(&v);
                let g = |i: usize| c.get(i).map(String::as_str).unwrap_or("");
                o.insert("name".into(), json!({ "family": opt(g(0)), "given": opt(g(1)), "additional": opt(g(2)), "prefix": opt(g(3)), "suffix": opt(g(4)) }));
            }
            "ORG" => {
                let c = components(&v);
                o.insert("organization".into(), opt(&c[0]));
                if c.len() > 1 && !c[1].is_empty() {
                    o.insert("department".into(), json!(c[1]));
                }
            }
            "TITLE" => {
                o.insert("title".into(), json!(unescape(&v)));
            }
            "NOTE" => {
                o.insert("note".into(), json!(unescape(&v)));
            }
            "BDAY" => {
                let d = v.trim();
                let iso = if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) { format!("{}-{}-{}", &d[..4], &d[4..6], &d[6..]) } else { d.to_owned() };
                o.insert("birthday".into(), json!(iso));
            }
            "EMAIL" => {
                emails.push(json!({ "value": v.trim().to_lowercase(), "types": p.types() }));
            }
            "TEL" => {
                let val = v.trim();
                let val = val.strip_prefix("tel:").unwrap_or(val);
                phones.push(json!({ "value": val, "types": p.types() }));
            }
            "URL" => {
                urls.push(json!(unescape(v.trim())));
            }
            "ADR" => {
                let c = components(&v);
                let g = |i: usize| c.get(i).map(String::as_str).unwrap_or("");
                addresses.push(json!({
                    "types": p.types(), "po_box": opt(g(0)), "extended": opt(g(1)), "street": opt(g(2)),
                    "city": opt(g(3)), "region": opt(g(4)), "postal_code": opt(g(5)), "country": opt(g(6))
                }));
            }
            _ => {}
        }
    }
    // A card without FN still gets a display name from N.
    if !o.contains_key("full_name") {
        if let Some(n) = o.get("name") {
            let parts: Vec<&str> = ["prefix", "given", "additional", "family", "suffix"].iter().filter_map(|k| n[k].as_str()).collect();
            o.insert("full_name".into(), json!(parts.join(" ")));
        }
    }
    o.insert("emails".into(), Value::Array(emails));
    o.insert("phones".into(), Value::Array(phones));
    o.insert("addresses".into(), Value::Array(addresses));
    o.insert("urls".into(), Value::Array(urls));
    Value::Object(o)
}

pub fn parse(text: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<Prop>> = None;
    for line in unfold(text) {
        let Some(p) = parse_prop(&line) else { continue };
        match (p.name.as_str(), p.value.trim().to_uppercase().as_str()) {
            ("BEGIN", "VCARD") => cur = Some(Vec::new()),
            ("END", "VCARD") => {
                if let Some(props) = cur.take() {
                    out.push(contact(&props));
                }
            }
            _ => {
                if let Some(c) = cur.as_mut() {
                    c.push(p);
                }
            }
        }
    }
    out
}

/// Reads the vCard text from `_options.field` (default `vcf`) and adds `contacts` and `contact_count`.
fn parse_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let o = m.remove("_options").unwrap_or(Value::Null);
    let field = o.get("field").and_then(Value::as_str).unwrap_or("vcf").to_owned();
    let Some(text) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    if !o.get("keep_source").and_then(Value::as_bool).unwrap_or(false) {
        m.remove(&field);
    }
    if !text.to_uppercase().contains("BEGIN:VCARD") {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!("no BEGIN:VCARD found"));
        return Value::Object(m);
    }
    let contacts = parse(&text);
    m.insert("contact_count".into(), json!(contacts.len()));
    m.insert("contacts".into(), Value::Array(contacts));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARDS: &str = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Doe;Jane;Ann;Dr.;PhD\r\nFN:Dr. Jane Ann Doe\\, PhD\r\nORG:Acme Corp;Procurement\r\nTITLE:Buyer\r\n\
EMAIL;TYPE=INTERNET,WORK,pref:Jane.Doe@Acme.com\r\nitem1.EMAIL;TYPE=INTERNET:jane@home.example\r\nTEL;TYPE=WORK,VOICE:+1 415 555 0123\r\n\
TEL;TYPE=CELL:+1 415 555 0199\r\nADR;TYPE=WORK:;Suite 200;123 Main St;Springfield;IL;62701;USA\r\nURL:https://acme.com\r\n\
BDAY:19900520\r\nNOTE:Prefers email\\nCall after 2pm\r\nEND:VCARD\r\n\
BEGIN:VCARD\r\nVERSION:2.1\r\nN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:M=C3=BCller;J=C3=B6rg\r\nTEL;WORK;VOICE:030 1234567\r\n\
NOTE;ENCODING=QUOTED-PRINTABLE:Line one=0D=0A=\r\nLine two\r\nEND:VCARD\r\n\
BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Sam Lee\r\nTEL;VALUE=uri;TYPE=\"voice,home\":tel:+44-20-7946-0958\r\nEND:VCARD\r\n";

    #[test]
    fn three_versions() {
        let out = run(serde_json::to_vec(&json!({ "vcf": CARDS })).unwrap().as_slice());
        assert_eq!(out["contact_count"], json!(3));
        let a = &out["contacts"][0];
        assert_eq!(a["full_name"], json!("Dr. Jane Ann Doe, PhD"));
        assert_eq!(a["name"], json!({ "family": "Doe", "given": "Jane", "additional": "Ann", "prefix": "Dr.", "suffix": "PhD" }));
        assert_eq!(a["organization"], json!("Acme Corp"));
        assert_eq!(a["department"], json!("Procurement"));
        assert_eq!(a["emails"], json!([{ "value": "jane.doe@acme.com", "types": ["work", "pref"] }, { "value": "jane@home.example", "types": [] }]));
        assert_eq!(a["phones"][1], json!({ "value": "+1 415 555 0199", "types": ["cell"] }));
        assert_eq!(a["addresses"][0]["street"], json!("123 Main St"));
        assert_eq!(a["addresses"][0]["extended"], json!("Suite 200"));
        assert_eq!(a["addresses"][0]["po_box"], Value::Null);
        assert_eq!(a["birthday"], json!("1990-05-20"));
        assert_eq!(a["note"], json!("Prefers email\nCall after 2pm"));
        let b = &out["contacts"][1];
        assert_eq!(b["name"]["family"], json!("Müller"));
        assert_eq!(b["full_name"], json!("Jörg Müller"));
        assert_eq!(b["phones"][0], json!({ "value": "030 1234567", "types": ["work", "voice"] }));
        assert_eq!(b["note"], json!("Line one\r\nLine two"));
        let c = &out["contacts"][2];
        assert_eq!(c["phones"][0], json!({ "value": "+44-20-7946-0958", "types": ["voice", "home"] }));
    }

    #[test]
    fn errors() {
        assert_eq!(run(br#"{"vcf": "hello"}"#)["ok"], json!(false));
        assert_eq!(run(br#"{"other": 1}"#)["ok"], json!(false));
    }
}
