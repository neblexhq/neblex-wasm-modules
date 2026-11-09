//! Neblex sandboxed module: read messy real-world CSV exports into clean records.
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(clean_record).collect()),
        Ok(v) => clean_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

struct Options {
    field: String,
    delimiter: Option<char>,
    header_row: Option<usize>,
    no_header: bool,
    numbers: bool,
    decimal: Option<char>,
    snake_case: bool,
    drop_totals: bool,
    keep_source: bool,
}

fn options(v: &Value) -> Options {
    let ch = |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| if s == "tab" || s == "\\t" { Some('\t') } else { s.chars().next() });
    Options {
        field: v.get("field").and_then(Value::as_str).unwrap_or("csv").to_owned(),
        delimiter: ch("delimiter"),
        header_row: v.get("header_row").and_then(Value::as_u64).map(|n| n.max(1) as usize),
        no_header: v.get("header").and_then(Value::as_bool) == Some(false),
        numbers: v.get("numbers").and_then(Value::as_bool).unwrap_or(false),
        decimal: ch("decimal"),
        snake_case: v.get("snake_case_headers").and_then(Value::as_bool).unwrap_or(false),
        drop_totals: v.get("drop_totals").and_then(Value::as_bool).unwrap_or(true),
        keep_source: v.get("keep_source").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Reads the CSV text in `_options.field` (default `csv`) and adds `records`, `columns`,
/// `delimiter`, `skipped` (lines dropped and why), `errors` and `_valid`.
fn clean_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = options(&m.remove("_options").unwrap_or(Value::Null));
    let Some(text) = m.get(&opt.field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{}' is missing or not text", opt.field)));
        return Value::Object(m);
    };
    if !opt.keep_source {
        m.remove(&opt.field);
    }
    let text = text.trim_start_matches('\u{feff}');
    let delim = opt.delimiter.unwrap_or_else(|| sniff(text));
    let rows = parse(text, delim);
    // Each row keeps its 1-based line number in the file for error reports.
    let rows: Vec<(usize, Vec<String>)> = rows.into_iter().filter(|(_, r)| r.iter().any(|c| !c.trim().is_empty())).collect();
    if rows.is_empty() {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!("no rows found"));
        return Value::Object(m);
    }
    let width = modal_width(&rows);
    let mut skipped = Vec::new();
    // The header is the first row as wide as most rows (title and "generated on" lines before it are skipped).
    let header_idx = match opt.header_row {
        Some(n) => rows.iter().position(|(line, _)| *line >= n).unwrap_or(0),
        None => rows.iter().position(|(_, r)| r.len() == width).unwrap_or(0),
    };
    for (line, r) in &rows[..header_idx] {
        skipped.push(json!({ "line": line, "reason": "before the header", "text": r.join(&delim.to_string()) }));
    }
    let (columns, body_start) = if opt.no_header {
        ((1..=width).map(|i| format!("column_{i}")).collect::<Vec<_>>(), header_idx)
    } else {
        (headers(&rows[header_idx].1, width, opt.snake_case), header_idx + 1)
    };
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let mut body: Vec<(usize, Vec<String>)> = rows[body_start..].to_vec();
    if opt.drop_totals {
        while let Some((line, r)) = body.last() {
            if is_total(r) {
                skipped.push(json!({ "line": line, "reason": "totals row", "text": r.join(&delim.to_string()) }));
                body.pop();
            } else {
                break;
            }
        }
    }
    let decimal = opt.decimal.unwrap_or_else(|| decimal_style(&body, delim));
    // A column becomes numbers only when every non-empty value in it is a number, so a ZIP
    // column with one leading-zero code stays text throughout.
    let numeric: Vec<bool> = (0..columns.len())
        .map(|i| {
            opt.numbers && body.iter().filter_map(|(_, r)| r.get(i).map(|c| c.trim())).filter(|c| !c.is_empty()).all(|c| number(c, decimal).is_some())
        })
        .collect();
    for (line, r) in &body {
        if r.len() != columns.len() {
            errors.push(json!({ "line": line, "error": format!("{} fields, expected {}", r.len(), columns.len()) }));
        }
        let mut obj = Map::new();
        obj.insert("_line".into(), json!(line));
        for (i, col) in columns.iter().enumerate() {
            let raw = r.get(i).map(|s| s.trim()).unwrap_or("");
            obj.insert(col.clone(), value(raw, numeric[i], decimal));
        }
        records.push(Value::Object(obj));
    }
    m.insert("columns".into(), json!(columns));
    m.insert("delimiter".into(), json!(if delim == '\t' { "tab".to_owned() } else { delim.to_string() }));
    m.insert("decimal".into(), json!(decimal.to_string()));
    m.insert("record_count".into(), json!(records.len()));
    m.insert("records".into(), Value::Array(records));
    m.insert("skipped".into(), Value::Array(skipped));
    m.insert("_valid".into(), json!(errors.is_empty()));
    m.insert("errors".into(), Value::Array(errors));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

/// Picks the delimiter that splits the most lines into the same number of fields (more than one).
fn sniff(text: &str) -> char {
    let mut best = (',', 0usize);
    for d in [',', ';', '\t', '|'] {
        let rows = parse(text, d);
        let widths: Vec<usize> = rows.iter().map(|(_, r)| r.len()).filter(|w| *w > 1).collect();
        let Some(w) = mode(&widths) else { continue };
        let score = widths.iter().filter(|x| **x == w).count();
        if score > best.1 {
            best = (d, score);
        }
    }
    best.0
}

fn mode(v: &[usize]) -> Option<usize> {
    let mut counts: Vec<(usize, usize)> = Vec::new();
    for x in v {
        match counts.iter_mut().find(|(k, _)| k == x) {
            Some(c) => c.1 += 1,
            None => counts.push((*x, 1)),
        }
    }
    counts.into_iter().max_by_key(|(k, c)| (*c, *k)).map(|(k, _)| k)
}

fn modal_width(rows: &[(usize, Vec<String>)]) -> usize {
    let widths: Vec<usize> = rows.iter().map(|(_, r)| r.len()).collect();
    mode(&widths).unwrap_or(1)
}

/// RFC 4180 parsing: quoted fields may hold the delimiter, line breaks and doubled quotes.
fn parse(text: &str, d: char) -> Vec<(usize, Vec<String>)> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut line = 1;
    let mut row_line = 1;
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if quoted {
            match c {
                '"' if it.peek() == Some(&'"') => {
                    cell.push('"');
                    it.next();
                }
                '"' => quoted = false,
                '\n' => {
                    line += 1;
                    cell.push('\n');
                }
                _ => cell.push(c),
            }
            continue;
        }
        match c {
            '"' if cell.trim().is_empty() => {
                cell.clear();
                quoted = true;
            }
            c if c == d => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                rows.push((row_line, std::mem::take(&mut row)));
                line += 1;
                row_line = line;
            }
            _ => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push((row_line, row));
    }
    rows
}

fn headers(raw: &[String], width: usize, snake: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in 0..width.max(raw.len()) {
        let mut h = raw.get(i).map(|s| s.trim().to_owned()).unwrap_or_default();
        if snake {
            h = snake_case(&h);
        }
        if h.is_empty() {
            h = format!("column_{}", i + 1);
        }
        let base = h.clone();
        let mut n = 2;
        while out.contains(&h) {
            h = format!("{base}_{n}");
            n += 1;
        }
        out.push(h);
    }
    out
}

fn snake_case(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_owned()
}

fn is_total(r: &[String]) -> bool {
    let first = r.iter().find(|c| !c.trim().is_empty()).map(|c| c.trim().to_lowercase()).unwrap_or_default();
    ["total", "totals", "grand total", "sum", "summe", "gesamt", "totaal", "totale", "subtotal"].iter().any(|t| first == *t || first.starts_with(&format!("{t}:")) || first.starts_with(&format!("{t} ")))
}

/// Decides whether numbers use a decimal comma: any value like "1.234,56" or "99,90" (with a
/// non-comma delimiter) means comma; any "1,234.56" or "12.50" means point. Point wins a tie.
fn decimal_style(body: &[(usize, Vec<String>)], delim: char) -> char {
    let (mut comma, mut point) = (0, 0);
    for (_, r) in body {
        for c in r {
            let t = c.trim().trim_start_matches(['-', '+', '$', '€', '£']).trim_end_matches(['%', '€']);
            if t.is_empty() || !t.chars().all(|ch| ch.is_ascii_digit() || ch == '.' || ch == ',' || ch == ' ') {
                continue;
            }
            match (t.rfind(','), t.rfind('.')) {
                (Some(c), Some(p)) if c > p => comma += 1,
                (Some(_), Some(_)) => point += 1,
                (Some(c), None) if delim != ',' && t.len() - c - 1 != 3 => comma += 1,
                (None, Some(p)) if t.len() - p - 1 != 3 => point += 1,
                _ => {}
            }
        }
    }
    if comma > point {
        ','
    } else {
        '.'
    }
}

fn value(raw: &str, numbers: bool, decimal: char) -> Value {
    if raw.is_empty() {
        return Value::Null;
    }
    if numbers {
        if let Some(n) = number(raw, decimal) {
            return n;
        }
    }
    json!(raw)
}

/// Numbers in the file's style; values with leading zeros (IDs, ZIP codes) stay text.
fn number(raw: &str, decimal: char) -> Option<Value> {
    let neg = raw.starts_with('-') || (raw.starts_with('(') && raw.ends_with(')'));
    let t = raw.trim_matches(['-', '+', '(', ')', ' ']).trim_start_matches(['$', '€', '£']).trim_end_matches(['€']).trim();
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',' || c == ' ' || c == '\'') {
        return None;
    }
    let group = if decimal == ',' { '.' } else { ',' };
    let cleaned: String = t.chars().filter(|c| *c != group && *c != ' ' && *c != '\'').map(|c| if c == decimal { '.' } else { c }).collect();
    if cleaned.matches('.').count() > 1 || (cleaned.len() > 1 && cleaned.starts_with('0') && !cleaned.starts_with("0.")) {
        return None;
    }
    if let Ok(i) = cleaned.parse::<i64>() {
        return Some(json!(if neg { -i } else { i }));
    }
    cleaned.parse::<f64>().ok().filter(|f| f.is_finite()).map(|f| json!(if neg { -f } else { f }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    #[test]
    fn european_semicolon_export_with_bom() {
        let out = go(json!({ "csv": "\u{feff}Kundennr;Name;Betrag\n1001;Müller GmbH;1.234,56\n1002;\"Schmidt; Söhne\";99,90\n", "_options": { "numbers": true } }));
        assert_eq!(out["delimiter"], json!(";"));
        assert_eq!(out["decimal"], json!(","));
        assert_eq!(out["columns"], json!(["Kundennr", "Name", "Betrag"]));
        assert_eq!(out["records"][1], json!({ "_line": 3, "Kundennr": 1002, "Name": "Schmidt; Söhne", "Betrag": 99.9 }));
        assert_eq!(out["records"][0]["Betrag"], json!(1234.56));
        assert_eq!(out["_valid"], json!(true));
    }

    #[test]
    fn title_lines_and_totals_row_are_skipped() {
        let out = go(json!({ "csv": "Customer export\nGenerated 2026-11-09 by ERP\n\nid,name,amount\n1,Acme,10.00\n2,Globex,20.00\nTotal,,30.00\n", "_options": { "numbers": true } }));
        assert_eq!(out["columns"], json!(["id", "name", "amount"]));
        assert_eq!(out["record_count"], json!(2));
        assert_eq!(out["records"][1], json!({ "_line": 6, "id": 2, "name": "Globex", "amount": 20.0 }));
        let reasons: Vec<_> = out["skipped"].as_array().unwrap().iter().map(|s| s["reason"].clone()).collect();
        assert_eq!(reasons, vec![json!("before the header"), json!("before the header"), json!("totals row")]);
    }

    #[test]
    fn quoting_ids_and_header_cleanup() {
        let csv = "Order ID\tZIP\tNotes\tNotes\t\n00042\t02134\t\"line one\nline two\"\tx\t\n00043\t94107\t\"he said \"\"hi\"\"\"\ty\tz\n";
        let out = go(json!({ "csv": csv, "_options": { "numbers": true, "snake_case_headers": true } }));
        assert_eq!(out["delimiter"], json!("tab"));
        assert_eq!(out["columns"], json!(["order_id", "zip", "notes", "notes_2", "column_5"]));
        assert_eq!(out["records"][0]["order_id"], json!("00042"));
        assert_eq!(out["records"][0]["notes"], json!("line one\nline two"));
        assert_eq!(out["records"][1]["notes"], json!("he said \"hi\""));
        assert_eq!(out["records"][1]["_line"], json!(4));
        // One leading-zero value keeps the whole column as text.
        assert_eq!(out["records"][1]["zip"], json!("94107"));
    }

    #[test]
    fn ragged_rows_are_reported() {
        let out = go(json!({ "csv": "a,b,c\n1,2,3\n4,5\n6,7,8,9\n" }));
        assert_eq!(out["_valid"], json!(false));
        assert_eq!(out["errors"].as_array().unwrap().len(), 2);
        assert_eq!(out["records"][1], json!({ "_line": 3, "a": "4", "b": "5", "c": null }));
    }

    #[test]
    fn options_and_errors() {
        let out = go(json!({ "data": "1|2\n3|4", "_options": { "field": "data", "header": false } }));
        assert_eq!(out["columns"], json!(["column_1", "column_2"]));
        assert_eq!(out["record_count"], json!(2));
        assert_eq!(go(json!({ "csv": "\n\n" }))["ok"], json!(false));
        assert_eq!(number("(1,250.00)", '.'), Some(json!(-1250.0)));
    }
}
