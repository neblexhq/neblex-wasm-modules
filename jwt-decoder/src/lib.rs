//! Neblex sandboxed module: decode JSON Web Tokens (JWT) into their header and claims, with
//! readable dates and expiry checks. It does not verify signatures.
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(decode_record).collect()),
        Ok(v) => decode_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// Base64url (with or without padding; standard base64 characters are accepted too).
fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Unix seconds to ISO 8601 UTC, e.g. 1762707600 -> "2025-11-09T17:00:00Z".
pub fn iso(secs: i64) -> String {
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", t / 3600, t / 60 % 60, t % 60)
}

/// "2026-11-09T17:00:00Z", "2026-11-09T12:00:00-05:00", "2026-11-09" or unix seconds, to unix seconds.
pub fn to_unix(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_f64() {
        return Some(n as i64);
    }
    let s = v.as_str()?.trim();
    let num = |a: usize, b: usize| s.get(a..b).and_then(|x| x.parse::<i64>().ok());
    let (y, mo, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    if s.len() == 10 {
        return Some(days_from_civil(y, mo as u32, d as u32) * 86_400);
    }
    let (h, mi) = (num(11, 13)?, num(14, 16)?);
    let sec = num(17, 19).unwrap_or(0);
    let rest = s.get(19..).unwrap_or("").trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "" | "Z" | "z" => 0,
        r => {
            let sign = if r.starts_with('-') { -1 } else { 1 };
            let r = r.trim_start_matches(['+', '-']).replace(':', "");
            sign * (r.get(0..2)?.parse::<i64>().ok()? * 3600 + r.get(2..4).and_then(|x| x.parse::<i64>().ok()).unwrap_or(0) * 60)
        }
    };
    Some(days_from_civil(y, mo as u32, d as u32) * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

fn segment(part: &str, name: &str) -> Result<Value, String> {
    let bytes = base64url(part).ok_or_else(|| format!("the {name} is not base64url"))?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(format!("the {name} is not a JSON object")),
    }
}

/// Decodes the token in `_options.field` (default `token`; a "Bearer " prefix is fine) into
/// `header`, `claims` and readable fields. With `_options.now`, adds `expired`, `not_yet_valid`
/// and `valid_now`.
fn decode_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("token").to_owned();
    let leeway = opt.get("leeway_s").and_then(Value::as_i64).unwrap_or(0);
    let fail = |mut m: Map<String, Value>, e: String| {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(e));
        Value::Object(m)
    };
    let Some(raw) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        return fail(m, format!("field '{field}' is missing or not text"));
    };
    let token = raw.trim();
    let token = token.strip_prefix("Bearer ").or_else(|| token.strip_prefix("bearer ")).unwrap_or(token).trim();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() == 5 {
        return fail(m, "this is an encrypted token (JWE); its claims can't be read without the key".into());
    }
    if parts.len() != 3 {
        return fail(m, format!("a JWT has 3 parts separated by dots; this has {}", parts.len()));
    }
    let (header, claims) = match (segment(parts[0], "header"), segment(parts[1], "payload")) {
        (Ok(h), Ok(c)) => (h, c),
        (Err(e), _) | (_, Err(e)) => return fail(m, e),
    };
    if !opt.get("keep_source").and_then(Value::as_bool).unwrap_or(false) {
        m.remove(&field);
    }

    let alg = header.get("alg").and_then(Value::as_str).unwrap_or("").to_owned();
    let mut warnings: Vec<String> = Vec::new();
    if alg.eq_ignore_ascii_case("none") || parts[2].is_empty() {
        warnings.push("the token is unsigned (alg \"none\" or no signature); never trust its claims".into());
    }
    let time = |k: &str| claims.get(k).and_then(Value::as_f64).map(|f| f as i64);
    let (exp, nbf, iat) = (time("exp"), time("nbf"), time("iat"));
    if exp.is_none() {
        warnings.push("the token has no expiry (exp)".into());
    }
    let audience = match claims.get("aud") {
        Some(Value::String(s)) => json!([s]),
        Some(Value::Array(a)) => json!(a),
        _ => json!([]),
    };

    m.insert("header".into(), header.clone());
    m.insert("claims".into(), claims.clone());
    m.insert("algorithm".into(), json!(alg));
    m.insert("key_id".into(), header.get("kid").cloned().unwrap_or(Value::Null));
    m.insert("issuer".into(), claims.get("iss").cloned().unwrap_or(Value::Null));
    m.insert("subject".into(), claims.get("sub").cloned().unwrap_or(Value::Null));
    m.insert("audience".into(), audience);
    m.insert("issued_at".into(), iat.map(|t| json!(iso(t))).unwrap_or(Value::Null));
    m.insert("not_before".into(), nbf.map(|t| json!(iso(t))).unwrap_or(Value::Null));
    m.insert("expires_at".into(), exp.map(|t| json!(iso(t))).unwrap_or(Value::Null));
    if let (Some(e), Some(i)) = (exp, iat) {
        m.insert("lifetime_s".into(), json!(e - i));
    }

    // WebAssembly modules have no clock, so expiry checks use the time the flow passes in.
    match opt.get("now").and_then(to_unix) {
        Some(now) => {
            let expired = exp.map(|e| now >= e + leeway).unwrap_or(false);
            let early = nbf.map(|n| now + leeway < n).unwrap_or(false);
            m.insert("expired".into(), json!(expired));
            m.insert("not_yet_valid".into(), json!(early));
            m.insert("valid_now".into(), json!(!expired && !early));
            if let Some(e) = exp {
                m.insert("expires_in_s".into(), json!(e - now));
            }
        }
        None => {
            m.insert("expired".into(), Value::Null);
            m.insert("valid_now".into(), Value::Null);
        }
    }
    m.insert("signature_verified".into(), json!(false));
    m.insert("warnings".into(), json!(warnings));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(v: &Value) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = serde_json::to_vec(v).unwrap();
        let mut s = String::new();
        for c in bytes.chunks(3) {
            let n = c.iter().enumerate().fold(0u32, |a, (i, b)| a | (*b as u32) << (16 - 8 * i));
            for i in 0..=c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            }
        }
        s
    }

    fn token(h: Value, c: Value, sig: &str) -> String {
        format!("{}.{}.{}", b64(&h), b64(&c), sig)
    }

    #[test]
    fn dates() {
        assert_eq!(iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso(1_762_707_600), "2025-11-09T17:00:00Z");
        assert_eq!(iso(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(to_unix(&json!("2025-11-09T12:00:00-05:00")), Some(1_762_707_600));
        assert_eq!(to_unix(&json!("2025-11-09T17:00:00.250Z")), Some(1_762_707_600));
        assert_eq!(to_unix(&json!("2000-02-29")), Some(951_782_400));
        assert_eq!(to_unix(&json!(12)), Some(12));
        assert_eq!(base64url("aGk").unwrap(), b"hi");
    }

    #[test]
    fn decodes_and_checks_expiry() {
        let t = token(
            json!({ "alg": "RS256", "typ": "JWT", "kid": "key-2026-11" }),
            json!({ "iss": "https://login.example.com/", "sub": "user-4471", "aud": "api://orders", "iat": 1_762_707_600, "exp": 1_762_711_200, "scope": "orders.read", "email": "jane@example.com" }),
            "c2lnbmF0dXJl",
        );
        let out = run(serde_json::to_vec(&json!({ "token": format!("Bearer {t}"), "_options": { "now": "2025-11-09T17:30:00Z" } })).unwrap().as_slice());
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["algorithm"], json!("RS256"));
        assert_eq!(out["key_id"], json!("key-2026-11"));
        assert_eq!(out["audience"], json!(["api://orders"]));
        assert_eq!(out["issued_at"], json!("2025-11-09T17:00:00Z"));
        assert_eq!(out["expires_at"], json!("2025-11-09T18:00:00Z"));
        assert_eq!(out["lifetime_s"], json!(3600));
        assert_eq!(out["expired"], json!(false));
        assert_eq!(out["valid_now"], json!(true));
        assert_eq!(out["expires_in_s"], json!(1800));
        assert_eq!(out["claims"]["scope"], json!("orders.read"));
        assert_eq!(out["signature_verified"], json!(false));
        assert_eq!(out["warnings"], json!([]));
        assert!(out.get("token").is_none());

        let later = run(serde_json::to_vec(&json!({ "token": t, "_options": { "now": 1_762_711_200 } })).unwrap().as_slice());
        assert_eq!(later["expired"], json!(true));
        let leeway = run(serde_json::to_vec(&json!({ "token": t, "_options": { "now": 1_762_711_200, "leeway_s": 60 } })).unwrap().as_slice());
        assert_eq!(leeway["expired"], json!(false));
        let no_clock = run(serde_json::to_vec(&json!({ "token": t })).unwrap().as_slice());
        assert_eq!(no_clock["expired"], Value::Null);
    }

    #[test]
    fn warnings_and_errors() {
        let unsigned = token(json!({ "alg": "none" }), json!({ "sub": "x", "nbf": 2_000_000_000 }), "");
        let out = run(serde_json::to_vec(&json!({ "token": unsigned, "_options": { "now": 1_762_707_600 } })).unwrap().as_slice());
        assert_eq!(out["warnings"].as_array().unwrap().len(), 2);
        assert_eq!(out["not_yet_valid"], json!(true));
        assert_eq!(out["valid_now"], json!(false));
        assert_eq!(run(br#"{"token": "not-a-token"}"#)["ok"], json!(false));
        assert_eq!(run(br#"{"token": "a.b.c.d.e"}"#)["error"].as_str().unwrap().contains("encrypted"), true);
        assert_eq!(run(br#"{"token": "!!!.e30.x"}"#)["ok"], json!(false));
        assert_eq!(run(br#"{"token": "e30.bm90IGpzb24.x"}"#)["error"], json!("the payload is not a JSON object"));
        assert_eq!(run(br#"{"jwt": 1}"#)["ok"], json!(false));
    }
}
