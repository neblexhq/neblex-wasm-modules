//! Neblex sandboxed module: PDF (base64 inside the step input JSON) -> JSON text.
//! Contract (from the Worker runtime): export `memory`, `alloc(i32)->i32`,
//! `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len; no imports.
//! Deliberately small: a text-only PDF reader, no encryption, images or OCR.

mod pdf;

use base64::Engine;
use serde_json::{json, Value};

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

fn run(input: &[u8]) -> Value {
    let value: Value = match serde_json::from_slice(input) {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    };
    let Some((field, b64)) = find_pdf(&value, String::new()) else {
        return json!({ "ok": false, "error": "no base64 PDF found in input (expected a string starting with JVBERi or a data:application/pdf URL)" });
    };
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(cleaned) {
        Ok(b) => b,
        Err(e) => return json!({ "ok": false, "error": format!("invalid base64 in {field}: {e}") }),
    };
    let doc = match pdf::extract(&bytes) {
        Ok(d) => d,
        Err(e) => return json!({ "ok": false, "source_field": field, "error": e }),
    };
    let text = doc.pages.join("\n\n");
    let pages: Vec<Value> = doc.pages.iter().enumerate().map(|(i, t)| json!({ "page": i + 1, "text": t })).collect();
    json!({
        "ok": true,
        "source_field": field,
        "page_count": pages.len(),
        "metadata": doc.metadata,
        "has_text": !text.trim().is_empty(),
        "text": text,
        "pages": pages,
    })
}

/// Depth-first search for the first string that is a base64 PDF.
fn find_pdf(v: &Value, path: String) -> Option<(String, String)> {
    match v {
        Value::String(s) => {
            let s = s.trim_start();
            if s.starts_with("JVBERi") {
                return Some((path, s.to_string()));
            }
            s.strip_prefix("data:application/pdf;base64,").map(|rest| (path, rest.to_string()))
        }
        Value::Object(m) => m.iter().find_map(|(k, x)| {
            find_pdf(x, if path.is_empty() { k.clone() } else { format!("{path}.{k}") })
        }),
        Value::Array(a) => a.iter().enumerate().find_map(|(i, x)| find_pdf(x, format!("{path}[{i}]"))),
        _ => None,
    }
}
