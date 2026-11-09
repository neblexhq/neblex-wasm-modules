//! Neblex sandboxed module: validate and normalize business identifiers on each record.
//! Checks IBAN, EU/UK VAT number format, GTIN/EAN/UPC, US ABA routing numbers and LEI.
//! Contract (from the Worker runtime): export `memory`, `alloc(i32)->i32`,
//! `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len; no imports.
//! Offline only: checks format and check digits, not whether an account or VAT number exists.

mod checks;

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
    let out = serde_json::to_vec(&run(input)).unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"serialize\"}".to_vec());
    let out = out.leak();
    ((out.as_ptr() as i64) << 32) | (out.len() as i64)
}

pub fn run(input: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(input) {
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(validate_record).collect()),
        Ok(v) => validate_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// Passes the record through and adds `_validation` (one entry per checked field)
/// and `_valid` (false if any checked field failed). Fields to check come from
/// `_checks` ({"field": "iban"}) when present, otherwise from the field names.
fn validate_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each record must be a JSON object" });
    };
    let explicit = m.remove("_checks");
    let targets: Vec<(String, String)> = match &explicit {
        Some(Value::Object(c)) => c.iter().filter_map(|(f, t)| Some((f.clone(), t.as_str()?.to_string()))).collect(),
        _ => m.keys().filter_map(|k| Some((k.clone(), checks::kind_for_field(k)?.to_string()))).collect(),
    };

    let mut results = Map::new();
    let mut all_valid = true;
    for (field, kind) in targets {
        let result = match m.get(&field) {
            Some(Value::String(s)) if !s.trim().is_empty() => checks::check(&kind, s),
            Some(Value::Number(n)) => checks::check(&kind, &n.to_string()),
            Some(Value::Null) | None => continue,
            Some(Value::String(_)) => continue,
            Some(_) => checks::invalid(&kind, "value is not a string or number"),
        };
        if result.get("valid") != Some(&Value::Bool(true)) {
            all_valid = false;
        }
        results.insert(field, result);
    }
    m.insert("_validation".into(), Value::Object(results));
    m.insert("_valid".into(), Value::Bool(all_valid));
    Value::Object(m)
}
