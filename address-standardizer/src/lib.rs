//! Neblex sandboxed module: standardize postal addresses in the style of USPS Publication 28
//! (abbreviations, casing, punctuation, state and ZIP formats, country codes). It formats
//! addresses; it does not check that an address exists.
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(standardize_record).collect()),
        Ok(v) => standardize_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

const SUFFIXES: [(&str, &str); 62] = [
    ("ALLEY", "ALY"), ("AVENUE", "AVE"), ("AV", "AVE"), ("AVEN", "AVE"), ("BOULEVARD", "BLVD"), ("BOUL", "BLVD"), ("CENTER", "CTR"),
    ("CENTRE", "CTR"), ("CIRCLE", "CIR"), ("COURT", "CT"), ("COVE", "CV"), ("CREEK", "CRK"), ("CROSSING", "XING"), ("DRIVE", "DR"),
    ("DRV", "DR"), ("ESTATES", "ESTS"), ("EXPRESSWAY", "EXPY"), ("FREEWAY", "FWY"), ("GARDENS", "GDNS"), ("GROVE", "GRV"),
    ("HEIGHTS", "HTS"), ("HIGHWAY", "HWY"), ("HIWAY", "HWY"), ("HILL", "HL"), ("HILLS", "HLS"), ("HOLLOW", "HOLW"), ("ISLAND", "IS"),
    ("JUNCTION", "JCT"), ("LAKE", "LK"), ("LANE", "LN"), ("MEADOWS", "MDWS"), ("MOUNT", "MT"), ("MOUNTAIN", "MTN"), ("PARKWAY", "PKWY"),
    ("PKY", "PKWY"), ("PLACE", "PL"), ("PLAZA", "PLZ"), ("POINT", "PT"), ("RIDGE", "RDG"), ("ROAD", "RD"), ("ROUTE", "RTE"),
    ("SPRINGS", "SPGS"), ("SQUARE", "SQ"), ("STREET", "ST"), ("STR", "ST"), ("STRT", "ST"), ("TERRACE", "TER"), ("TRACE", "TRCE"),
    ("TRAIL", "TRL"), ("TURNPIKE", "TPKE"), ("VALLEY", "VLY"), ("VIEW", "VW"), ("VILLAGE", "VLG"), ("WAY", "WAY"), ("LOOP", "LOOP"),
    ("PARK", "PARK"), ("PATH", "PATH"), ("PIKE", "PIKE"), ("RUN", "RUN"), ("WALK", "WALK"), ("PASS", "PASS"), ("ROW", "ROW"),
];
const DIRECTIONS: [(&str, &str); 8] = [
    ("NORTH", "N"), ("SOUTH", "S"), ("EAST", "E"), ("WEST", "W"), ("NORTHEAST", "NE"), ("NORTHWEST", "NW"), ("SOUTHEAST", "SE"), ("SOUTHWEST", "SW"),
];
const UNITS: [(&str, &str); 14] = [
    ("APARTMENT", "APT"), ("APT", "APT"), ("SUITE", "STE"), ("STE", "STE"), ("UNIT", "UNIT"), ("FLOOR", "FL"), ("FL", "FL"),
    ("BUILDING", "BLDG"), ("BLDG", "BLDG"), ("ROOM", "RM"), ("RM", "RM"), ("DEPARTMENT", "DEPT"), ("DEPT", "DEPT"), ("#", "#"),
];
const STATES: [(&str, &str); 56] = [
    ("ALABAMA", "AL"), ("ALASKA", "AK"), ("ARIZONA", "AZ"), ("ARKANSAS", "AR"), ("CALIFORNIA", "CA"), ("COLORADO", "CO"), ("CONNECTICUT", "CT"),
    ("DELAWARE", "DE"), ("DISTRICT OF COLUMBIA", "DC"), ("FLORIDA", "FL"), ("GEORGIA", "GA"), ("HAWAII", "HI"), ("IDAHO", "ID"), ("ILLINOIS", "IL"),
    ("INDIANA", "IN"), ("IOWA", "IA"), ("KANSAS", "KS"), ("KENTUCKY", "KY"), ("LOUISIANA", "LA"), ("MAINE", "ME"), ("MARYLAND", "MD"),
    ("MASSACHUSETTS", "MA"), ("MICHIGAN", "MI"), ("MINNESOTA", "MN"), ("MISSISSIPPI", "MS"), ("MISSOURI", "MO"), ("MONTANA", "MT"),
    ("NEBRASKA", "NE"), ("NEVADA", "NV"), ("NEW HAMPSHIRE", "NH"), ("NEW JERSEY", "NJ"), ("NEW MEXICO", "NM"), ("NEW YORK", "NY"),
    ("NORTH CAROLINA", "NC"), ("NORTH DAKOTA", "ND"), ("OHIO", "OH"), ("OKLAHOMA", "OK"), ("OREGON", "OR"), ("PENNSYLVANIA", "PA"),
    ("RHODE ISLAND", "RI"), ("SOUTH CAROLINA", "SC"), ("SOUTH DAKOTA", "SD"), ("TENNESSEE", "TN"), ("TEXAS", "TX"), ("UTAH", "UT"),
    ("VERMONT", "VT"), ("VIRGINIA", "VA"), ("WASHINGTON", "WA"), ("WEST VIRGINIA", "WV"), ("WISCONSIN", "WI"), ("WYOMING", "WY"),
    ("PUERTO RICO", "PR"), ("GUAM", "GU"), ("VIRGIN ISLANDS", "VI"), ("AMERICAN SAMOA", "AS"), ("NORTHERN MARIANA ISLANDS", "MP"),
];
const PROVINCES: [(&str, &str); 13] = [
    ("ALBERTA", "AB"), ("BRITISH COLUMBIA", "BC"), ("MANITOBA", "MB"), ("NEW BRUNSWICK", "NB"), ("NEWFOUNDLAND AND LABRADOR", "NL"),
    ("NOVA SCOTIA", "NS"), ("NORTHWEST TERRITORIES", "NT"), ("NUNAVUT", "NU"), ("ONTARIO", "ON"), ("PRINCE EDWARD ISLAND", "PE"),
    ("QUEBEC", "QC"), ("SASKATCHEWAN", "SK"), ("YUKON", "YT"),
];
const COUNTRIES: [(&str, &[&str]); 8] = [
    ("US", &["US", "USA", "U S", "U S A", "UNITED STATES", "UNITED STATES OF AMERICA", "AMERICA"]),
    ("CA", &["CA", "CAN", "CANADA"]),
    ("MX", &["MX", "MEX", "MEXICO", "MÉXICO"]),
    ("GB", &["GB", "UK", "U K", "UNITED KINGDOM", "GREAT BRITAIN", "ENGLAND", "SCOTLAND", "WALES"]),
    ("DE", &["DE", "DEU", "GERMANY", "DEUTSCHLAND"]),
    ("FR", &["FR", "FRA", "FRANCE"]),
    ("NL", &["NL", "NLD", "NETHERLANDS", "THE NETHERLANDS", "HOLLAND"]),
    ("AU", &["AU", "AUS", "AUSTRALIA"]),
];

fn lookup(table: &[(&'static str, &'static str)], k: &str) -> Option<&'static str> {
    table.iter().find(|(a, b)| *a == k || *b == k).map(|(_, b)| *b)
}

/// Uppercase, periods and commas removed (initialisms such as "N.W." and "P.O." joined up),
/// "#" spaced, runs of spaces collapsed.
fn clean(s: &str) -> String {
    let up: Vec<char> = s.to_uppercase().chars().collect();
    let mut out = String::new();
    for (i, &c) in up.iter().enumerate() {
        let single_letter_before = i >= 1 && up[i - 1].is_alphabetic() && (i < 2 || !up[i - 2].is_alphanumeric());
        if c == '.' && single_letter_before && up.get(i + 1).is_some_and(|n| n.is_alphabetic()) {
            continue;
        }
        match c {
            '.' | ',' | ';' => out.push(' '),
            '#' => out.push_str(" # "),
            c if c.is_whitespace() => out.push(' '),
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A street line: "123 north main street apartment 4b" -> "123 N MAIN ST APT 4B".
pub fn street_line(raw: &str) -> String {
    let mut t = clean(raw);
    // PO boxes.
    for p in ["POST OFFICE BOX", "P O BOX", "PO BOX", "P O B", "POB", "BOX"] {
        if t.starts_with(&format!("{p} ")) && t[p.len() + 1..].starts_with(|c: char| c.is_ascii_digit()) {
            t = format!("PO BOX {}", &t[p.len() + 1..]);
            return t;
        }
    }
    let words: Vec<String> = t.split(' ').map(str::to_owned).collect();
    // Split off the first unit designator and everything after it. A line that starts with
    // one ("BUILDING 7 FLOOR 3") is all unit.
    let is_unit = |w: &str| UNITS.iter().any(|(a, _)| *a == w);
    let unit_at = words.iter().position(|w| is_unit(w));
    let (street, unit) = match unit_at {
        Some(p) => (&words[..p], &words[p..]),
        None => (&words[..], &[][..]),
    };
    let mut st: Vec<String> = street.to_vec();
    let n = st.len();
    let is_dir = |w: &str| DIRECTIONS.iter().any(|(a, b)| *a == w || *b == w);
    let is_suffix = |w: &str| SUFFIXES.iter().any(|(a, b)| *a == w || *b == w);
    // The suffix is the last word, or the word before a trailing direction ("MAIN STREET NORTH").
    let suffix_at = if n >= 3 && is_dir(&st[n - 1]) && is_suffix(&st[n - 2]) { Some(n - 2) } else if n >= 3 && is_suffix(&st[n - 1]) { Some(n - 1) } else { None };
    if let Some(k) = suffix_at {
        st[k] = lookup(&SUFFIXES, &st[k]).unwrap_or("").to_owned();
        if k + 1 < n {
            st[k + 1] = lookup(&DIRECTIONS, &st[k + 1]).unwrap_or("").to_owned();
        }
    }
    // A pre-direction after the house number, unless it is the street name itself ("100 NORTH ST").
    let name_end = suffix_at.unwrap_or(n);
    if n >= 2 && st[0].starts_with(|c: char| c.is_ascii_digit()) && is_dir(&st[1]) && name_end > 2 {
        st[1] = lookup(&DIRECTIONS, &st[1]).unwrap_or("").to_owned();
    }
    // Unit part: designators abbreviated, a "#" right after a designator dropped ("SUITE # 200").
    let mut parts: Vec<String> = if st.is_empty() { Vec::new() } else { vec![st.join(" ")] };
    let mut prev_designator = false;
    for w in unit {
        if w == "#" && prev_designator {
            continue;
        }
        match lookup(&UNITS, w) {
            Some(d) if is_unit(w) => {
                parts.push(d.to_owned());
                prev_designator = d != "#";
            }
            _ => {
                parts.push(w.clone());
                prev_designator = false;
            }
        }
    }
    parts.join(" ")
}

fn country(raw: &str) -> Option<&'static str> {
    let k = clean(raw);
    COUNTRIES.iter().find(|(_, names)| names.contains(&k.as_str())).map(|(c, _)| *c)
}

fn title(s: &str) -> String {
    s.split(' ')
        .map(|w| {
            let keep_upper = w.len() <= 2 && (lookup(&DIRECTIONS, w).is_some() || w == "PO" || STATES.iter().any(|(_, c)| *c == w)) || w.chars().any(|c| c.is_ascii_digit()) || w == "#";
            if keep_upper {
                w.to_owned()
            } else {
                let mut cs = w.chars();
                cs.next().map(|f| f.to_uppercase().collect::<String>() + &cs.as_str().to_lowercase()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reads address fields named by `_options.fields` (defaults: line1, line2, city, state, zip,
/// country) and adds `address` (standardized), `changes` and `_valid` with `errors`.
fn standardize_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let name = |k: &str| opt.get("fields").and_then(|f| f.get(k)).and_then(Value::as_str).unwrap_or(k).to_owned();
    let get = |m: &Map<String, Value>, k: &str| m.get(&name(k)).and_then(|v| match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }).unwrap_or_default();
    let title_case = opt.get("case").and_then(Value::as_str) == Some("title");
    let default_country = opt.get("default_country").and_then(Value::as_str).unwrap_or("US").to_uppercase();

    let raw: Vec<(&str, String)> = ["line1", "line2", "city", "state", "zip", "country"].iter().map(|k| (*k, get(&m, k))).collect();
    let country_code = if raw[5].1.trim().is_empty() { Some(default_country.clone()) } else { country(&raw[5].1).map(str::to_owned) };
    let cc = country_code.clone().unwrap_or_default();
    let mut out = Map::new();
    let mut errors: Vec<String> = Vec::new();

    let line1 = street_line(&raw[0].1);
    let line2 = if raw[1].1.trim().is_empty() { String::new() } else { street_line(&raw[1].1) };
    let city = clean(&raw[2].1);
    let state_key = clean(&raw[3].1);
    let state = match cc.as_str() {
        "US" => lookup(&STATES, &state_key).map(str::to_owned),
        "CA" => lookup(&PROVINCES, &state_key).map(str::to_owned),
        _ => (!state_key.is_empty()).then(|| state_key.clone()),
    };
    let zip_raw = raw[4].1.trim().to_uppercase();
    let zip = match cc.as_str() {
        "US" => {
            let d: String = zip_raw.chars().filter(char::is_ascii_digit).collect();
            let d = if d.len() == 4 || d.len() == 8 { format!("0{d}") } else { d };
            match d.len() {
                5 => Some(d),
                9 => Some(format!("{}-{}", &d[..5], &d[5..])),
                _ => None,
            }
        }
        "CA" => {
            let a: String = zip_raw.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            let shape = a.len() == 6 && a.chars().enumerate().all(|(i, c)| if i % 2 == 0 { c.is_ascii_alphabetic() } else { c.is_ascii_digit() });
            shape.then(|| format!("{} {}", &a[..3], &a[3..]))
        }
        _ => (!zip_raw.is_empty()).then(|| clean(&zip_raw)),
    };

    if line1.is_empty() {
        errors.push("line1 is empty".into());
    }
    if city.is_empty() {
        errors.push("city is empty".into());
    }
    if country_code.is_none() {
        errors.push(format!("country '{}' isn't recognized", raw[5].1.trim()));
    }
    if (cc == "US" || cc == "CA") && state.is_none() {
        errors.push(format!("state '{}' isn't a {} code or name", raw[3].1.trim(), if cc == "US" { "US state" } else { "Canadian province" }));
    }
    if (cc == "US" || cc == "CA") && zip.is_none() {
        errors.push(format!("postal code '{}' isn't a valid {} format", raw[4].1.trim(), if cc == "US" { "ZIP" } else { "Canadian postal code" }));
    }

    let fmt = |s: String| if title_case { title(&s) } else { s };
    let std: [(&str, Value); 6] = [
        ("line1", json!(fmt(line1))),
        ("line2", if line2.is_empty() { Value::Null } else { json!(fmt(line2)) }),
        ("city", json!(fmt(city))),
        ("state", state.map(|s| json!(s)).unwrap_or(json!(state_key))),
        ("zip", zip.map(|z| json!(z)).unwrap_or(json!(zip_raw))),
        ("country", country_code.map(|c| json!(c)).unwrap_or(json!(raw[5].1.trim()))),
    ];
    let mut changes = Vec::new();
    for ((k, before), (_, after)) in raw.iter().zip(std.iter()) {
        let after_s = after.as_str().unwrap_or("");
        if !(before.trim().is_empty() && after_s.is_empty()) && before.as_str() != after_s && !(k == &"country" && before.trim().is_empty()) {
            changes.push(json!({ "field": k, "from": before, "to": after }));
        }
        out.insert((*k).into(), after.clone());
    }
    m.insert("address".into(), Value::Object(out));
    m.insert("changes".into(), json!(changes));
    m.insert("_valid".into(), json!(errors.is_empty()));
    m.insert("errors".into(), json!(errors));
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
    fn street_lines() {
        assert_eq!(street_line("123 north main street apartment 4b"), "123 N MAIN ST APT 4B");
        assert_eq!(street_line("456 Oak Avenue, Suite #200"), "456 OAK AVE STE 200");
        assert_eq!(street_line("789 Elm St. N.W."), "789 ELM ST NW");
        assert_eq!(street_line("100 North Street"), "100 NORTH ST");
        assert_eq!(street_line("12 Main Street North"), "12 MAIN ST N");
        assert_eq!(street_line("P.O. Box 4411"), "PO BOX 4411");
        assert_eq!(street_line("1 Infinite Loop"), "1 INFINITE LOOP");
        assert_eq!(street_line("55 Park Avenue # 3"), "55 PARK AVE # 3");
        assert_eq!(street_line("Building 7, Floor 3"), "BLDG 7 FL 3");
    }

    #[test]
    fn us_record() {
        let out = go(json!({ "line1": "123 north main street apt 4b", "city": "springfield", "state": "Illinois", "zip": "2134", "id": 7 }));
        assert_eq!(out["address"], json!({ "line1": "123 N MAIN ST APT 4B", "line2": null, "city": "SPRINGFIELD", "state": "IL", "zip": "02134", "country": "US" }));
        assert_eq!(out["_valid"], json!(true));
        assert_eq!(out["id"], json!(7));
        assert_eq!(out["changes"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn title_case_custom_fields_and_canada() {
        let out = go(json!({
            "street": "500 West Georgia Street, Suite 1000", "town": "vancouver", "prov": "british columbia", "postal": "v6b4n8", "ctry": "Canada",
            "_options": { "case": "title", "fields": { "line1": "street", "city": "town", "state": "prov", "zip": "postal", "country": "ctry" } }
        }));
        assert_eq!(out["address"]["line1"], json!("500 W Georgia St Ste 1000"));
        assert_eq!(out["address"]["state"], json!("BC"));
        assert_eq!(out["address"]["zip"], json!("V6B 4N8"));
        assert_eq!(out["address"]["country"], json!("CA"));
        assert_eq!(out["_valid"], json!(true));
    }

    #[test]
    fn errors_are_reported() {
        let out = go(json!({ "line1": "1 Main St", "city": "Austin", "state": "Texass", "zip": "123", "country": "Narnia" }));
        assert_eq!(out["_valid"], json!(false));
        assert_eq!(out["errors"].as_array().unwrap().len(), 1, "{out}"); // unknown country: no US/CA checks
        let us = go(json!({ "line1": "1 Main St", "city": "Austin", "state": "Texass", "zip": "123" }));
        assert_eq!(us["errors"].as_array().unwrap().len(), 2, "{us}");
        assert_eq!(us["address"]["state"], json!("TEXASS"));
    }
}
