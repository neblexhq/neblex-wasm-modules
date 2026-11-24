//! Neblex sandboxed module: validate records against a JSON Schema and list every problem with its field.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the options.

use jsonschema::error::ValidationErrorKind;
use jsonschema::{Draft, Validator};
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

/// The sandbox has no random source, and the validator only needs random bytes to seed hash
/// tables, so a fixed-seed generator is enough (results never depend on it).
#[cfg(target_arch = "wasm32")]
#[no_mangle]
unsafe extern "Rust" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> Result<(), getrandom::Error> {
    static mut STATE: u64 = 0x9E37_79B9_7F4A_7C15;
    for i in 0..len {
        STATE ^= STATE << 13;
        STATE ^= STATE >> 7;
        STATE ^= STATE << 17;
        *dest.add(i) = STATE as u8;
    }
    Ok(())
}

pub fn run(input: &[u8]) -> Value {
    let mut cache = Cache::default();
    match serde_json::from_slice::<Value>(input) {
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(|r| validate_record(r, &mut cache)).collect()),
        Ok(v) => validate_record(v, &mut cache),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// The compiled schema of the previous record, reused while records share the same schema.
#[derive(Default)]
struct Cache {
    key: String,
    validator: Option<Validator>,
}

/// Validates the record (or the field named by `_options.field`) against `_options.schema` and
/// adds `valid`, `error_count`, `errors` ([{ field, path, keyword, message, schema_path }]) and
/// `invalid_fields`.
fn validate_record(record: Value, cache: &mut Cache) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let o = m.remove("_options").unwrap_or(Value::Null);
    let fail = |mut m: Map<String, Value>, msg: String| {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(msg));
        Value::Object(m)
    };
    // The schema may arrive as an object or as JSON text (for example from a file or a table cell).
    let schema = match o.get("schema") {
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v,
            Err(e) => return fail(m, format!("_options.schema is not valid JSON: {e}")),
        },
        Some(v @ (Value::Object(_) | Value::Bool(_))) => v.clone(),
        _ => return fail(m, "_options.schema is missing; pass a JSON Schema object".into()),
    };
    let formats = o.get("formats").and_then(Value::as_bool).unwrap_or(true);
    let max_errors = o.get("max_errors").and_then(Value::as_u64).unwrap_or(50) as usize;
    let draft = match o.get("draft").map(|d| d.as_str().map(str::to_owned).unwrap_or_else(|| d.to_string())) {
        None => None,
        Some(d) => match d.trim_start_matches("draft").trim_start_matches(['-', ' ']) {
            "4" | "04" => Some(Draft::Draft4),
            "6" | "06" => Some(Draft::Draft6),
            "7" | "07" => Some(Draft::Draft7),
            "2019-09" => Some(Draft::Draft201909),
            "2020-12" => Some(Draft::Draft202012),
            other => return fail(m, format!("unknown draft '{other}'; use 4, 6, 7, 2019-09 or 2020-12")),
        },
    };

    let key = format!("{formats}|{draft:?}|{schema}");
    if cache.key != key || cache.validator.is_none() {
        let mut opts = jsonschema::options().should_validate_formats(formats);
        if let Some(d) = draft {
            opts = opts.with_draft(d);
        }
        match opts.build(&schema) {
            Ok(v) => {
                cache.validator = Some(v);
                cache.key = key;
            }
            Err(e) => {
                cache.validator = None;
                return fail(m, format!("the schema is not valid: {e}"));
            }
        }
    }
    let validator = cache.validator.as_ref().expect("compiled above");

    let field = o.get("field").and_then(Value::as_str).filter(|f| !f.is_empty());
    let instance = match field {
        Some(f) => get_path(&Value::Object(m.clone()), f).cloned().unwrap_or(Value::Null),
        None => Value::Object(m.clone()),
    };

    let mut errors = Vec::new();
    let mut fields: Vec<String> = Vec::new();
    let mut count = 0usize;
    for e in validator.iter_errors(&instance) {
        count += 1;
        let pointer = e.instance_path().as_str().to_owned();
        let mut dotted = pointer_to_field(&pointer);
        // For a missing required property, point at the property itself, not its parent.
        if let ValidationErrorKind::Required { property } = e.kind() {
            if let Some(p) = property.as_str() {
                dotted = if dotted.is_empty() { p.to_owned() } else { format!("{dotted}.{p}") };
            }
        }
        if let Some(f) = field {
            dotted = if dotted.is_empty() { f.to_owned() } else if dotted.starts_with('[') { format!("{f}{dotted}") } else { format!("{f}.{dotted}") };
        }
        if !fields.contains(&dotted) {
            fields.push(dotted.clone());
        }
        if errors.len() < max_errors {
            errors.push(json!({
                "field": dotted,
                "path": pointer,
                "keyword": e.kind().keyword(),
                "message": e.to_string(),
                "schema_path": e.schema_path().as_str(),
            }));
        }
    }
    m.insert("valid".into(), json!(count == 0));
    m.insert("error_count".into(), json!(count));
    m.insert("errors".into(), Value::Array(errors));
    m.insert("invalid_fields".into(), json!(fields));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

/// `/items/0/qty` -> `items[0].qty` (JSON Pointer escapes ~1 and ~0 undone).
fn pointer_to_field(pointer: &str) -> String {
    let mut out = String::new();
    for seg in pointer.split('/').skip(1) {
        let seg = seg.replace("~1", "/").replace("~0", "~");
        if !seg.is_empty() && seg.bytes().all(|b| b.is_ascii_digit()) {
            out.push_str(&format!("[{seg}]"));
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out.push_str(&seg);
        }
    }
    out
}

/// Reads a dotted path such as `order.lines` or `order.lines.0` from the record.
fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, k| match cur {
        Value::Object(m) => m.get(k),
        Value::Array(a) => k.parse::<usize>().ok().and_then(|i| a.get(i)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    fn order_schema() -> Value {
        json!({
            "type": "object",
            "required": ["order_id", "email", "lines"],
            "properties": {
                "order_id": { "type": "string", "pattern": "^PO-[0-9]{4,}$" },
                "email": { "type": "string", "format": "email" },
                "ship_date": { "type": "string", "format": "date" },
                "currency": { "enum": ["USD", "EUR", "GBP"] },
                "lines": {
                    "type": "array", "minItems": 1,
                    "items": {
                        "type": "object", "required": ["sku", "qty"],
                        "properties": { "sku": { "type": "string", "minLength": 3 }, "qty": { "type": "integer", "minimum": 1 } },
                        "additionalProperties": false
                    }
                }
            }
        })
    }

    #[test]
    fn valid_record() {
        let out = go(json!({
            "order_id": "PO-10042", "email": "buyer@example.com", "ship_date": "2026-12-01", "currency": "USD",
            "lines": [{ "sku": "AB-100", "qty": 3 }],
            "_options": { "schema": order_schema() }
        }));
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["valid"], json!(true));
        assert_eq!(out["error_count"], json!(0));
        assert_eq!(out["errors"], json!([]));
        assert!(out.get("_options").is_none());
    }

    #[test]
    fn every_error_with_field_names() {
        let out = go(json!({
            "order_id": "10042", "email": "not-an-email", "ship_date": "2026-13-45", "currency": "CAD",
            "lines": [{ "sku": "AB", "qty": 0, "note": "x" }, { "qty": 2 }],
            "_options": { "schema": order_schema() }
        }));
        assert_eq!(out["valid"], json!(false));
        let fields: Vec<&str> = out["invalid_fields"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        for f in ["order_id", "email", "ship_date", "currency", "lines[0].sku", "lines[0].qty", "lines[0]", "lines[1].sku"] {
            assert!(fields.contains(&f), "missing {f} in {fields:?}");
        }
        let kw: Vec<&str> = out["errors"].as_array().unwrap().iter().map(|e| e["keyword"].as_str().unwrap()).collect();
        for k in ["pattern", "format", "enum", "minLength", "minimum", "additionalProperties", "required"] {
            assert!(kw.contains(&k), "missing keyword {k} in {kw:?}");
        }
        let req = out["errors"].as_array().unwrap().iter().find(|e| e["keyword"] == "required").unwrap();
        assert_eq!(req["field"], json!("lines[1].sku"));
        assert_eq!(req["path"], json!("/lines/1"));
    }

    #[test]
    fn missing_required_top_level() {
        let out = go(json!({ "email": "a@b.co", "_options": { "schema": order_schema() } }));
        assert_eq!(out["invalid_fields"], json!(["order_id", "lines"]));
    }

    #[test]
    fn formats_can_be_annotations_only() {
        let out = go(json!({ "email": "nope", "_options": { "schema": { "properties": { "email": { "format": "email" } } }, "formats": false } }));
        assert_eq!(out["valid"], json!(true));
    }

    #[test]
    fn validates_one_field_and_schema_as_text() {
        let schema = r#"{"type":"object","required":["zip"],"properties":{"zip":{"type":"string","pattern":"^[0-9]{5}$"}}}"#;
        let out = go(json!({ "id": 1, "address": { "zip": "0213" }, "_options": { "schema": schema, "field": "address" } }));
        assert_eq!(out["valid"], json!(false));
        assert_eq!(out["invalid_fields"], json!(["address.zip"]));
        let missing = go(json!({ "id": 1, "_options": { "schema": schema, "field": "address" } }));
        assert_eq!(missing["valid"], json!(false));
        assert_eq!(missing["errors"][0]["keyword"], json!("type"));
    }

    #[test]
    fn defs_and_refs_resolve_locally() {
        let schema = json!({
            "$defs": { "money": { "type": "number", "multipleOf": 0.01, "minimum": 0 } },
            "properties": { "total": { "$ref": "#/$defs/money" }, "tax": { "$ref": "#/$defs/money" } }
        });
        let out = go(json!({ "total": 10.5, "tax": -1, "_options": { "schema": schema } }));
        assert_eq!(out["invalid_fields"], json!(["tax"]));
    }

    #[test]
    fn remote_ref_is_reported_not_fetched() {
        let out = go(json!({ "a": 1, "_options": { "schema": { "$ref": "https://example.com/schema.json" } } }));
        assert_eq!(out["ok"], json!(false));
        assert!(out["error"].as_str().unwrap().contains("schema is not valid"));
    }

    #[test]
    fn draft_7_and_conditionals() {
        let schema = json!({
            "if": { "properties": { "country": { "const": "US" } } },
            "then": { "required": ["state"] },
            "else": { "required": ["region"] }
        });
        let us = go(json!({ "country": "US", "_options": { "schema": schema.clone(), "draft": 7 } }));
        assert_eq!(us["invalid_fields"], json!(["state"]));
        let de = go(json!({ "country": "DE", "region": "BY", "_options": { "schema": schema, "draft": "7" } }));
        assert_eq!(de["valid"], json!(true));
    }

    #[test]
    fn max_errors_caps_the_list_not_the_count() {
        let schema = json!({ "type": "array", "items": { "type": "integer" } });
        let out = go(json!({ "values": ["a", "b", "c", "d"], "_options": { "schema": schema, "field": "values", "max_errors": 2 } }));
        assert_eq!(out["error_count"], json!(4));
        assert_eq!(out["errors"].as_array().unwrap().len(), 2);
        assert_eq!(out["invalid_fields"], json!(["values[0]", "values[1]", "values[2]", "values[3]"]));
    }

    #[test]
    fn batch_reuses_schema_and_bad_input() {
        let schema = json!({ "required": ["id"] });
        let out = go(json!([
            { "id": 1, "_options": { "schema": schema.clone() } },
            { "name": "x", "_options": { "schema": schema } },
            "not an object"
        ]));
        assert_eq!(out[0]["valid"], json!(true));
        assert_eq!(out[1]["valid"], json!(false));
        assert_eq!(out[2]["ok"], json!(false));
        let none = go(json!({ "id": 1 }));
        assert_eq!(none["ok"], json!(false));
        let bad = go(json!({ "id": 1, "_options": { "schema": { "type": "nonsense" } } }));
        assert_eq!(bad["ok"], json!(false));
    }

    #[test]
    fn pointer_conversion() {
        assert_eq!(pointer_to_field("/a~1b/0/c~0d"), "a/b[0].c~d");
        assert_eq!(pointer_to_field(""), "");
    }
}
