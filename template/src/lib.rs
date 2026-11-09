//! Starting point for a Neblex sandboxed module.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. Replace `transform` with your own logic.

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
    let out = match serde_json::from_slice::<Value>(input) {
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(transform).collect()),
        Ok(v) => transform(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    };
    let out = serde_json::to_vec(&out).unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"serialize\"}".to_vec()).leak();
    ((out.as_ptr() as i64) << 32) | (out.len() as i64)
}

/// Example logic: count the fields on each record and pass the record through.
fn transform(record: Value) -> Value {
    match record {
        Value::Object(mut m) => {
            let n = m.len();
            m.insert("_field_count".into(), json!(n));
            Value::Object(m)
        }
        other => json!({ "value": other, "_field_count": 0 }),
    }
}
