//! Neblex sandboxed module: split a scanned GS1 barcode (GS1-128, GS1 DataMatrix, GS1 QR) into fields.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the supported Application Identifiers.

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

#[derive(Clone, Copy, PartialEq)]
enum F {
    /// Numeric, fixed length.
    N(usize),
    /// Variable length up to n, ends at a group separator.
    V(usize),
    /// YYMMDD date.
    Date,
    /// Measure: 6 digits, decimal places in the AI's last digit.
    Measure,
    /// Amount: up to 15 digits, decimal places in the AI's last digit.
    Amount,
    /// ISO 4217 numeric currency (3 digits) then an amount.
    CurrencyAmount,
}

/// (AI, key, format, check digit). Four-digit measure AIs are matched by their first three digits.
const AIS: &[(&str, &str, F, bool)] = &[
    ("00", "sscc", F::N(18), true),
    ("01", "gtin", F::N(14), true),
    ("02", "content_gtin", F::N(14), true),
    ("10", "batch", F::V(20), false),
    ("11", "production_date", F::Date, false),
    ("12", "due_date", F::Date, false),
    ("13", "packaging_date", F::Date, false),
    ("15", "best_before", F::Date, false),
    ("16", "sell_by", F::Date, false),
    ("17", "expiry", F::Date, false),
    ("20", "variant", F::N(2), false),
    ("21", "serial", F::V(20), false),
    ("22", "consumer_product_variant", F::V(20), false),
    ("30", "variable_count", F::V(8), false),
    ("37", "count", F::V(8), false),
    ("240", "additional_product_id", F::V(30), false),
    ("241", "customer_part_number", F::V(30), false),
    ("250", "secondary_serial", F::V(30), false),
    ("400", "customer_po", F::V(30), false),
    ("401", "consignment_number", F::V(30), false),
    ("402", "shipment_id", F::N(17), true),
    ("410", "ship_to_gln", F::N(13), true),
    ("411", "bill_to_gln", F::N(13), true),
    ("412", "purchased_from_gln", F::N(13), true),
    ("413", "ship_for_gln", F::N(13), true),
    ("414", "location_gln", F::N(13), true),
    ("415", "invoicing_party_gln", F::N(13), true),
    ("420", "ship_to_postal_code", F::V(20), false),
    ("422", "country_of_origin", F::N(3), false),
    ("310", "net_weight_kg", F::Measure, false),
    ("311", "length_m", F::Measure, false),
    ("312", "width_m", F::Measure, false),
    ("313", "height_m", F::Measure, false),
    ("315", "net_volume_l", F::Measure, false),
    ("320", "net_weight_lb", F::Measure, false),
    ("330", "gross_weight_kg", F::Measure, false),
    ("340", "gross_weight_lb", F::Measure, false),
    ("392", "price", F::Amount, false),
    ("393", "price_with_currency", F::CurrencyAmount, false),
    ("8020", "payment_slip_reference", F::V(25), false),
];

const GS: char = '\u{1d}';

fn lookup(s: &str) -> Option<(String, &'static str, F, bool)> {
    // Internal AIs 90-99 are free text the company defines.
    if s.len() >= 2 && s.starts_with('9') && s.as_bytes()[1].is_ascii_digit() {
        let ai = &s[..2];
        return Some((ai.to_owned(), if ai == "90" { "internal_90" } else { ["internal_91", "internal_92", "internal_93", "internal_94", "internal_95", "internal_96", "internal_97", "internal_98", "internal_99"][(s.as_bytes()[1] - b'1') as usize] }, F::V(if ai == "90" { 30 } else { 90 }), false));
    }
    for &(ai, key, f, check) in AIS {
        let len = if matches!(f, F::Measure | F::Amount | F::CurrencyAmount) { 4 } else { ai.len() };
        if s.len() >= len && s.starts_with(ai) && s.as_bytes()[len - 1].is_ascii_digit() {
            return Some((s[..len].to_owned(), key, f, check));
        }
    }
    None
}

fn check_digit_ok(d: &str) -> bool {
    let digits: Vec<u32> = d.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != d.len() || digits.is_empty() {
        return false;
    }
    let (body, check) = digits.split_at(digits.len() - 1);
    let sum: u32 = body.iter().rev().enumerate().map(|(i, v)| if i % 2 == 0 { v * 3 } else { *v }).sum();
    (10 - sum % 10) % 10 == check[0]
}

/// YYMMDD to ISO. Day 00 means the last day of the month. Years 00-49 are 20xx, 50-99 19xx
/// (the module has no clock, so it can't apply GS1's sliding century window).
fn date(v: &str) -> Result<String, String> {
    let n = |r: std::ops::Range<usize>| v.get(r).and_then(|x| x.parse::<u32>().ok());
    let (yy, mm, dd) = (n(0..2).ok_or("bad date")?, n(2..4).ok_or("bad date")?, n(4..6).ok_or("bad date")?);
    let y = if yy < 50 { 2000 + yy } else { 1900 + yy };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&mm) {
        return Err(format!("'{v}' has month {mm}"));
    }
    let last = days[mm as usize - 1];
    let d = if dd == 0 { last } else { dd };
    if d > last {
        return Err(format!("'{v}' has day {dd}"));
    }
    Ok(format!("{y:04}-{mm:02}-{d:02}"))
}

fn decimal(digits: &str, places: u32) -> Value {
    let n: u64 = digits.parse().unwrap_or(0);
    if places == 0 {
        json!(n)
    } else {
        json!(n as f64 / 10f64.powi(places as i32))
    }
}

/// Splits element strings in either raw form (FNC1 sent as the GS character) or
/// human-readable form with parentheses: "(01)09506000134352(17)261231(10)AB-123".
pub fn parse(scan: &str) -> Value {
    let mut s = scan.trim().to_owned();
    // Symbology identifier from the scanner, e.g. ]C1 (GS1-128), ]d2 (DataMatrix), ]Q3 (QR), ]e0 (DataBar).
    if s.starts_with(']') && s.len() > 3 {
        s = s[3..].to_owned();
    }
    let s = s.replace("<GS>", "\u{1d}").replace("{GS}", "\u{1d}").trim_start_matches(GS).to_owned();
    let mut elements = Vec::new();
    let mut fields = Map::new();
    let mut errors = Vec::new();
    if s.starts_with('(') {
        let mut rest = s.as_str();
        while let Some(r) = rest.strip_prefix('(') {
            let Some(close) = r.find(')') else {
                errors.push(json!({ "error": "unclosed '(' in human-readable barcode" }));
                break;
            };
            let ai = &r[..close];
            let after = &r[close + 1..];
            let end = after.find('(').unwrap_or(after.len());
            let value = after[..end].trim_end_matches(GS);
            match lookup(&format!("{ai}{value}")).filter(|(a, ..)| a == ai) {
                Some(def) => element(def, value, &mut elements, &mut fields, &mut errors),
                None => errors.push(json!({ "ai": ai, "error": "unknown or unsupported Application Identifier" })),
            }
            rest = &after[end..];
        }
    } else {
        let mut rest = s.as_str();
        while !rest.is_empty() {
            let Some(def) = lookup(rest) else {
                errors.push(json!({ "at": s.len() - rest.len(), "error": format!("unknown Application Identifier at '{}'", rest.chars().take(6).collect::<String>()) }));
                break;
            };
            let body = &rest[def.0.len()..];
            let len = match def.2 {
                F::N(n) => n.min(body.len()),
                F::Date | F::Measure => 6.min(body.len()),
                F::V(_) | F::Amount | F::CurrencyAmount => body.find(GS).unwrap_or(body.len()),
            };
            let value = &body[..len];
            let mut next = &body[len..];
            next = next.strip_prefix(GS).unwrap_or(next);
            element(def, value, &mut elements, &mut fields, &mut errors);
            rest = next;
        }
    }
    json!({ "gs1": fields, "elements": elements, "errors": errors, "_valid": errors.is_empty() && !elements.is_empty() })
}

fn element(def: (String, &'static str, F, bool), value: &str, elements: &mut Vec<Value>, fields: &mut Map<String, Value>, errors: &mut Vec<Value>) {
    let (ai, key, f, check) = def;
    let places = ai.chars().last().and_then(|c| c.to_digit(10)).unwrap_or(0);
    let numeric = value.chars().all(|c| c.is_ascii_digit());
    let parsed: Result<Value, String> = match f {
        F::N(n) if value.len() != n || !numeric => Err(format!("expected {n} digits, got '{value}'")),
        F::N(_) if check && !check_digit_ok(value) => Err(format!("check digit of '{value}' is wrong")),
        F::N(_) => Ok(json!(value)),
        F::V(max) if value.is_empty() || value.chars().count() > max => Err(format!("expected 1 to {max} characters, got '{value}'")),
        F::V(_) => Ok(json!(value)),
        F::Date if value.len() != 6 || !numeric => Err(format!("expected YYMMDD, got '{value}'")),
        F::Date => date(value).map(|d| json!(d)),
        F::Measure if value.len() != 6 || !numeric => Err(format!("expected 6 digits, got '{value}'")),
        F::Measure => Ok(decimal(value, places)),
        F::Amount if value.is_empty() || value.len() > 15 || !numeric => Err(format!("expected up to 15 digits, got '{value}'")),
        F::Amount => Ok(decimal(value, places)),
        F::CurrencyAmount if value.len() < 4 || value.len() > 18 || !numeric => Err(format!("expected a 3-digit currency and an amount, got '{value}'")),
        F::CurrencyAmount => Ok(json!({ "currency_numeric": &value[..3], "amount": decimal(&value[3..], places) })),
    };
    match parsed {
        Ok(v) => {
            elements.push(json!({ "ai": ai, "name": key, "value": v, "raw": value }));
            fields.insert(key.to_owned(), v);
        }
        Err(e) => errors.push(json!({ "ai": ai, "name": key, "error": e })),
    }
}

/// Reads the scan from `_options.field` (default `barcode`) and adds `gs1` (fields by name),
/// `elements` (in scan order, with AI and raw value), `errors` and `_valid`.
fn parse_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("barcode").to_owned();
    let Some(scan) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    m.extend(parse(&scan).as_object().unwrap().clone());
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_readable() {
        let v = parse("(01)09506000134352(17)261231(10)AB-123(21)SN0001");
        assert_eq!(v["_valid"], json!(true), "{v}");
        assert_eq!(v["gs1"], json!({ "gtin": "09506000134352", "expiry": "2026-12-31", "batch": "AB-123", "serial": "SN0001" }));
    }

    #[test]
    fn raw_scan_with_fnc1_and_symbology_id() {
        let v = parse("]C101095060001343521728020010LOT77\u{1d}3103001250\u{1d}21XYZ");
        assert_eq!(v["_valid"], json!(true), "{v}");
        assert_eq!(v["gs1"]["expiry"], json!("2028-02-29"));
        assert_eq!(v["gs1"]["batch"], json!("LOT77"));
        assert_eq!(v["gs1"]["net_weight_kg"], json!(1.25));
        assert_eq!(v["gs1"]["serial"], json!("XYZ"));
        let ais: Vec<_> = v["elements"].as_array().unwrap().iter().map(|e| e["ai"].clone()).collect();
        assert_eq!(ais, vec![json!("01"), json!("17"), json!("10"), json!("3103"), json!("21")]);
    }

    #[test]
    fn sscc_logistics_label_and_day_zero() {
        let v = parse("(00)106141411234567897(15)260200(400)PO-4411(410)0614141000005");
        assert!(v["errors"].as_array().unwrap().iter().all(|e| e["ai"] != "00"), "{v}");
        assert_eq!(v["gs1"]["best_before"], json!("2026-02-28"));
        assert_eq!(v["gs1"]["customer_po"], json!("PO-4411"));
    }

    #[test]
    fn check_digits_and_unknown_ais() {
        let v = parse("(01)09506000134353(17)261331");
        let errs = v["errors"].as_array().unwrap();
        assert_eq!(errs.len(), 2, "{v}");
        assert_eq!(v["_valid"], json!(false));
        let u = parse("(55)123");
        assert_eq!(u["errors"][0]["ai"], json!("55"));
    }

    #[test]
    fn price_with_currency_and_record_interface() {
        let out = run(br#"{"scan": "(01)09506000134352(3932)8401299", "_options": {"field": "scan"}}"#);
        assert_eq!(out["gs1"]["price_with_currency"], json!({ "currency_numeric": "840", "amount": 12.99 }));
        assert_eq!(run(br#"{"other": 1}"#)["ok"], json!(false));
    }
}
