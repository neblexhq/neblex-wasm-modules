//! Neblex sandboxed module: convert YAML (configuration files, API specs, CI files, front matter)
//! to JSON, one document or several.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the options.

use serde::Deserialize;
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(convert_record).collect()),
        Ok(v) => convert_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// YAML values to JSON: keys that aren't text (numbers, booleans) become text, tagged values keep
/// their value, and non-finite numbers (.inf, .nan) become null.
fn to_json(v: serde_yaml::Value) -> Value {
    match v {
        serde_yaml::Value::Null => Value::Null,
        serde_yaml::Value::Bool(b) => json!(b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                json!(i)
            } else if let Some(u) = n.as_u64() {
                json!(u)
            } else {
                n.as_f64().filter(|f| f.is_finite()).map(|f| json!(f)).unwrap_or(Value::Null)
            }
        }
        serde_yaml::Value::String(s) => json!(s),
        serde_yaml::Value::Sequence(items) => Value::Array(items.into_iter().map(to_json).collect()),
        serde_yaml::Value::Mapping(m) => {
            let mut o = Map::new();
            for (k, v) in m {
                let key = match to_json(k) {
                    Value::String(s) => s,
                    Value::Null => "null".to_owned(),
                    other => other.to_string(),
                };
                o.insert(key, to_json(v));
            }
            Value::Object(o)
        }
        serde_yaml::Value::Tagged(t) => to_json(t.value),
    }
}

/// Every document in the text (separated by "---"), converted. Errors include the line and column.
pub fn documents(text: &str) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for doc in serde_yaml::Deserializer::from_str(text) {
        let describe = |e: serde_yaml::Error| match e.location() {
            Some(l) => format!("{} (line {}, column {})", e.to_string().split(" at line").next().unwrap_or(""), l.line(), l.column()),
            None => e.to_string(),
        };
        let mut v = serde_yaml::Value::deserialize(doc).map_err(describe)?;
        // Merge keys ("<<: *defaults") copy the anchored mapping's fields in.
        v.apply_merge().map_err(describe)?;
        out.push(to_json(v));
    }
    Ok(out)
}

/// Reads YAML from `_options.field` (default `yaml`) and adds `data` (the document), or with several
/// documents in the text, `documents` (a list) and `document_count`.
fn convert_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("yaml").to_owned();
    let always_list = opt.get("always_list").and_then(Value::as_bool).unwrap_or(false);
    let Some(text) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    match documents(&text) {
        Ok(docs) => {
            if !opt.get("keep_source").and_then(Value::as_bool).unwrap_or(false) {
                m.remove(&field);
            }
            if docs.len() == 1 && !always_list {
                m.insert("data".into(), docs.into_iter().next().unwrap());
            } else {
                m.insert("document_count".into(), json!(docs.len()));
                m.insert("documents".into(), Value::Array(docs));
            }
            m.insert("ok".into(), json!(true));
        }
        Err(e) => {
            m.insert("ok".into(), json!(false));
            m.insert("error".into(), json!(format!("not valid YAML: {e}")));
        }
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "# Sync settings\nname: hubspot-to-erp\nenabled: true\nschedule: \"*/15 * * * *\"\nbatch_size: 500\nthreshold: 0.85\n\
defaults: &defaults\n  retries: 3\n  timeout_s: 30\ntargets:\n  - system: erp\n    <<: *defaults\n    fields: [email, company, phone]\n  - system: warehouse\n    retries: 5\n\
notes: |\n  Line one\n  Line two\nmissing: ~\n404: not found\nversion: 1.10\nzip: \"02134\"\n";

    #[test]
    fn config_file() {
        let out = run(serde_json::to_vec(&json!({ "yaml": CONFIG })).unwrap().as_slice());
        let d = &out["data"];
        assert_eq!(d["name"], json!("hubspot-to-erp"));
        assert_eq!(d["enabled"], json!(true));
        assert_eq!(d["batch_size"], json!(500));
        assert_eq!(d["threshold"], json!(0.85));
        assert_eq!(d["targets"][0]["retries"], json!(3));
        assert_eq!(d["targets"][0]["fields"], json!(["email", "company", "phone"]));
        assert_eq!(d["targets"][1], json!({ "system": "warehouse", "retries": 5 }));
        assert_eq!(d["notes"], json!("Line one\nLine two\n"));
        assert_eq!(d["missing"], Value::Null);
        assert_eq!(d["404"], json!("not found"));
        assert_eq!(d["version"], json!(1.1));
        assert_eq!(d["zip"], json!("02134"));
        assert!(out.get("yaml").is_none());
    }

    #[test]
    fn several_documents_and_errors() {
        let out = run(br#"{"yaml": "kind: a\n---\nkind: b\n"}"#);
        assert_eq!(out["document_count"], json!(2));
        assert_eq!(out["documents"][1], json!({ "kind": "b" }));
        let one = run(br#"{"yaml": "x: 1", "_options": {"always_list": true}}"#);
        assert_eq!(one["documents"], json!([{ "x": 1 }]));
        let bad = run(br#"{"yaml": "a: [1, 2\nb: 3"}"#);
        assert_eq!(bad["ok"], json!(false));
        assert!(bad["error"].as_str().unwrap().contains("line"), "{bad}");
        assert_eq!(run(br#"{"other": 1}"#)["ok"], json!(false));
    }

    #[test]
    fn alias_bomb_is_refused() {
        let bomb = "a: &a [x, x, x, x, x, x, x, x, x]\nb: &b [*a, *a, *a, *a, *a, *a, *a, *a, *a]\nc: &c [*b, *b, *b, *b, *b, *b, *b, *b, *b]\nd: &d [*c, *c, *c, *c, *c, *c, *c, *c, *c]\ne: &e [*d, *d, *d, *d, *d, *d, *d, *d, *d]\nf: &f [*e, *e, *e, *e, *e, *e, *e, *e, *e]\ng: &g [*f, *f, *f, *f, *f, *f, *f, *f, *f]\nh: [*g, *g, *g, *g, *g, *g, *g, *g, *g]\n";
        let out = run(serde_json::to_vec(&json!({ "yaml": bomb })).unwrap().as_slice());
        assert_eq!(out["ok"], json!(false), "expansion should be refused");
    }
}
