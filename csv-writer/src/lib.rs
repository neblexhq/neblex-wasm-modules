//! Neblex sandboxed module: write records as CSV with control over the delimiter, quoting,
//! decimal separator, header, line endings and byte-order mark, and protection against
//! spreadsheet formula injection.
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(write_record).collect()),
        Ok(v) => write_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

#[derive(Clone)]
struct Options {
    delimiter: char,
    quote_all: bool,
    decimal: char,
    eol: String,
    header: bool,
    bom: bool,
    null_value: String,
    escape_formulas: bool,
}

/// A column: the field to read (a dotted path for nested objects) and its header text.
struct Column {
    path: String,
    header: String,
}

fn columns(spec: Option<&Value>, rows: &[Value]) -> Vec<Column> {
    match spec.and_then(Value::as_array) {
        Some(list) => list
            .iter()
            .filter_map(|c| match c {
                Value::String(s) => Some(Column { path: s.clone(), header: s.clone() }),
                Value::Object(o) => {
                    let path = o.get("field").and_then(Value::as_str)?.to_owned();
                    let header = o.get("header").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| path.clone());
                    Some(Column { path, header })
                }
                _ => None,
            })
            .collect(),
        None => {
            // Every top-level field, alphabetically: JSON objects don't keep key order between flow
            // steps, so an explicit `columns` list is the way to set one.
            let mut out: Vec<Column> = Vec::new();
            for r in rows {
                if let Some(o) = r.as_object() {
                    for k in o.keys() {
                        if !out.iter().any(|c| &c.path == k) {
                            out.push(Column { path: k.clone(), header: k.clone() });
                        }
                    }
                }
            }
            out
        }
    }
}

fn lookup<'a>(row: &'a Value, path: &str) -> Option<&'a Value> {
    if let Some(v) = row.get(path) {
        return Some(v);
    }
    path.split('.').try_fold(row, |cur, k| cur.get(k))
}

fn cell(v: Option<&Value>, o: &Options) -> String {
    let text = match v {
        None | Some(Value::Null) => return o.null_value.clone(),
        Some(Value::String(s)) => {
            // Text a spreadsheet would run as a formula gets a leading apostrophe (OWASP advice).
            let numeric = s.trim().parse::<f64>().is_ok();
            if o.escape_formulas && !numeric && s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
                format!("'{s}")
            } else {
                s.clone()
            }
        }
        Some(Value::Number(n)) => {
            let s = n.to_string();
            if o.decimal == ',' { s.replace('.', ",") } else { s }
        }
        Some(Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    };
    let needs_quotes = o.quote_all || text.contains(o.delimiter) || text.contains('"') || text.contains('\n') || text.contains('\r') || text.starts_with(' ') || text.ends_with(' ');
    if needs_quotes {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

/// Writes the list in `_options.records_field` (default `records`) to `_options.into` (default
/// `csv`), with `row_count` and `columns`. The list is removed unless `keep_source` is set.
fn write_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let s = |k: &str| opt.get(k).and_then(Value::as_str);
    let b = |k: &str, d: bool| opt.get(k).and_then(Value::as_bool).unwrap_or(d);
    let src = s("records_field").unwrap_or("records").to_owned();
    let into = s("into").unwrap_or("csv").to_owned();
    let delimiter = match s("delimiter").unwrap_or(",") {
        "tab" | "\\t" | "\t" => '\t',
        d => d.chars().next().unwrap_or(','),
    };
    let o = Options {
        delimiter,
        quote_all: s("quote") == Some("all"),
        decimal: if s("decimal") == Some(",") { ',' } else { '.' },
        eol: if s("line_ending") == Some("\n") { "\n".into() } else { "\r\n".into() },
        header: b("header", true),
        bom: b("bom", false),
        null_value: s("null_value").unwrap_or("").to_owned(),
        escape_formulas: b("escape_formulas", true),
    };
    let fail = |mut m: Map<String, Value>, e: String| {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(e));
        Value::Object(m)
    };
    if o.decimal == ',' && o.delimiter == ',' {
        return fail(m, "a decimal comma needs another delimiter, such as \";\"".into());
    }
    let Some(rows) = m.get(&src).and_then(Value::as_array).cloned() else {
        return fail(m, format!("field '{src}' is missing or not a list"));
    };
    let cols = columns(opt.get("columns"), &rows);
    let mut out = String::new();
    if o.bom {
        out.push('\u{feff}');
    }
    let join = |cells: Vec<String>| cells.join(&o.delimiter.to_string());
    if o.header {
        out.push_str(&join(cols.iter().map(|c| cell(Some(&json!(c.header)), &Options { escape_formulas: false, ..o.clone() })).collect()));
        out.push_str(&o.eol);
    }
    for r in &rows {
        out.push_str(&join(cols.iter().map(|c| cell(lookup(r, &c.path), &o)).collect()));
        out.push_str(&o.eol);
    }
    if !b("keep_source", false) {
        m.remove(&src);
    }
    m.insert(into, json!(out));
    m.insert("row_count".into(), json!(rows.len()));
    m.insert("columns".into(), json!(cols.iter().map(|c| c.header.clone()).collect::<Vec<_>>()));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    #[test]
    fn defaults() {
        let out = go(json!({ "records": [
            { "id": 1, "name": "Acme, Inc.", "amount": 1234.5, "active": true },
            { "id": 2, "name": "Say \"hi\"", "note": "line\nbreak", "amount": null }
        ]}));
        assert_eq!(out["csv"], json!("active,amount,id,name,note\r\ntrue,1234.5,1,\"Acme, Inc.\",\r\n,,2,\"Say \"\"hi\"\"\",\"line\nbreak\"\r\n"));
        assert_eq!(out["row_count"], json!(2));
        assert!(out.get("records").is_none());
    }

    #[test]
    fn european_layout_columns_and_bom() {
        let out = go(json!({
            "rows": [ { "sku": "A-1", "price": 9.5, "customer": { "name": "Müller GmbH" } } ],
            "_options": { "records_field": "rows", "into": "file", "delimiter": ";", "decimal": ",", "bom": true, "line_ending": "\n",
                          "columns": [ { "field": "sku", "header": "Artikel" }, { "field": "customer.name", "header": "Kunde" }, "price" ] }
        }));
        assert_eq!(out["file"], json!("\u{feff}Artikel;Kunde;price\nA-1;Müller GmbH;9,5\n"));
        assert_eq!(out["columns"], json!(["Artikel", "Kunde", "price"]));
    }

    #[test]
    fn formula_injection_and_errors() {
        let out = go(json!({ "records": [ { "a": "=HYPERLINK(\"http://x\")", "b": "-42", "c": "@SUM(A1)", "d": "+1 415 555 0123" } ], "_options": { "header": false } }));
        assert_eq!(out["csv"], json!("\"'=HYPERLINK(\"\"http://x\"\")\",-42,'@SUM(A1),'+1 415 555 0123\r\n"));
        let raw = go(json!({ "records": [ { "a": "=1+1" } ], "_options": { "header": false, "escape_formulas": false, "quote": "all" } }));
        assert_eq!(raw["csv"], json!("\"=1+1\"\r\n"));
        assert_eq!(go(json!({ "records": [], "_options": { "decimal": "," } }))["ok"], json!(false));
        assert_eq!(go(json!({ "x": 1 }))["ok"], json!(false));
    }
}
