//! Neblex sandboxed module: write records as a fixed-width (positional) text file.
//! Uses the same `_layout` as fixed-width-parser, so one layout reads and writes a format.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the layout format.

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

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Integer,
    Decimal,
    Zoned,
    Date,
}

struct Column {
    name: String,
    start: usize,
    length: usize,
    kind: Kind,
    scale: u32,
    date_format: String,
    align_right: bool,
    pad: char,
    constant: Option<String>,
}

struct RecordType {
    name: Option<String>,
    marker: Option<(usize, String)>,
    columns: Vec<Column>,
    width: usize,
}

fn columns(v: Option<&Value>, at: &str) -> Result<Vec<Column>, String> {
    let list = v.and_then(Value::as_array).ok_or(format!("{at}.columns is required"))?;
    let mut out = Vec::new();
    for (i, c) in list.iter().enumerate() {
        let name = c.get("name").and_then(Value::as_str).ok_or(format!("{at}.columns[{i}].name is required"))?;
        let start = c.get("start").and_then(Value::as_u64).ok_or(format!("column '{name}': start is required"))?;
        let length = c.get("length").and_then(Value::as_u64).ok_or(format!("column '{name}': length is required"))?;
        if start == 0 || length == 0 {
            return Err(format!("column '{name}': start is 1-based and length must be at least 1"));
        }
        let kind = match c.get("type").and_then(Value::as_str).unwrap_or("text") {
            "text" | "string" => Kind::Text,
            "integer" => Kind::Integer,
            "decimal" => Kind::Decimal,
            "zoned" => Kind::Zoned,
            "date" => Kind::Date,
            other => return Err(format!("column '{name}': unknown type '{other}'")),
        };
        let numeric = matches!(kind, Kind::Integer | Kind::Decimal | Kind::Zoned);
        let align_right = match c.get("align").and_then(Value::as_str) {
            Some("right") => true,
            Some("left") => false,
            _ => numeric,
        };
        let pad = c.get("pad").and_then(Value::as_str).and_then(|p| p.chars().next()).unwrap_or(if numeric { '0' } else { ' ' });
        out.push(Column {
            name: name.to_owned(),
            start: start as usize - 1,
            length: length as usize,
            kind,
            scale: c.get("scale").and_then(Value::as_u64).unwrap_or(0) as u32,
            date_format: c.get("format").and_then(Value::as_str).unwrap_or("yyyymmdd").to_lowercase(),
            align_right,
            pad,
            constant: c.get("value").map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())),
        });
    }
    Ok(out)
}

fn record_types(layout: &Value) -> Result<Vec<RecordType>, String> {
    let make = |name: Option<String>, marker: Option<(usize, String)>, columns: Vec<Column>, explicit: Option<u64>| {
        let mut width = columns.iter().map(|c| c.start + c.length).max().unwrap_or(0);
        if let Some((s, v)) = &marker {
            width = width.max(s + v.chars().count());
        }
        RecordType { name, marker, columns, width: explicit.map(|w| w as usize).unwrap_or(width) }
    };
    let line_length = layout.get("line_length").and_then(Value::as_u64);
    if let Some(list) = layout.get("record_types").and_then(Value::as_array) {
        let mut out = Vec::new();
        for (i, t) in list.iter().enumerate() {
            let marker = match t.get("when") {
                None => None,
                Some(w) => {
                    let start = w.get("start").and_then(Value::as_u64).ok_or(format!("record_types[{i}].when.start is required"))?;
                    let value = w.get("equals").and_then(Value::as_str).ok_or(format!("record_types[{i}].when.equals is required"))?;
                    if start == 0 {
                        return Err(format!("record_types[{i}].when.start is 1-based"));
                    }
                    Some((start as usize - 1, value.to_owned()))
                }
            };
            let name = t.get("name").and_then(Value::as_str).map(str::to_owned);
            out.push(make(name, marker, columns(t.get("columns"), &format!("record_types[{i}]"))?, line_length));
        }
        if out.is_empty() {
            return Err("_layout.record_types is empty".into());
        }
        Ok(out)
    } else {
        Ok(vec![make(None, None, columns(layout.get("columns"), "_layout")?, line_length)])
    }
}

/// Writes `records` (or `_layout.records_field`) as lines into `_layout.into` (default `file`),
/// with `line_count`, `errors` and `_valid`. `_layout` is removed.
fn write_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let Some(layout) = m.remove("_layout") else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!("missing _layout (see README)"));
        return Value::Object(m);
    };
    let fail = |mut m: Map<String, Value>, e: String| {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(e));
        Value::Object(m)
    };
    let types = match record_types(&layout) {
        Ok(t) => t,
        Err(e) => return fail(m, e),
    };
    let src = layout.get("records_field").and_then(Value::as_str).unwrap_or("records").to_owned();
    let into = layout.get("into").and_then(Value::as_str).unwrap_or("file").to_owned();
    let eol = layout.get("line_ending").and_then(Value::as_str).unwrap_or("\n").to_owned();
    let truncate = layout.get("truncate").and_then(Value::as_bool).unwrap_or(false);
    let Some(rows) = m.remove(&src).and_then(|v| v.as_array().cloned()) else {
        return fail(m, format!("field '{src}' is missing or not an array"));
    };
    let mut lines = Vec::new();
    let mut errors = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let want = row.get("_record_type").and_then(Value::as_str);
        let rt = match want {
            Some(w) => types.iter().find(|t| t.name.as_deref() == Some(w)),
            None if types.len() == 1 => types.first(),
            None => None,
        };
        let Some(rt) = rt else {
            errors.push(json!({ "record": i, "error": format!("no record type for _record_type {:?}", want) }));
            continue;
        };
        let mut line = vec![' '; rt.width];
        if let Some((s, v)) = &rt.marker {
            for (k, ch) in v.chars().enumerate() {
                line[s + k] = ch;
            }
        }
        for col in &rt.columns {
            let value = match &col.constant {
                Some(c) => Value::String(c.clone()),
                None => row.get(&col.name).cloned().unwrap_or(Value::Null),
            };
            match render(col, &value) {
                Ok(text) => {
                    let mut chars: Vec<char> = text.chars().collect();
                    if chars.len() > col.length {
                        if truncate && col.kind == Kind::Text {
                            chars.truncate(col.length);
                        } else {
                            errors.push(json!({ "record": i, "column": col.name, "error": format!("'{text}' is longer than {} characters", col.length) }));
                            chars = vec![col.pad; col.length];
                        }
                    }
                    let fill = col.length - chars.len();
                    let cell: Vec<char> = if col.align_right {
                        // Keep a leading minus sign in front of zero padding.
                        if col.pad == '0' && chars.first() == Some(&'-') {
                            std::iter::once('-').chain(std::iter::repeat_n('0', fill)).chain(chars[1..].iter().copied()).collect()
                        } else {
                            std::iter::repeat_n(col.pad, fill).chain(chars).collect()
                        }
                    } else {
                        chars.into_iter().chain(std::iter::repeat_n(col.pad, fill)).collect()
                    };
                    for (k, ch) in cell.into_iter().enumerate() {
                        if col.start + k < line.len() {
                            line[col.start + k] = ch;
                        }
                    }
                }
                Err(e) => errors.push(json!({ "record": i, "column": col.name, "error": e })),
            }
        }
        lines.push(line.into_iter().collect::<String>());
    }
    let mut text = lines.join(&eol);
    if !lines.is_empty() && layout.get("final_newline").and_then(Value::as_bool).unwrap_or(true) {
        text.push_str(&eol);
    }
    m.insert(into, json!(text));
    m.insert("line_count".into(), json!(lines.len()));
    m.insert("_valid".into(), json!(errors.is_empty()));
    m.insert("errors".into(), Value::Array(errors));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

/// Formats one value; null becomes blank (numbers become zeros).
fn render(col: &Column, v: &Value) -> Result<String, String> {
    if v.is_null() || v.as_str() == Some("") {
        return Ok(match col.kind {
            Kind::Integer | Kind::Decimal | Kind::Zoned => "0".repeat(col.length.min(1)),
            Kind::Date => if col.pad == '0' { "0".repeat(col.length) } else { String::new() },
            Kind::Text => String::new(),
        });
    }
    match col.kind {
        Kind::Text => Ok(v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())),
        Kind::Integer => {
            let n = number(v)?;
            if n.fract() != 0.0 {
                return Err(format!("{n} is not a whole number"));
            }
            Ok(format!("{}", n as i64))
        }
        Kind::Decimal => {
            let (neg, digits) = scaled(number(v)?, col.scale)?;
            Ok(if neg { format!("-{digits}") } else { digits })
        }
        Kind::Zoned => {
            let (neg, digits) = scaled(number(v)?, col.scale)?;
            let width = col.length.max(digits.len());
            let mut d: Vec<char> = format!("{digits:0>width$}").chars().collect();
            let last = d.pop().unwrap().to_digit(10).unwrap() as u8;
            d.push(match (neg, last) {
                (false, 0) => '{',
                (false, n) => (b'A' + n - 1) as char,
                (true, 0) => '}',
                (true, n) => (b'J' + n - 1) as char,
            });
            Ok(d.into_iter().collect())
        }
        Kind::Date => {
            let s = v.as_str().ok_or("date must be text like 2026-11-06")?;
            let p: Vec<&str> = s.get(..10).unwrap_or(s).split('-').collect();
            if p.len() != 3 || p[0].len() != 4 || p.iter().any(|x| x.parse::<u32>().is_err()) {
                return Err(format!("'{s}' is not an ISO date (YYYY-MM-DD)"));
            }
            Ok(match col.date_format.as_str() {
                "yymmdd" => format!("{}{}{}", &p[0][2..], p[1], p[2]),
                "mmddyyyy" => format!("{}{}{}", p[1], p[2], p[0]),
                "mmddyy" => format!("{}{}{}", p[1], p[2], &p[0][2..]),
                "ddmmyyyy" => format!("{}{}{}", p[2], p[1], p[0]),
                _ => format!("{}{}{}", p[0], p[1], p[2]),
            })
        }
    }
}

fn number(v: &Value) -> Result<f64, String> {
    match v {
        Value::Number(n) => n.as_f64().ok_or("not a number".into()),
        Value::String(s) => s.trim().parse::<f64>().map_err(|_| format!("'{s}' is not a number")),
        other => Err(format!("{other} is not a number")),
    }
}

/// The absolute value with the decimal point removed, rounded to `scale` places.
fn scaled(n: f64, scale: u32) -> Result<(bool, String), String> {
    let v = (n.abs() * 10f64.powi(scale as i32)).round();
    if v > 9.0e15 {
        return Err(format!("{n} is too large"));
    }
    Ok((n < 0.0 && v != 0.0, format!("{}", v as u64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    #[test]
    fn single_layout() {
        let out = go(json!({
            "records": [ { "id": 1, "name": "ACME CORP", "amount": 12.5 }, { "id": 2, "name": "GLOBEX INC", "amount": "99" } ],
            "_layout": { "columns": [
                { "name": "id", "start": 1, "length": 5, "type": "integer" },
                { "name": "name", "start": 6, "length": 14 },
                { "name": "amount", "start": 20, "length": 7, "type": "decimal", "scale": 2 }
            ]}
        }));
        assert_eq!(out["file"], json!("00001ACME CORP     0001250\n00002GLOBEX INC    0009900\n"));
        assert_eq!(out["_valid"], json!(true));
        assert!(out.get("records").is_none());
    }

    #[test]
    fn record_types_round_trip_with_the_parser_layout() {
        // Same layout and records as fixed-width-parser's sample.
        let layout = json!({ "record_types": [
            { "name": "header", "when": { "start": 1, "equals": "H" }, "columns": [
                { "name": "file_date", "start": 2, "length": 8, "type": "date" }, { "name": "batch", "start": 10, "length": 7 } ] },
            { "name": "detail", "when": { "start": 1, "equals": "D" }, "columns": [
                { "name": "seq", "start": 2, "length": 4, "type": "integer" }, { "name": "customer", "start": 6, "length": 14 },
                { "name": "amount", "start": 20, "length": 7, "type": "zoned", "scale": 2 }, { "name": "due_date", "start": 27, "length": 8, "type": "date", "pad": "0" } ] },
            { "name": "trailer", "when": { "start": 1, "equals": "T" }, "columns": [
                { "name": "detail_count", "start": 2, "length": 2, "type": "integer" } ] }
        ]});
        let out = go(json!({ "records": [
            { "_record_type": "header", "file_date": "2026-11-06", "batch": "BATCH-7" },
            { "_record_type": "detail", "seq": 1, "customer": "ACME CORP", "amount": 1.2, "due_date": "2026-11-01" },
            { "_record_type": "detail", "seq": 2, "customer": "GLOBEX INC", "amount": -0.11, "due_date": null },
            { "_record_type": "trailer", "detail_count": 2 }
        ], "_layout": layout }));
        assert_eq!(out["file"], json!("H20261106BATCH-7\nD0001ACME CORP     000012{20261101\nD0002GLOBEX INC    000001J00000000\nT02\n"), "{out}");
    }

    #[test]
    fn errors_and_options() {
        let out = go(json!({
            "rows": [ { "name": "A very long company name", "qty": 123456, "when": "11/06/2026" }, { "_record_type": "nope" } ],
            "_layout": { "records_field": "rows", "into": "payload", "line_ending": "\r\n", "columns": [
                { "name": "name", "start": 1, "length": 10 },
                { "name": "qty", "start": 11, "length": 3, "type": "integer" },
                { "name": "when", "start": 14, "length": 8, "type": "date" },
                { "name": "filler", "start": 22, "length": 2, "value": "XX" } ] }
        }));
        let errs = out["errors"].as_array().unwrap();
        assert_eq!(errs.len(), 4, "{errs:?}");
        // Without "truncate", a value that doesn't fit is an error and the field is left blank.
        assert_eq!(out["payload"], json!("          000        XX\r\n"));
        let cut = go(json!({ "records": [{ "name": "A very long company name" }], "_layout": { "truncate": true, "columns": [{ "name": "name", "start": 1, "length": 10 }] } }));
        assert_eq!(cut["file"], json!("A very lon\n"));
        assert_eq!(cut["_valid"], json!(true));
        let neg = go(json!({ "records": [{ "n": -42 }], "_layout": { "columns": [{ "name": "n", "start": 1, "length": 5, "type": "integer" }] } }));
        assert_eq!(neg["file"], json!("-0042\n"));
    }
}
