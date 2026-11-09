//! Neblex sandboxed module: pull labeled values ("Invoice No: 4411", "Total Due $1,250.00") and
//! simple aligned tables out of document text, such as text from pdf-to-json or an email body.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the options.

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

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Number,
    Amount,
    Date,
}

struct Field {
    name: String,
    labels: Vec<String>,
    kind: Kind,
}

fn fields(v: Option<&Value>) -> Result<Vec<Field>, String> {
    let m = v.and_then(Value::as_object).ok_or("_options.fields is required: { \"name\": [\"label\", ...] } or { \"name\": { \"labels\": [...], \"type\": \"amount\" } }")?;
    let mut out = Vec::new();
    for (name, spec) in m {
        let (labels, kind) = match spec {
            Value::Array(a) => (a.clone(), "text"),
            Value::String(s) => (vec![json!(s)], "text"),
            Value::Object(o) => (
                o.get("labels").and_then(Value::as_array).cloned().unwrap_or_default(),
                o.get("type").and_then(Value::as_str).unwrap_or("text"),
            ),
            _ => return Err(format!("field '{name}': expected a list of labels or an object")),
        };
        let labels: Vec<String> = labels.iter().filter_map(|l| l.as_str()).map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect();
        if labels.is_empty() {
            return Err(format!("field '{name}': no labels"));
        }
        let kind = match kind {
            "number" => Kind::Number,
            "amount" => Kind::Amount,
            "date" => Kind::Date,
            "text" => Kind::Text,
            other => return Err(format!("field '{name}': unknown type '{other}'")),
        };
        out.push(Field { name: name.clone(), labels, kind });
    }
    Ok(out)
}

/// Reads `_options.field` (default `text`) and adds `fields` (name -> value), `found` (where each
/// value came from), `missing` and, with `"tables": true`, `tables`.
fn extract_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("text").to_owned();
    let date_order = opt.get("date_order").and_then(Value::as_str).unwrap_or("").to_owned();
    let Some(text) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    let specs = if opt.get("fields").is_some() { fields(opt.get("fields")) } else { Ok(Vec::new()) };
    let specs = match specs {
        Ok(s) => s,
        Err(e) => {
            m.insert("ok".into(), json!(false));
            m.insert("error".into(), json!(e));
            return Value::Object(m);
        }
    };
    if !opt.get("keep_source").and_then(Value::as_bool).unwrap_or(false) {
        m.remove(&field);
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut values = Map::new();
    let mut found = Map::new();
    let mut missing = Vec::new();
    for f in &specs {
        match find_value(&lines, &f.labels) {
            Some((line, label, raw)) => {
                let v = typed(&raw, f.kind, &date_order);
                found.insert(f.name.clone(), json!({ "label": label, "line": line + 1, "raw": raw }));
                if v.is_null() {
                    missing.push(f.name.clone());
                }
                values.insert(f.name.clone(), v);
            }
            None => {
                values.insert(f.name.clone(), Value::Null);
                missing.push(f.name.clone());
            }
        }
    }
    m.insert("fields".into(), Value::Object(values));
    m.insert("found".into(), Value::Object(found));
    m.insert("missing".into(), json!(missing));
    if opt.get("tables").and_then(Value::as_bool).unwrap_or(false) {
        m.insert("tables".into(), Value::Array(tables(&lines)));
    }
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

/// The first line containing one of the labels (checked in order, longest first, on word
/// boundaries), and the value after it: the rest of that line up to a gap of 2+ spaces or a tab,
/// or, when nothing follows the label, the next non-empty line.
fn find_value(lines: &[&str], labels: &[String]) -> Option<(usize, String, String)> {
    let mut sorted: Vec<&String> = labels.iter().collect();
    sorted.sort_by_key(|l| std::cmp::Reverse(l.chars().count()));
    for (n, line) in lines.iter().enumerate() {
        let lower = line.to_lowercase();
        if lower.len() != line.len() {
            // Lowercasing changed byte lengths (rare non-ASCII); fall back to an ASCII match.
            if let Some(r) = match_line(line, &line.to_ascii_lowercase(), &sorted, lines, n) {
                return Some(r);
            }
            continue;
        }
        if let Some(r) = match_line(line, &lower, &sorted, lines, n) {
            return Some(r);
        }
    }
    None
}

fn match_line(line: &str, lower: &str, labels: &[&String], lines: &[&str], n: usize) -> Option<(usize, String, String)> {
    for label in labels {
        let mut from = 0;
        while let Some(p) = lower[from..].find(label.as_str()).map(|p| p + from) {
            let end = p + label.len();
            let before_ok = p == 0 || !lower[..p].chars().next_back().is_some_and(char::is_alphanumeric);
            let after_ok = end >= lower.len() || !lower[end..].chars().next().is_some_and(char::is_alphanumeric) || label.ends_with(|c: char| !c.is_alphanumeric());
            if before_ok && after_ok {
                let rest = strip_separators(&line[end..]);
                let value = first_cell(rest);
                if !value.is_empty() {
                    return Some((n, (*label).clone(), value));
                }
                if rest.trim().is_empty() {
                    if let Some((k, next)) = lines.iter().enumerate().skip(n + 1).find(|(_, l)| !l.trim().is_empty()) {
                        return Some((k, (*label).clone(), first_cell(next.trim())));
                    }
                }
            }
            from = end;
        }
    }
    None
}

/// Drops what usually sits between a label and its value: spaces, ":", "#", ".", "-", "=" and
/// the words "no", "nr", "number".
fn strip_separators(s: &str) -> &str {
    let mut t = s;
    loop {
        let before = t;
        t = t.trim_start_matches([' ', '\t', ':', '#', '.', '-', '=', '\u{a0}']);
        let low = t.to_ascii_lowercase();
        for w in ["number", "no", "nr"] {
            if low.starts_with(w) && !low[w.len()..].chars().next().is_some_and(char::is_alphanumeric) {
                t = &t[w.len()..];
                break;
            }
        }
        if t == before {
            return t;
        }
    }
}

/// Text up to a gap of two or more spaces or a tab (the next column in aligned layouts).
fn first_cell(s: &str) -> String {
    let s = s.trim();
    let end = s.find("  ").into_iter().chain(s.find('\t')).min().unwrap_or(s.len());
    s[..end].trim().to_owned()
}

fn typed(raw: &str, kind: Kind, date_order: &str) -> Value {
    match kind {
        Kind::Text => json!(raw),
        Kind::Number => number(raw).map(|n| json!(n)).unwrap_or(Value::Null),
        Kind::Amount => match number(raw) {
            Some(n) => match currency(raw) {
                Some(c) => json!({ "amount": n, "currency": c }),
                None => json!({ "amount": n, "currency": null }),
            },
            None => Value::Null,
        },
        Kind::Date => date(raw, date_order).map(|d| json!(d)).unwrap_or(Value::Null),
    }
}

/// The first number in the text: "$1,250.00" -> 1250.0, "1.234,56 EUR" -> 1234.56, "(500)" -> -500.0.
/// The last separator is the decimal mark when 1 or 2 digits follow it.
pub fn number(raw: &str) -> Option<f64> {
    let chars: Vec<char> = raw.chars().collect();
    let start = chars.iter().position(|c| c.is_ascii_digit())?;
    let mut end = start;
    while end < chars.len() && (chars[end].is_ascii_digit() || ((matches!(chars[end], ',' | '.' | ' ' | '\'')) && chars.get(end + 1).is_some_and(char::is_ascii_digit))) {
        end += 1;
    }
    let t: String = chars[start..end].iter().filter(|c| !matches!(c, ' ' | '\'')).collect();
    let neg = chars[..start].iter().rev().take(3).any(|c| *c == '-' || *c == '(') || chars.get(end) == Some(&'-');
    let last = t.rfind([',', '.']);
    let (int, frac) = match last {
        Some(p) if (1..=2).contains(&(t.len() - p - 1)) => (&t[..p], &t[p + 1..]),
        _ => (&t[..], ""),
    };
    let int: String = int.chars().filter(char::is_ascii_digit).collect();
    let v: f64 = format!("{}.{}", if int.is_empty() { "0" } else { &int }, if frac.is_empty() { "0" } else { frac }).parse().ok()?;
    Some(if neg { -v } else { v })
}

fn currency(raw: &str) -> Option<&'static str> {
    let up = raw.to_uppercase();
    for code in ["USD", "EUR", "GBP", "CAD", "AUD", "CHF", "JPY", "MXN", "BRL", "INR", "SEK", "NOK", "DKK", "PLN"] {
        if up.split(|c: char| !c.is_ascii_alphabetic()).any(|w| w == code) {
            return Some(code);
        }
    }
    for (sym, code) in [("€", "EUR"), ("£", "GBP"), ("¥", "JPY"), ("₹", "INR"), ("$", "USD")] {
        if raw.contains(sym) {
            return Some(code);
        }
    }
    None
}

/// ISO dates, month-name dates and numeric dates (with `date_order` "mdy" or "dmy" when ambiguous).
pub fn date(raw: &str, order: &str) -> Option<String> {
    let valid = |y: u32, m: u32, d: u32| {
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        (1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1]
    };
    let fmt = |y: u32, m: u32, d: u32| valid(y, m, d).then(|| format!("{y:04}-{m:02}-{d:02}"));
    let tokens: Vec<String> = raw.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(str::to_lowercase).collect();
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let month = |t: &str| (t.len() >= 3 && t.chars().all(|c| c.is_ascii_alphabetic())).then(|| MONTHS.iter().position(|m| t.starts_with(m))).flatten().map(|i| i as u32 + 1);
    let num = |t: &str| t.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<u32>().ok().filter(|_| t.starts_with(|c: char| c.is_ascii_digit()));
    for w in tokens.windows(3) {
        let (a, b, c) = (&w[0], &w[1], &w[2]);
        if a.len() == 4 && a.chars().all(|c| c.is_ascii_digit()) {
            if let (Some(y), Some(m), Some(d)) = (num(a), num(b), num(c)) {
                return fmt(y, m, d);
            }
        }
        if let (Some(m), Some(d), Some(y)) = (month(a), num(b), num(c)) {
            if c.len() == 4 {
                return fmt(y, m, d);
            }
        }
        if let (Some(d), Some(m), Some(y)) = (num(a), month(b), num(c)) {
            if c.len() == 4 {
                return fmt(y, m, d);
            }
        }
        if let (Some(x), Some(z), Some(y)) = (num(a), num(b), num(c)) {
            if c.len() == 4 && a.len() <= 2 && b.len() <= 2 {
                let (md, dm) = (valid(y, x, z), valid(y, z, x));
                return match (md, dm, order) {
                    (true, false, _) | (true, true, "mdy") => fmt(y, x, z),
                    (false, true, _) | (true, true, "dmy") => fmt(y, z, x),
                    (true, true, _) if x == z => fmt(y, x, z),
                    _ => None,
                };
            }
        }
    }
    None
}

/// Aligned text tables: a header line followed by at least 2 rows with the same columns, separated
/// by 2+ spaces or tabs. Each row is matched to the header by position; a missing last cell is null.
/// Lines of "Label: value" pairs are not tables.
fn tables(lines: &[&str]) -> Vec<Value> {
    let cells = |l: &str| -> Vec<String> {
        l.split('\t').flat_map(|p| p.split("  ")).map(str::trim).filter(|c| !c.is_empty()).map(str::to_owned).collect()
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let head = cells(lines[i]);
        // A header is mostly words: not numbers, and not "Label: value" pairs.
        let labelled = |c: &Vec<String>| c.iter().any(|x| x.contains(": ") || x.ends_with(':'));
        if head.len() < 2 || labelled(&head) || head.iter().any(|h| h.chars().filter(char::is_ascii_digit).count() > h.len() / 2) {
            i += 1;
            continue;
        }
        let mut rows = Vec::new();
        let mut j = i + 1;
        while j < lines.len() {
            let c = cells(lines[j]);
            // Rows have the header's columns (the last may be empty).
            if c.len() + 1 < head.len() || c.len() > head.len() || labelled(&c) {
                break;
            }
            let mut row = Map::new();
            for (k, h) in head.iter().enumerate() {
                row.insert(h.clone(), c.get(k).map(|v| json!(v)).unwrap_or(Value::Null));
            }
            rows.push(Value::Object(row));
            j += 1;
        }
        if rows.len() >= 2 {
            out.push(json!({ "line": i + 1, "columns": head, "rows": rows }));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const INVOICE: &str = "ACME SUPPLY CO.                      INVOICE\n\
Invoice No: INV-2026-0091            Invoice Date: Nov 3, 2026\n\
PO Number: 45-1180                   Due Date: 03/12/2026\n\
Bill To:\n\
Globex Corporation\n\
\n\
Item          Description          Qty    Unit Price    Amount\n\
WID-9         Widget, blue           2        500.00   1,000.00\n\
BLT-2         Bolt pack              5         50.00     250.00\n\
\n\
Subtotal:   1,250.00\n\
Total Due:  USD 1,250.00\n";

    fn opts() -> Value {
        json!({
            "fields": {
                "invoice_number": ["invoice no", "invoice number", "invoice #"],
                "invoice_date": { "labels": ["invoice date"], "type": "date" },
                "due_date": { "labels": ["due date"], "type": "date" },
                "po_number": ["po number", "purchase order", "po"],
                "bill_to": ["bill to"],
                "total": { "labels": ["total due", "amount due", "total"], "type": "amount" },
                "tax": { "labels": ["tax", "vat"], "type": "amount" }
            },
            "date_order": "dmy",
            "tables": true
        })
    }

    #[test]
    fn invoice_fields() {
        let out = run(serde_json::to_vec(&json!({ "text": INVOICE, "_options": opts() })).unwrap().as_slice());
        let f = &out["fields"];
        assert_eq!(f["invoice_number"], json!("INV-2026-0091"));
        assert_eq!(f["invoice_date"], json!("2026-11-03"));
        assert_eq!(f["due_date"], json!("2026-12-03"));
        assert_eq!(f["po_number"], json!("45-1180"));
        assert_eq!(f["bill_to"], json!("Globex Corporation"));
        assert_eq!(f["total"], json!({ "amount": 1250.0, "currency": "USD" }));
        assert_eq!(f["tax"], Value::Null);
        assert_eq!(out["missing"], json!(["tax"]));
        assert_eq!(out["found"]["total"]["line"], json!(12));
    }

    #[test]
    fn aligned_table() {
        let out = run(serde_json::to_vec(&json!({ "text": INVOICE, "_options": opts() })).unwrap().as_slice());
        let t = &out["tables"][0];
        assert_eq!(t["columns"], json!(["Item", "Description", "Qty", "Unit Price", "Amount"]));
        assert_eq!(t["rows"][1], json!({ "Item": "BLT-2", "Description": "Bolt pack", "Qty": "5", "Unit Price": "50.00", "Amount": "250.00" }));
        assert_eq!(out["tables"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn numbers_dates_and_labels() {
        assert_eq!(number("$1,250.00"), Some(1250.0));
        assert_eq!(number("1.234,56 EUR"), Some(1234.56));
        assert_eq!(number("(500.00)"), Some(-500.0));
        assert_eq!(number("1,234"), Some(1234.0));
        assert_eq!(date("2026-11-03", ""), Some("2026-11-03".into()));
        assert_eq!(date("3 March 2026", ""), Some("2026-03-03".into()));
        assert_eq!(date("04/03/2026", ""), None);
        assert_eq!(date("25/03/2026", ""), Some("2026-03-25".into()));
        // "total" must not match inside "subtotal", and "po" not inside "report".
        let out = run(br#"{"text": "Report 7\nSubtotal: 10.00\nTotal: 12.00", "_options": {"fields": {"total": {"labels": ["total"], "type": "number"}, "po": ["po"]}}}"#);
        assert_eq!(out["fields"]["total"], json!(12.0));
        assert_eq!(out["fields"]["po"], Value::Null);
    }

    #[test]
    fn errors() {
        assert_eq!(run(br#"{"text": "x", "_options": {"fields": {"a": {"labels": ["a"], "type": "money"}}}}"#)["ok"], json!(false));
        assert_eq!(run(br#"{"body": "x"}"#)["ok"], json!(false));
    }
}
