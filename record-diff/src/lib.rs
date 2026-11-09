//! Neblex sandboxed module: compare an incoming record with the stored version and return only what changed.
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(diff_record).collect()),
        Ok(v) => diff_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

struct Options {
    ignore: Vec<String>,
    only: Option<Vec<String>>,
    case_insensitive: bool,
    trim: bool,
    empty_equal: bool,
    tolerance: f64,
    numeric_strings: bool,
    report_removed: bool,
}

fn options(v: &Value) -> Options {
    let list = |k: &str| v.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect::<Vec<_>>());
    let b = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
    Options {
        ignore: list("ignore").unwrap_or_default(),
        only: list("only"),
        case_insensitive: b("case_insensitive", false),
        trim: b("trim", true),
        empty_equal: b("empty_equal", true),
        tolerance: v.get("tolerance").and_then(Value::as_f64).unwrap_or(0.0),
        numeric_strings: b("numeric_strings", true),
        report_removed: b("report_removed", false),
    }
}

/// Compares `old` with `new` (names set by `_options.old` and `_options.new`) and adds
/// `changed`, `changed_fields`, `changes` ({ path: { old, new } }) and `patch` (the new values
/// of changed fields, nested like the record). Paths are dotted; arrays compare as a whole.
fn diff_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let o = m.remove("_options").unwrap_or(Value::Null);
    let old_key = o.get("old").and_then(Value::as_str).unwrap_or("old").to_owned();
    let new_key = o.get("new").and_then(Value::as_str).unwrap_or("new").to_owned();
    let opt = options(&o);
    let old = m.get(&old_key).cloned().unwrap_or(Value::Null);
    let Some(new) = m.get(&new_key).cloned().filter(Value::is_object) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{new_key}' is missing or not an object")));
        return Value::Object(m);
    };
    // No stored record at all means everything is new.
    let is_new = old.is_null();
    let old = if is_new { json!({}) } else { old };
    if !old.is_object() {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{old_key}' is not an object")));
        return Value::Object(m);
    }
    let mut changes = Map::new();
    walk("", &old, &new, &opt, &mut changes);
    let mut patch = Value::Object(Map::new());
    for (path, c) in &changes {
        if c.get("removed").is_none() {
            set_path(&mut patch, path, c["new"].clone());
        }
    }
    let fields: Vec<String> = changes.keys().cloned().collect();
    m.insert("is_new".into(), json!(is_new));
    m.insert("changed".into(), json!(!changes.is_empty()));
    m.insert("changed_fields".into(), json!(fields));
    m.insert("changes".into(), Value::Object(changes));
    m.insert("patch".into(), patch);
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

fn wanted(path: &str, opt: &Options) -> bool {
    let hit = |list: &[String]| list.iter().any(|p| path == p || path.starts_with(&format!("{p}.")) || p.starts_with(&format!("{path}.")));
    if opt.ignore.iter().any(|p| path == p || path.starts_with(&format!("{p}."))) {
        return false;
    }
    opt.only.as_ref().is_none_or(|only| hit(only))
}

fn walk(prefix: &str, old: &Value, new: &Value, opt: &Options, out: &mut Map<String, Value>) {
    let (Some(o), Some(n)) = (old.as_object(), new.as_object()) else { return };
    let none = Value::Object(Map::new());
    for (k, nv) in n {
        let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
        if !wanted(&path, opt) {
            continue;
        }
        let ov = o.get(k).unwrap_or(&Value::Null);
        if nv.is_object() && (ov.is_object() || ov.is_null()) {
            walk(&path, if ov.is_null() { &none } else { ov }, nv, opt, out);
        } else if !same(ov, nv, opt) {
            out.insert(path, json!({ "old": ov, "new": nv }));
        }
    }
    if opt.report_removed {
        for (k, ov) in o {
            let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            if !n.contains_key(k) && wanted(&path, opt) && !(opt.empty_equal && empty(ov)) {
                out.insert(path, json!({ "old": ov, "new": null, "removed": true }));
            }
        }
    }
}

fn empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

fn same(a: &Value, b: &Value, opt: &Options) -> bool {
    if opt.empty_equal && empty(a) && empty(b) {
        return true;
    }
    let num = |v: &Value| match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if opt.numeric_strings => s.trim().parse::<f64>().ok(),
        _ => None,
    };
    if let (Some(x), Some(y)) = (num(a), num(b)) {
        if a.is_number() || b.is_number() {
            return (x - y).abs() <= opt.tolerance;
        }
    }
    match (a, b) {
        (Value::String(x), Value::String(y)) => {
            let (x, y) = if opt.trim { (x.trim(), y.trim()) } else { (x.as_str(), y.as_str()) };
            if opt.case_insensitive {
                x.to_lowercase() == y.to_lowercase()
            } else {
                x == y
            }
        }
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q, opt)),
        _ => a == b,
    }
}

fn set_path(root: &mut Value, path: &str, v: Value) {
    let mut cur = root;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, p) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            cur.as_object_mut().unwrap().insert((*p).to_owned(), v);
            return;
        }
        cur = cur.as_object_mut().unwrap().entry(*p).or_insert_with(|| Value::Object(Map::new()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    #[test]
    fn changed_fields_and_patch() {
        let out = go(json!({
            "old": { "id": 7, "email": "Jane@Example.com", "phone": "", "amount": "1250.00", "address": { "city": "Boston", "zip": "02134" }, "updated_at": "2026-11-01", "tags": ["a", "b"] },
            "new": { "id": 7, "email": "jane@example.com ", "phone": null, "amount": 1250, "address": { "city": "Cambridge", "zip": "02134" }, "updated_at": "2026-11-09", "tags": ["a", "b", "c"], "tier": "gold" },
            "_options": { "ignore": ["updated_at"], "case_insensitive": true }
        }));
        assert_eq!(out["changed"], json!(true));
        assert_eq!(out["changed_fields"], json!(["address.city", "tags", "tier"]));
        assert_eq!(out["changes"]["address.city"], json!({ "old": "Boston", "new": "Cambridge" }));
        assert_eq!(out["patch"], json!({ "address": { "city": "Cambridge" }, "tags": ["a", "b", "c"], "tier": "gold" }));
        assert_eq!(out["is_new"], json!(false));
    }

    #[test]
    fn unchanged_record() {
        let out = go(json!({ "old": { "a": 1, "b": "x" }, "new": { "a": 1.0, "b": "x" } }));
        assert_eq!(out["changed"], json!(false));
        assert_eq!(out["patch"], json!({}));
    }

    #[test]
    fn strict_options_and_tolerance() {
        let strict = go(json!({ "old": { "e": "A@b.co", "p": "" }, "new": { "e": "a@b.co", "p": null }, "_options": { "empty_equal": false } }));
        assert_eq!(strict["changed_fields"], json!(["e", "p"]));
        let tol = go(json!({ "old": { "price": 9.99 }, "new": { "price": 9.994 }, "_options": { "tolerance": 0.005 } }));
        assert_eq!(tol["changed"], json!(false));
    }

    #[test]
    fn new_record_only_and_removed() {
        let n = go(json!({ "new": { "a": 1, "b": { "c": 2 } } }));
        assert_eq!(n["is_new"], json!(true));
        assert_eq!(n["changed_fields"], json!(["a", "b.c"]));
        let o = go(json!({ "stored": { "a": 1, "b": 2, "c": 3 }, "incoming": { "a": 2, "c": 3 }, "_options": { "old": "stored", "new": "incoming", "only": ["a", "b"], "report_removed": true } }));
        assert_eq!(o["changed_fields"], json!(["a", "b"]));
        assert_eq!(o["changes"]["b"]["removed"], json!(true));
        assert_eq!(o["patch"], json!({ "a": 2 }));
        assert_eq!(go(json!({ "old": {} }))["ok"], json!(false));
    }
}
