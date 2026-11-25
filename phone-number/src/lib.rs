//! Neblex sandboxed module: read a phone number in any common format and tell what it is: valid
//! or not under its country's numbering plan, the country it belongs to, its type (mobile,
//! landline, toll-free, premium rate, VoIP...), its extension, and E.164, international,
//! national and tel: formats. Rules come from Google's libphonenumber metadata.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the fields.

use phonenumber::{country, Mode, PhoneNumber, Type};
use serde_json::{json, Map, Value};
use std::str::FromStr;

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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(check_record).collect()),
        Ok(v) => check_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// Snake-case name of a number type ("mobile", "fixed_line_or_mobile", "toll_free").
pub fn type_name(t: Type) -> &'static str {
    match t {
        Type::FixedLine => "fixed_line",
        Type::Mobile => "mobile",
        Type::FixedLineOrMobile => "fixed_line_or_mobile",
        Type::TollFree => "toll_free",
        Type::PremiumRate => "premium_rate",
        Type::SharedCost => "shared_cost",
        Type::PersonalNumber => "personal",
        Type::Voip => "voip",
        Type::Pager => "pager",
        Type::Uan => "uan",
        Type::Emergency => "emergency",
        Type::Voicemail => "voicemail",
        Type::ShortCode => "short_code",
        Type::StandardRate => "standard_rate",
        Type::Carrier => "carrier",
        Type::NoInternational => "no_international",
        Type::Unknown => "unknown",
    }
}

/// Parses `text` with `region` ("US", "GB"...) as the country for numbers without a "+" or
/// international prefix, and describes it ("(415) 555-2671", US -> e164 "+14155552671").
pub fn describe(text: &str, region: Option<country::Id>) -> Map<String, Value> {
    let mut o = Map::new();
    let digits = text.chars().filter(|c| c.is_ascii_alphanumeric()).count();
    if digits == 0 {
        o.insert("valid".into(), json!(false));
        o.insert("reason".into(), json!("empty"));
        return o;
    }
    let n: PhoneNumber = match phonenumber::parse(region, text) {
        Ok(n) => n,
        Err(e) => {
            let reason = match e {
                phonenumber::ParseError::InvalidCountryCode if region.is_none() && !text.trim_start().starts_with('+') => {
                    "no country: add a + and country code, or set _options.region".to_owned()
                }
                other => other.to_string(),
            };
            o.insert("valid".into(), json!(false));
            o.insert("reason".into(), json!(reason));
            return o;
        }
    };
    let valid = n.is_valid();
    let db = &*phonenumber::metadata::DATABASE;
    let fmt = |m: Mode| n.format().mode(m).to_string();
    o.insert("valid".into(), json!(valid));
    if !valid {
        o.insert("reason".into(), json!("not a valid number under the country's numbering plan"));
    }
    o.insert("e164".into(), json!(fmt(Mode::E164)));
    o.insert("international".into(), json!(fmt(Mode::International)));
    o.insert("national".into(), json!(fmt(Mode::National)));
    o.insert("rfc3966".into(), json!(fmt(Mode::Rfc3966)));
    o.insert("country_code".into(), json!(n.code().value()));
    o.insert("region".into(), json!(n.country().id().map(|c| c.as_ref().to_owned())));
    o.insert("type".into(), json!(if valid { type_name(n.number_type(db)) } else { "unknown" }));
    o.insert("extension".into(), json!(n.extension().map(|e| e.to_string())));
    o
}

/// Reads `_options.field` (default `phone`) and adds `phone_info` (or `_options.into`);
/// `_options.region` is the country for numbers written without a "+".
fn check_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("phone").to_owned();
    let into = opt.get("into").and_then(Value::as_str).unwrap_or("phone_info").to_owned();
    let region = match opt.get("region").and_then(Value::as_str).map(|r| r.trim().to_uppercase()) {
        None => None,
        Some(r) if r.is_empty() => None,
        Some(r) => match country::Id::from_str(&r) {
            Ok(id) => Some(id),
            Err(_) => return fail(m, format!("unknown region '{r}': use a two-letter country code such as US or GB")),
        },
    };
    let text = match m.get(&field) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Null) | None => {
            m.insert(into, json!({ "valid": false, "reason": "empty" }));
            m.insert("ok".into(), json!(true));
            return Value::Object(m);
        }
        Some(_) => return fail(m, format!("field '{field}' is not text")),
    };
    m.insert(into, Value::Object(describe(&text, region)));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

fn fail(mut m: Map<String, Value>, error: String) -> Value {
    m.insert("ok".into(), json!(false));
    m.insert("error".into(), json!(error));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }
    fn info(phone: &str, region: &str) -> Value {
        go(json!({ "phone": phone, "_options": { "region": region } }))["phone_info"].clone()
    }

    #[test]
    fn formats_and_regions() {
        let us = info("(415) 555-2671", "US");
        assert_eq!(us["valid"], json!(true));
        assert_eq!(us["e164"], json!("+14155552671"));
        assert_eq!(us["international"], json!("+1 415-555-2671"));
        assert_eq!(us["national"], json!("(415) 555-2671"));
        assert_eq!(us["rfc3966"], json!("tel:+1-415-555-2671"));
        assert_eq!(us["country_code"], json!(1));
        assert_eq!(us["region"], json!("US"));
        assert_eq!(us["type"], json!("fixed_line_or_mobile"));
        // Canada shares +1: the area code decides the region.
        assert_eq!(info("+1 416 555 0199", "US")["region"], json!("CA"));
        let gb = info("020 7946 0958", "GB");
        assert_eq!(gb["e164"], json!("+442079460958"));
        assert_eq!(gb["national"], json!("020 7946 0958"));
        assert_eq!(gb["type"], json!("fixed_line"));
        // The region option is only a default: a + number keeps its own country.
        assert_eq!(info("+49 30 1234567", "US")["region"], json!("DE"));
        assert_eq!(info("0049 30 1234567", "GB")["region"], json!("DE"));
    }

    #[test]
    fn types() {
        assert_eq!(info("+44 7400 123456", "")["type"], json!("mobile"));
        // +44 7911 is a Guernsey range in the numbering data.
        assert_eq!(info("+44 7911 123456", "")["region"], json!("GG"));
        assert_eq!(info("+49 151 23456789", "")["type"], json!("mobile"));
        assert_eq!(info("+33 6 12 34 56 78", "")["type"], json!("mobile"));
        assert_eq!(info("+61 412 345 678", "")["type"], json!("mobile"));
        assert_eq!(info("1-800-FLOWERS", "US")["type"], json!("toll_free"));
        assert_eq!(info("1-800-FLOWERS", "US")["e164"], json!("+18003569377"));
        assert_eq!(info("0800 123 4567", "GB")["type"], json!("toll_free"));
        assert_eq!(info("+1 900 555 0100", "")["type"], json!("premium_rate"));
        assert_eq!(info("+44 56 1234 5678", "")["type"], json!("voip"));
    }

    #[test]
    fn extensions() {
        let a = info("+61 2 9374 4000 ext 12", "");
        assert_eq!(a["valid"], json!(true));
        assert_eq!(a["extension"], json!("12"));
        assert_eq!(a["e164"], json!("+61293744000"));
        assert_eq!(info("(212) 555-0123 x305", "US")["extension"], json!("305"));
        assert_eq!(info("+44 20 7946 0958", "")["extension"], Value::Null);
    }

    #[test]
    fn must_not_match() {
        // Area codes never start with 1, too long, too short, letters only.
        let bad = info("+1 123 456 7890", "");
        assert_eq!(bad["valid"], json!(false));
        assert_eq!(bad["type"], json!("unknown"));
        assert_eq!(info("+1 212 555 012345", "")["valid"], json!(false));
        assert_eq!(info("12345", "US")["valid"], json!(false));
        assert_eq!(info("call me", "US")["valid"], json!(false));
        assert_eq!(info("", "US")["reason"], json!("empty"));
        // No + and no region: it can't be placed.
        let r = go(json!({ "phone": "020 7946 0958" }));
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["phone_info"]["valid"], json!(false));
        assert!(r["phone_info"]["reason"].as_str().unwrap().contains("region"), "{r}");
    }

    #[test]
    fn options_and_input_handling() {
        let r = go(json!({ "mobile": "07400 123456", "_options": { "field": "mobile", "into": "m", "region": "gb" } }));
        assert_eq!(r["m"]["e164"], json!("+447400123456"));
        assert_eq!(r["mobile"], json!("07400 123456"));
        assert_eq!(go(json!({ "phone": 4155552671u64, "_options": { "region": "US" } }))["phone_info"]["valid"], json!(true));
        assert_eq!(go(json!({ "phone": null }))["phone_info"]["reason"], json!("empty"));
        assert_eq!(go(json!({ "name": "no phone" }))["ok"], json!(true));
        assert_eq!(go(json!({ "phone": "1", "_options": { "region": "XX" } }))["ok"], json!(false));
        assert_eq!(go(json!({ "phone": ["1"] }))["ok"], json!(false));
        assert_eq!(go(json!("text"))["ok"], json!(false));
        assert_eq!(run(b"nope")["ok"], json!(false));
        let r = go(json!([{ "phone": "+44 7400 123456" }, { "phone": "+33 1 23 45 67 89" }]));
        assert_eq!(r[0]["phone_info"]["region"], json!("GB"));
        assert_eq!(r[1]["phone_info"]["region"], json!("FR"));
    }
}
