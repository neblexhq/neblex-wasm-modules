//! Neblex sandboxed module: parse fixed-width (positional) text files into records.
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(parse_record).collect()),
        Ok(v) => parse_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// Parses the text named by `_layout.field` and returns the record with `records`,
/// `line_count`, `errors` and `_valid` added. `_layout` is removed from the output.
fn parse_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let Some(layout) = m.remove("_layout") else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!("missing _layout (see README)"));
        return Value::Object(m);
    };
    let field = layout.get("field").and_then(Value::as_str).unwrap_or("file");
    let Some(text) = m.get(field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    let types = match record_types(&layout) {
        Ok(t) => t,
        Err(e) => {
            m.insert("ok".into(), json!(false));
            m.insert("error".into(), json!(e));
            return Value::Object(m);
        }
    };
    let skip = layout.get("skip_lines").and_then(Value::as_u64).unwrap_or(0) as usize;
    let keep_text = layout.get("keep_source").and_then(Value::as_bool).unwrap_or(false);

    let mut records = Vec::new();
    let mut errors = Vec::new();
    let mut line_count = 0;
    for (i, raw) in text.lines().enumerate().skip(skip) {
        let line_no = i + 1;
        let line: Vec<char> = raw.trim_end_matches('\r').chars().collect();
        if line.iter().all(|c| c.is_whitespace()) {
            continue;
        }
        line_count += 1;
        let Some(rt) = types.iter().find(|t| t.matches(&line)) else {
            errors.push(json!({ "line": line_no, "error": "no record type matches this line" }));
            continue;
        };
        let mut out = Map::new();
        if let Some(name) = &rt.name {
            out.insert("_record_type".into(), json!(name));
        }
        out.insert("_line".into(), json!(line_no));
        for col in &rt.columns {
            let (value, err) = col.extract(&line);
            if let Some(e) = err {
                errors.push(json!({ "line": line_no, "column": col.name, "error": e }));
            }
            out.insert(col.name.clone(), value);
        }
        records.push(Value::Object(out));
    }

    if !keep_text {
        m.remove(field);
    }
    m.insert("records".into(), Value::Array(records));
    m.insert("line_count".into(), json!(line_count));
    m.insert("_valid".into(), json!(errors.is_empty()));
    m.insert("errors".into(), Value::Array(errors));
    Value::Object(m)
}

struct Matcher {
    start: usize,
    value: String,
}

struct RecordType {
    name: Option<String>,
    when: Option<Matcher>,
    columns: Vec<Column>,
}

impl RecordType {
    fn matches(&self, line: &[char]) -> bool {
        match &self.when {
            None => true,
            Some(w) => {
                let end = w.start + w.value.chars().count();
                end <= line.len() && line[w.start..end].iter().copied().eq(w.value.chars())
            }
        }
    }
}

/// `_layout.columns` is a single record type; `_layout.record_types` is a list of
/// `{ name, when: { start, equals }, columns }`, tried in order.
fn record_types(layout: &Value) -> Result<Vec<RecordType>, String> {
    if let Some(list) = layout.get("record_types").and_then(Value::as_array) {
        let mut out = Vec::new();
        for (i, t) in list.iter().enumerate() {
            let when = match t.get("when") {
                None => None,
                Some(w) => {
                    let start = w.get("start").and_then(Value::as_u64).ok_or(format!("record_types[{i}].when.start is required"))?;
                    let value = w.get("equals").and_then(Value::as_str).ok_or(format!("record_types[{i}].when.equals is required"))?;
                    if start == 0 {
                        return Err(format!("record_types[{i}].when.start is 1-based"));
                    }
                    Some(Matcher { start: start as usize - 1, value: value.to_owned() })
                }
            };
            out.push(RecordType {
                name: t.get("name").and_then(Value::as_str).map(str::to_owned),
                when,
                columns: columns(t.get("columns"), &format!("record_types[{i}]"))?,
            });
        }
        if out.is_empty() {
            return Err("_layout.record_types is empty".into());
        }
        Ok(out)
    } else {
        Ok(vec![RecordType { name: None, when: None, columns: columns(layout.get("columns"), "_layout")? }])
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
    trim: bool,
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
        out.push(Column {
            name: name.to_owned(),
            start: start as usize - 1,
            length: length as usize,
            kind,
            scale: c.get("scale").and_then(Value::as_u64).unwrap_or(0) as u32,
            trim: c.get("trim").and_then(Value::as_bool).unwrap_or(true),
        });
    }
    Ok(out)
}

impl Column {
    /// Returns the parsed value, plus an error message when the slice is short or doesn't parse.
    fn extract(&self, line: &[char]) -> (Value, Option<String>) {
        if self.start >= line.len() {
            return (Value::Null, Some("line ends before this column".into()));
        }
        let end = (self.start + self.length).min(line.len());
        let raw: String = line[self.start..end].iter().collect();
        let short = end - self.start < self.length;
        let s = raw.trim();
        if self.kind == Kind::Text {
            let v = if self.trim { s.to_owned() } else { raw };
            return (json!(v), None);
        }
        if s.is_empty() {
            return (Value::Null, None);
        }
        let parsed = match self.kind {
            Kind::Integer => s.parse::<i64>().map(|n| json!(n)).map_err(|_| format!("'{s}' is not an integer")),
            Kind::Decimal => decimal(s, self.scale),
            Kind::Zoned => zoned(s, self.scale),
            Kind::Date => date(s),
            Kind::Text => unreachable!(),
        };
        match parsed {
            Ok(v) if short && self.kind != Kind::Date => (v, Some("line ends inside this column".into())),
            Ok(v) => (v, None),
            Err(e) => (Value::Null, Some(e)),
        }
    }
}

/// Number with an optional sign and either an explicit point ("12.50") or an implied
/// scale ("0001250" with scale 2 = 12.5). A trailing sign ("1250-") is accepted.
fn decimal(s: &str, scale: u32) -> Result<Value, String> {
    let (neg, digits) = match (s.strip_prefix('-'), s.strip_suffix('-')) {
        (Some(d), _) | (_, Some(d)) => (true, d.trim()),
        _ => (false, s.strip_prefix('+').or_else(|| s.strip_suffix('+')).unwrap_or(s).trim()),
    };
    if digits.contains('.') {
        let v: f64 = digits.parse().map_err(|_| format!("'{s}' is not a number"))?;
        return Ok(json!(if neg { -v } else { v }));
    }
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("'{s}' is not a number"));
    }
    scaled(digits, scale, neg)
}

/// COBOL zoned decimal (signed overpunch): the last character carries the sign.
/// '{' and 'A'-'I' are +0..+9, '}' and 'J'-'R' are -0..-9; a plain digit is positive.
fn zoned(s: &str, scale: u32) -> Result<Value, String> {
    let mut chars: Vec<char> = s.chars().collect();
    let last = chars.pop().ok_or("empty value")?;
    let (digit, neg) = match last {
        '0'..='9' => (last, false),
        '{' => ('0', false),
        'A'..='I' => ((b'1' + (last as u8 - b'A')) as char, false),
        '}' => ('0', true),
        'J'..='R' => ((b'1' + (last as u8 - b'J')) as char, true),
        _ => return Err(format!("'{s}' is not a zoned decimal")),
    };
    chars.push(digit);
    let digits: String = chars.into_iter().collect();
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("'{s}' is not a zoned decimal"));
    }
    scaled(&digits, scale, neg)
}

fn scaled(digits: &str, scale: u32, neg: bool) -> Result<Value, String> {
    let n: i128 = digits.parse().map_err(|_| format!("'{digits}' is too long"))?;
    if scale == 0 {
        let n = if neg { -n } else { n };
        return i64::try_from(n).map(|n| json!(n)).map_err(|_| format!("'{digits}' is too long"));
    }
    let v = n as f64 / 10f64.powi(scale as i32);
    Ok(json!(if neg { -v } else { v }))
}

/// YYYYMMDD or YYMMDD (years 00-69 are 20xx, 70-99 are 19xx) to an ISO date.
/// All zeros, the usual mainframe "no date", becomes null.
fn date(s: &str) -> Result<Value, String> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("'{s}' is not a YYYYMMDD or YYMMDD date"));
    }
    if s.bytes().all(|b| b == b'0') {
        return Ok(Value::Null);
    }
    let (y, rest) = match s.len() {
        8 => (s[..4].parse::<u32>().unwrap(), &s[4..]),
        6 => {
            let yy = s[..2].parse::<u32>().unwrap();
            (if yy < 70 { 2000 + yy } else { 1900 + yy }, &s[2..])
        }
        _ => return Err(format!("'{s}' is not a YYYYMMDD or YYMMDD date")),
    };
    let mo = rest[..2].parse::<u32>().unwrap();
    let d = rest[2..].parse::<u32>().unwrap();
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match mo {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    if d == 0 || d > days {
        return Err(format!("'{s}' is not a valid date"));
    }
    Ok(json!(format!("{y:04}-{mo:02}-{d:02}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    #[test]
    fn single_layout_with_implied_decimals() {
        let out = go(json!({
            "file": "00001ACME CORP     0001250\n00002GLOBEX INC    0009900\n",
            "_layout": { "columns": [
                { "name": "id", "start": 1, "length": 5, "type": "integer" },
                { "name": "name", "start": 6, "length": 14 },
                { "name": "amount", "start": 20, "length": 7, "type": "decimal", "scale": 2 }
            ]}
        }));
        assert_eq!(out["_valid"], json!(true));
        assert_eq!(out["records"][0], json!({ "_line": 1, "id": 1, "name": "ACME CORP", "amount": 12.5 }));
        assert_eq!(out["records"][1]["amount"], json!(99.0));
        assert!(out.get("file").is_none() && out.get("_layout").is_none());
    }

    #[test]
    fn record_types_by_prefix() {
        let out = go(json!({
            "text": "H20261106BATCH-7\nD0001000012{\nD0002000001J\nT02\n",
            "_layout": { "field": "text", "record_types": [
                { "name": "header", "when": { "start": 1, "equals": "H" }, "columns": [
                    { "name": "date", "start": 2, "length": 8, "type": "date" },
                    { "name": "batch", "start": 10, "length": 7 } ] },
                { "name": "detail", "when": { "start": 1, "equals": "D" }, "columns": [
                    { "name": "seq", "start": 2, "length": 4, "type": "integer" },
                    { "name": "amount", "start": 6, "length": 7, "type": "zoned", "scale": 2 } ] },
                { "name": "trailer", "when": { "start": 1, "equals": "T" }, "columns": [
                    { "name": "count", "start": 2, "length": 2, "type": "integer" } ] }
            ]}
        }));
        assert_eq!(out["_valid"], json!(true), "{out}");
        assert_eq!(out["records"][0]["date"], json!("2026-11-06"));
        assert_eq!(out["records"][1]["amount"], json!(1.2));
        assert_eq!(out["records"][2]["amount"], json!(-0.11));
        assert_eq!(out["records"][3]["_record_type"], json!("trailer"));
    }

    #[test]
    fn errors_are_reported_per_line_and_column() {
        let out = go(json!({
            "file": "00X01ACME\nZZZ\n",
            "_layout": { "record_types": [
                { "name": "row", "when": { "start": 1, "equals": "0" }, "columns": [
                    { "name": "id", "start": 1, "length": 5, "type": "integer" },
                    { "name": "amount", "start": 20, "length": 7, "type": "decimal" } ] }
            ]}
        }));
        assert_eq!(out["_valid"], json!(false));
        let errs = out["errors"].as_array().unwrap();
        assert_eq!(errs.len(), 3, "{errs:?}");
        assert_eq!(errs[2], json!({ "line": 2, "error": "no record type matches this line" }));
    }

    #[test]
    fn signs_dates_and_bad_layouts() {
        assert_eq!(decimal("1250-", 2).unwrap(), json!(-12.5));
        assert_eq!(decimal("-12.50", 0).unwrap(), json!(-12.5));
        assert_eq!(zoned("12}", 1).unwrap(), json!(-12.0));
        assert_eq!(date("000000").unwrap(), Value::Null);
        assert_eq!(date("991231").unwrap(), json!("1999-12-31"));
        assert!(date("20260230").is_err());
        let out = go(json!({ "file": "x", "_layout": { "columns": [{ "name": "a", "start": 0, "length": 1 }] } }));
        assert_eq!(out["ok"], json!(false));
    }
}
