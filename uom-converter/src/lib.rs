//! Neblex sandboxed module: convert quantities between units of measure, including your own pack
//! sizes (a case of 12, a pallet of 40 cases), the way ERP, warehouse and order systems disagree.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the units and options.

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

/// Built-in units: (code, dimension, size in the dimension's base unit, aliases).
/// Base units: G for mass, M for length, L for volume, EA for count.
const UNITS: &[(&str, &str, f64, &[&str])] = &[
    ("MG", "mass", 0.001, &["milligram", "milligrams"]),
    ("G", "mass", 1.0, &["gram", "grams", "grm"]),
    ("KG", "mass", 1000.0, &["kgs", "kilo", "kilos", "kilogram", "kilograms"]),
    ("T", "mass", 1_000_000.0, &["tonne", "tonnes", "metric ton", "metric tons", "mt"]),
    ("OZ", "mass", 28.349523125, &["ounce", "ounces"]),
    ("LB", "mass", 453.59237, &["lbs", "pound", "pounds", "#"]),
    ("MM", "length", 0.001, &["millimeter", "millimeters", "millimetre", "millimetres"]),
    ("CM", "length", 0.01, &["centimeter", "centimeters", "centimetre", "centimetres"]),
    ("M", "length", 1.0, &["meter", "meters", "metre", "metres", "mtr"]),
    ("KM", "length", 1000.0, &["kilometer", "kilometers", "kilometre", "kilometres"]),
    ("IN", "length", 0.0254, &["inch", "inches", "\""]),
    ("FT", "length", 0.3048, &["foot", "feet", "'"]),
    ("YD", "length", 0.9144, &["yard", "yards"]),
    ("MI", "length", 1609.344, &["mile", "miles"]),
    ("ML", "volume", 0.001, &["milliliter", "milliliters", "millilitre", "millilitres"]),
    ("L", "volume", 1.0, &["ltr", "liter", "liters", "litre", "litres"]),
    ("M3", "volume", 1000.0, &["cubic meter", "cubic meters", "cbm"]),
    ("FLOZ", "volume", 0.0295735295625, &["fl oz", "fluid ounce", "fluid ounces"]),
    ("PT", "volume", 0.473176473, &["pint", "pints"]),
    ("QT", "volume", 0.946352946, &["quart", "quarts"]),
    ("GAL", "volume", 3.785411784, &["gallon", "gallons", "us gal"]),
    ("EA", "count", 1.0, &["each", "pc", "pcs", "piece", "pieces", "unit", "units", "count"]),
    ("PR", "count", 2.0, &["pair", "pairs"]),
    ("DZ", "count", 12.0, &["dozen", "doz"]),
    ("GRO", "count", 144.0, &["gross"]),
];

/// Upper-case code for a unit name or alias; unknown units come back upper-cased so pack sizes
/// (CS, BX, PLT ...) can use any code.
fn code(u: &str) -> String {
    let k = u.trim().trim_end_matches('.').to_lowercase();
    for (c, _, _, aliases) in UNITS {
        if c.to_lowercase() == k || aliases.contains(&k.as_str()) {
            return (*c).to_owned();
        }
    }
    k.to_uppercase()
}

fn builtin(c: &str) -> Option<(&'static str, f64)> {
    UNITS.iter().find(|(u, ..)| *u == c).map(|(_, d, f, _)| (*d, *f))
}

/// How many `to` are in one `from`, using built-in units and `packs` ({"CS": {"EA": 12}} means
/// one CS holds 12 EA). Pack sizes chain both ways and through built-in units.
pub fn factor(from: &str, to: &str, packs: &[(String, String, f64)]) -> Result<f64, String> {
    let (f, t) = (code(from), code(to));
    if f == t {
        return Ok(1.0);
    }
    // Search over units, each step multiplying the factor.
    let mut seen: Vec<(String, f64)> = vec![(f.clone(), 1.0)];
    let mut queue = vec![(f.clone(), 1.0)];
    while let Some((u, k)) = queue.pop() {
        let mut next: Vec<(String, f64)> = Vec::new();
        for (a, b, n) in packs {
            if *a == u {
                next.push((b.clone(), k * n));
            }
            if *b == u {
                next.push((a.clone(), k / n));
            }
        }
        if let Some((dim, size)) = builtin(&u) {
            for (c, d, s, _) in UNITS {
                if *d == dim && *c != u {
                    next.push(((*c).to_owned(), k * size / s));
                }
            }
        }
        for (v, kv) in next {
            if v == t {
                return Ok(kv);
            }
            if !seen.iter().any(|(s, _)| *s == v) {
                seen.push((v.clone(), kv));
                queue.push((v, kv));
            }
        }
    }
    match (builtin(&f), builtin(&t)) {
        (Some((a, _)), Some((b, _))) => Err(format!("can't convert {a} ({f}) to {b} ({t})")),
        _ => Err(format!("no conversion from {f} to {t}; add a pack size such as {{\"{f}\": {{\"EA\": 12}}}}")),
    }
}

fn packs(v: Option<&Value>) -> Result<Vec<(String, String, f64)>, String> {
    let mut out = Vec::new();
    let Some(m) = v.and_then(Value::as_object) else { return Ok(out) };
    for (outer, inner) in m {
        let Some(inner) = inner.as_object() else { return Err(format!("packs.{outer} must be an object such as {{\"EA\": 12}}")) };
        for (unit, n) in inner {
            let n = n.as_f64().filter(|n| *n > 0.0).ok_or(format!("packs.{outer}.{unit} must be a positive number"))?;
            out.push((code(outer), code(unit), n));
        }
    }
    Ok(out)
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().replace(',', "").parse().ok(),
        _ => None,
    }
}

fn round(x: f64, places: Option<u32>) -> Value {
    let Some(p) = places else { return json!(x) };
    let f = 10f64.powi(p as i32);
    let r = (x * f).round() / f;
    if p == 0 {
        json!(r as i64)
    } else {
        json!(r)
    }
}

/// Converts `quantity` from `from` to `to` (field names set in `_options`) and adds
/// `converted`, `factor` and `_valid`; or, with `_options.lines`, converts each entry of that list.
fn convert_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let s = |k: &str, d: &str| opt.get(k).and_then(Value::as_str).unwrap_or(d).to_owned();
    let (qf, ff, tf, into) = (s("quantity_field", "quantity"), s("from_field", "from"), s("to_field", "to"), s("into", "converted"));
    let places = opt.get("places").and_then(Value::as_u64).map(|p| p as u32);
    let fixed_to = opt.get("to").and_then(Value::as_str).map(str::to_owned);
    let pk = match packs(opt.get("packs")) {
        Ok(p) => p,
        Err(e) => {
            m.insert("ok".into(), json!(false));
            m.insert("error".into(), json!(e));
            return Value::Object(m);
        }
    };
    let one = |r: &mut Map<String, Value>| {
        let q = r.get(&qf).and_then(number);
        let from = r.get(&ff).and_then(Value::as_str).map(str::to_owned);
        let to = fixed_to.clone().or_else(|| r.get(&tf).and_then(Value::as_str).map(str::to_owned));
        let res = match (q, from, to) {
            (None, ..) => Err(format!("'{qf}' is missing or not a number")),
            (_, None, _) => Err(format!("'{ff}' is missing")),
            (_, _, None) => Err(format!("'{tf}' is missing")),
            (Some(q), Some(f), Some(t)) => factor(&f, &t, &pk).map(|k| (q * k, k, code(&t))),
        };
        match res {
            Ok((v, k, t)) => {
                r.insert(into.clone(), round(v, places));
                r.insert(format!("{into}_unit"), json!(t));
                r.insert("factor".into(), json!(k));
                r.insert("_valid".into(), json!(true));
            }
            Err(e) => {
                r.insert(into.clone(), Value::Null);
                r.insert("_valid".into(), json!(false));
                r.insert("error".into(), json!(e));
            }
        }
    };
    if let Some(lf) = opt.get("lines").and_then(Value::as_str) {
        let Some(Value::Array(lines)) = m.get_mut(lf) else {
            m.insert("ok".into(), json!(false));
            m.insert("error".into(), json!(format!("'{lf}' is missing or not a list")));
            return Value::Object(m);
        };
        let mut all_valid = true;
        for l in lines.iter_mut() {
            if let Value::Object(lm) = l {
                one(lm);
                all_valid &= lm.get("_valid") == Some(&json!(true));
            }
        }
        m.insert("_valid".into(), json!(all_valid));
    } else {
        one(&mut m);
    }
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    #[test]
    fn builtin_units() {
        let k = factor("kg", "lbs", &[]).unwrap();
        assert!((k - 2.2046226218).abs() < 1e-9);
        assert_eq!(factor("dozen", "each", &[]).unwrap(), 12.0);
        assert!((factor("gallon", "L", &[]).unwrap() - 3.785411784).abs() < 1e-12);
        assert!((factor("ft", "in", &[]).unwrap() - 12.0).abs() < 1e-12);
        assert!(factor("kg", "m", &[]).unwrap_err().contains("can't convert mass"));
    }

    #[test]
    fn packs_chain_both_ways() {
        let p = packs(Some(&json!({ "CS": { "EA": 12 }, "PLT": { "CS": 40 }, "BX": { "EA": 6 } }))).unwrap();
        assert_eq!(factor("PLT", "EA", &p).unwrap(), 480.0);
        assert_eq!(factor("EA", "CS", &p).unwrap(), 1.0 / 12.0);
        assert_eq!(factor("CS", "BX", &p).unwrap(), 2.0);
        assert_eq!(factor("PLT", "DZ", &p).unwrap(), 40.0);
        assert!(factor("CS", "KG", &p).is_err());
        assert!(factor("XX", "EA", &p).unwrap_err().contains("add a pack size"));
    }

    #[test]
    fn records_and_lines() {
        let out = go(json!({ "quantity": "2.5", "from": "KG", "to": "lb", "_options": { "places": 2 } }));
        assert_eq!(out["converted"], json!(5.51));
        assert_eq!(out["converted_unit"], json!("LB"));
        let order = go(json!({
            "order": "SO-1001",
            "lines": [ { "sku": "W-9", "qty": 3, "uom": "CS" }, { "sku": "B-2", "qty": 30, "uom": "EA" }, { "sku": "Z", "qty": 1, "uom": "KG" } ],
            "_options": { "lines": "lines", "quantity_field": "qty", "from_field": "uom", "to": "EA", "into": "qty_ea", "packs": { "CS": { "EA": 12 } }, "places": 0 }
        }));
        assert_eq!(order["lines"][0]["qty_ea"], json!(36));
        assert_eq!(order["lines"][1]["qty_ea"], json!(30));
        assert_eq!(order["lines"][2]["_valid"], json!(false));
        assert_eq!(order["_valid"], json!(false));
        assert_eq!(go(json!({ "quantity": 1, "from": "EA", "to": "EA", "_options": { "packs": { "CS": { "EA": -1 } } } }))["ok"], json!(false));
    }
}
