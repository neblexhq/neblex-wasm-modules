//! Neblex sandboxed module: recalculate a spreadsheet's formulas with values from each record.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the options.

use base64::Engine as _;
use ironcalc_base::cell::CellValue;
use ironcalc_base::types::{CellType, Workbook};
use ironcalc_base::Model;
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

/// The sandbox has no random source. RAND() and RANDBETWEEN() and hash-table seeds use this
/// fixed-seed generator, so every run repeats exactly.
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(|r| calc_record(r, &mut cache)).collect()),
        Ok(v) => calc_record(v, &mut cache),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// The workbook of the previous record, reused while records share the same file.
#[derive(Default)]
struct Cache {
    key: String,
    workbook: Option<Workbook>,
}

/// A cell or range on one sheet (1-based rows and columns).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    sheet: u32,
    row: i32,
    col: i32,
    rows: i32,
    cols: i32,
}

/// Loads the workbook, writes `_options.inputs` from the record, recalculates and adds
/// `_options.outputs` under `_options.into` (default `result`).
fn calc_record(record: Value, cache: &mut Cache) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let o = m.remove("_options").unwrap_or(Value::Null);
    let fail = |mut m: Map<String, Value>, msg: String| {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(msg));
        Value::Object(m)
    };
    let tz = o.get("timezone").and_then(Value::as_str).unwrap_or("UTC").to_owned();
    let now = match o.get("now") {
        None | Some(Value::Null) => 0,
        Some(v) => match now_millis(v) {
            Some(ms) => ms,
            None => return fail(m, format!("_options.now must be an ISO 8601 date or time, or Unix milliseconds; got {v}")),
        },
    };
    ironcalc_base::mock_time::set_mock_time(now);

    let mut model = match load(&o, &tz, cache) {
        Ok(model) => model,
        Err(e) => return fail(m, e),
    };
    let record = Value::Object(m.clone());

    // Inputs: { "cell, range or name": "record.field" | { "field": ..., "columns": [...] } | { "value": ... } }
    let mut missing = Vec::new();
    if let Some(inputs) = o.get("inputs") {
        let Some(inputs) = inputs.as_object() else { return fail(m, "_options.inputs must be an object of cell: field".into()) };
        for (target, spec) in inputs {
            let area = match resolve(&model, target) {
                Ok(a) => a,
                Err(e) => return fail(m, format!("input '{target}': {e}")),
            };
            let (value, columns) = match spec {
                Value::String(field) => (get_path(&record, field).cloned(), None),
                Value::Object(s) if s.contains_key("value") => (s.get("value").cloned(), None),
                Value::Object(s) => match s.get("field").and_then(Value::as_str) {
                    Some(field) => (get_path(&record, field).cloned(), s.get("columns").and_then(Value::as_array).cloned()),
                    None => return fail(m, format!("input '{target}': give a field name, {{\"field\": ...}} or {{\"value\": ...}}")),
                },
                _ => return fail(m, format!("input '{target}': give a field name, {{\"field\": ...}} or {{\"value\": ...}}")),
            };
            let Some(value) = value else {
                missing.push(json!(target));
                continue;
            };
            if let Err(e) = write(&mut model, area, &value, columns.as_deref()) {
                return fail(m, format!("input '{target}': {e}"));
            }
        }
    }
    model.evaluate();

    // Outputs: { "field": "cell, range or name" | { "cell": ..., "formatted": true } }
    let mut result = Map::new();
    let mut errors = Map::new();
    let outputs = match o.get("outputs").and_then(Value::as_object) {
        Some(x) => x,
        None => return fail(m, "_options.outputs is missing; pass an object of field: cell".into()),
    };
    for (field, spec) in outputs {
        let (target, formatted) = match spec {
            Value::String(s) => (s.as_str(), false),
            Value::Object(s) => match s.get("cell").and_then(Value::as_str) {
                Some(c) => (c, s.get("formatted").and_then(Value::as_bool).unwrap_or(false)),
                None => return fail(m, format!("output '{field}': give a cell or {{\"cell\": ..., \"formatted\": true}}")),
            },
            _ => return fail(m, format!("output '{field}': give a cell or {{\"cell\": ..., \"formatted\": true}}")),
        };
        let area = match resolve(&model, target) {
            Ok(a) => a,
            Err(e) => return fail(m, format!("output '{field}': {e}")),
        };
        let mut read = |r: i32, c: i32| -> Value {
            let (v, err) = read_cell(&model, area.sheet, r, c, formatted);
            if let Some(err) = err {
                let at = if area.rows == 1 && area.cols == 1 { field.clone() } else { format!("{field}[{}][{}]", r - area.row, c - area.col) };
                errors.insert(at, json!(err));
            }
            v
        };
        let v = if area.rows == 1 && area.cols == 1 {
            read(area.row, area.col)
        } else {
            let mut rows = Vec::new();
            for r in area.row..area.row + area.rows {
                let row: Vec<Value> = (area.col..area.col + area.cols).map(|c| read(r, c)).collect();
                rows.push(Value::Array(row));
            }
            // Trailing empty rows of a range (an unfilled line-item table) are dropped.
            while rows.last().is_some_and(|r| r.as_array().is_some_and(|c| c.iter().all(|v| v.is_null() || v == ""))) {
                rows.pop();
            }
            Value::Array(rows)
        };
        result.insert(field.clone(), v);
    }

    let into = o.get("into").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("result").to_owned();
    m.insert(into, Value::Object(result));
    m.insert("cell_errors".into(), Value::Object(errors));
    if !missing.is_empty() {
        m.insert("missing_inputs".into(), Value::Array(missing));
    }
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

/// Builds the model from `_options.workbook` (an .xlsx file as base64) or `_options.sheets`
/// ({ "Sheet": { "A1": value or "=formula" } }).
fn load<'a>(o: &Value, tz: &'a str, cache: &mut Cache) -> Result<Model<'a>, String> {
    if let Some(wb) = o.get("workbook") {
        let Some(b64) = wb.as_str() else { return Err("_options.workbook must be the .xlsx file as base64 text".into()) };
        let key = format!("{tz}|{b64}");
        if cache.key != key || cache.workbook.is_none() {
            // Accept data URLs and line-wrapped base64.
            let b64 = b64.rsplit_once("base64,").map_or(b64, |(_, d)| d);
            let clean: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(clean.trim_end_matches('='))
                .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(clean.trim_end_matches('=')))
                .map_err(|e| format!("_options.workbook is not valid base64: {e}"))?;
            if !bytes.starts_with(b"PK") {
                return Err("_options.workbook is not an .xlsx file (Excel 2007 or later; .xls and .csv aren't supported)".into());
            }
            let wb = ironcalc::import::load_from_xlsx_bytes(&bytes, "workbook", "en", tz).map_err(|e| format!("could not read the workbook: {e:?}"))?;
            cache.workbook = Some(wb);
            cache.key = key;
        }
        let wb = cache.workbook.clone().expect("loaded above");
        return Model::from_workbook(wb, "en").map_err(|e| format!("could not open the workbook: {e}"));
    }
    let Some(sheets) = o.get("sheets").and_then(Value::as_object) else {
        return Err("pass _options.workbook (an .xlsx file as base64) or _options.sheets".into());
    };
    let mut model = Model::new_empty("workbook", "en", tz, "en").map_err(|e| format!("could not create the workbook: {e}"))?;
    for (i, name) in sheets.keys().enumerate() {
        if i == 0 {
            model.rename_sheet("Sheet1", name).map_err(|e| format!("sheet '{name}': {e}"))?;
        } else {
            model.add_sheet(name).map_err(|e| format!("sheet '{name}': {e}"))?;
        }
    }
    for (i, cells) in sheets.values().enumerate() {
        let Some(cells) = cells.as_object() else { return Err("each sheet in _options.sheets must be an object of cell: value".into()) };
        for (cell, v) in cells {
            let (row, col) = parse_cell(cell).ok_or_else(|| format!("'{cell}' is not a cell reference"))?;
            // Sheets you define are typed like a person typing: "=..." is a formula, "5%" a number.
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Null => continue,
                other => other.to_string(),
            };
            model.set_user_input(i as u32, row, col, text).map_err(|e| format!("cell {cell}: {e}"))?;
        }
    }
    Ok(model)
}

/// Writes a record value: a single value into one cell, a list down a column, a list of lists
/// as rows, or a list of objects as rows with the given `columns`.
fn write(model: &mut Model, a: Area, v: &Value, columns: Option<&[Value]>) -> Result<(), String> {
    match v {
        Value::Array(items) => {
            for (r, item) in items.iter().enumerate() {
                let row = a.row + r as i32;
                match (item, columns) {
                    (Value::Object(obj), Some(cols)) => {
                        for (c, key) in cols.iter().enumerate() {
                            let key = key.as_str().unwrap_or_default();
                            let cell = get_path(&Value::Object(obj.clone()), key).cloned().unwrap_or(Value::Null);
                            put(model, a.sheet, row, a.col + c as i32, &cell)?;
                        }
                    }
                    (Value::Object(_), None) => return Err("a list of objects needs \"columns\" (the field for each column, in order)".into()),
                    (Value::Array(cells), _) => {
                        for (c, cell) in cells.iter().enumerate() {
                            put(model, a.sheet, row, a.col + c as i32, cell)?;
                        }
                    }
                    (scalar, _) => put(model, a.sheet, row, a.col, scalar)?,
                }
            }
            Ok(())
        }
        other => put(model, a.sheet, a.row, a.col, other),
    }
}

/// Writes one value with its type: numbers as numbers, true/false as booleans, ISO dates as
/// spreadsheet dates and other text as text (never as a formula).
fn put(model: &mut Model, sheet: u32, row: i32, col: i32, v: &Value) -> Result<(), String> {
    match v {
        Value::Null => model.set_user_input(sheet, row, col, String::new()),
        Value::Bool(b) => model.update_cell_with_bool(sheet, row, col, *b),
        Value::Number(n) => model.update_cell_with_number(sheet, row, col, n.as_f64().unwrap_or(0.0)),
        Value::String(s) => match iso_date_serial(s) {
            Some(serial) => model.update_cell_with_number(sheet, row, col, serial),
            None => model.update_cell_with_text(sheet, row, col, s),
        },
        other => model.update_cell_with_text(sheet, row, col, &other.to_string()),
    }
}

/// Reads one cell: numbers, text and booleans as JSON values, empty cells as null, and error
/// values (#DIV/0!, #N/A) as null plus the error text.
fn read_cell(model: &Model, sheet: u32, row: i32, col: i32, formatted: bool) -> (Value, Option<String>) {
    let is_error = matches!(model.get_cell_type(sheet, row, col), Ok(CellType::ErrorValue));
    let value = model.get_cell_value_by_index(sheet, row, col).unwrap_or(CellValue::None);
    if is_error {
        let text = match value {
            CellValue::String(s) => s,
            _ => "#ERROR".into(),
        };
        return (Value::Null, Some(text));
    }
    if formatted {
        let empty = matches!(value, CellValue::None);
        return match model.get_formatted_cell_value(sheet, row, col) {
            Ok(s) if !empty => (json!(s), None),
            _ => (Value::Null, None),
        };
    }
    let v = match value {
        CellValue::None => Value::Null,
        CellValue::String(s) => json!(s),
        CellValue::Boolean(b) => json!(b),
        CellValue::Number(n) => number(n),
    };
    (v, None)
}

/// Whole numbers come back as integers and others are rounded to 12 significant digits, so
/// 0.1 + 0.2 reads 0.3, as a spreadsheet shows it.
fn number(n: f64) -> Value {
    if !n.is_finite() {
        return Value::Null;
    }
    let r: f64 = format!("{n:.12e}").parse().unwrap_or(n);
    if r.fract() == 0.0 && r.abs() < 9.0e15 {
        json!(r as i64)
    } else {
        json!(r)
    }
}

/// Resolves `B4`, `Quote!B4`, `'Price List'!A2:C9`, `$B$4` or a defined name such as `Total`.
fn resolve(model: &Model, target: &str) -> Result<Area, String> {
    let t = target.trim();
    let (sheet_part, cells) = match t.rsplit_once('!') {
        Some((s, c)) => (Some(s.trim_matches('\'').replace("''", "'")), c),
        None => (None, t),
    };
    let sheet = match &sheet_part {
        Some(name) => model
            .get_worksheets_properties()
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("there is no sheet named '{name}'"))? as u32,
        None => 0,
    };
    let cells = cells.replace('$', "");
    let (a, b) = cells.split_once(':').unwrap_or((&cells, &cells));
    if let (Some((r1, c1)), Some((r2, c2))) = (parse_cell(a), parse_cell(b)) {
        return Ok(Area { sheet, row: r1.min(r2), col: c1.min(c2), rows: (r2 - r1).abs() + 1, cols: (c2 - c1).abs() + 1 });
    }
    if sheet_part.is_none() {
        for (name, _scope, formula) in model.get_defined_name_list() {
            if name.eq_ignore_ascii_case(t) {
                let f = formula.trim_start_matches('=');
                if f.contains('!') && !f.contains('(') {
                    return resolve(model, f);
                }
                return Err(format!("the name '{name}' refers to '{formula}', not to cells"));
            }
        }
    }
    Err(format!("'{target}' is not a cell, a range or a defined name in this workbook"))
}

/// `B4` -> (row 4, column 2).
fn parse_cell(s: &str) -> Option<(i32, i32)> {
    let s = s.trim().to_ascii_uppercase();
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = s.split_at(split);
    if letters.is_empty() || letters.len() > 3 || !letters.bytes().all(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let row: i32 = digits.parse().ok().filter(|r| (1..=1_048_576).contains(r))?;
    let col = letters.bytes().fold(0i32, |acc, b| acc * 26 + (b - b'A' + 1) as i32);
    (col <= 16_384).then_some((row, col))
}

/// `2026-11-24` -> 46350 (days since 1899-12-30, the spreadsheet date system).
fn iso_date_serial(s: &str) -> Option<f64> {
    if s.len() != 10 || s.as_bytes()[4] != b'-' || s.as_bytes()[7] != b'-' {
        return None;
    }
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?;
    Some((d - base).num_days() as f64)
}

fn now_millis(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    let s = v.as_str()?.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    if let Ok(t) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(t.and_utc().timestamp_millis());
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok().map(|d| d.and_hms_opt(0, 0, 0).expect("midnight").and_utc().timestamp_millis())
}

/// Reads a dotted path such as `order.qty` or `lines.0.sku` from the record.
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

    fn workbook() -> String {
        let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/test/fixtures/quote-calculator.xlsx")).unwrap();
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn quote(record: Value) -> Value {
        let mut r = record;
        r["_options"] = json!({
            "workbook": workbook(),
            "now": "2026-11-24",
            "inputs": { "Quote!B3": "customer.tier", "Quantity": "qty", "Quote!B5": "list_price", "Quote!B6": "ship_to.state" },
            "outputs": {
                "tier_discount": "Quote!B8", "volume_discount": "Quote!B9", "net_unit_price": "Quote!B10",
                "tax": "Quote!B13", "total": "Total", "needs_approval": "Quote!B15",
                "valid_until": { "cell": "Quote!B16", "formatted": true }, "summary": "Quote!B17"
            }
        });
        go(r)
    }

    #[test]
    fn prices_a_quote_from_an_xlsx() {
        let out = quote(json!({ "customer": { "tier": "gold" }, "qty": 600, "list_price": 12, "ship_to": { "state": "TX" } }));
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["cell_errors"], json!({}));
        let r = &out["result"];
        assert_eq!(r["tier_discount"], json!(0.1));
        assert_eq!(r["volume_discount"], json!(0.05));
        assert_eq!(r["net_unit_price"], json!(10.26)); // ROUND(12 * 0.9 * 0.95, 2)
        assert_eq!(r["tax"], json!(507.87)); // 6156 * 0.0825
        assert_eq!(r["total"], json!(6663.87));
        assert_eq!(r["needs_approval"], json!(false));
        assert_eq!(r["valid_until"], json!("2026-12-24"));
        assert_eq!(r["summary"], json!("gold x600 = 6,663.87"));
        assert!(out.get("_options").is_none());
    }

    #[test]
    fn approval_flag_and_unknown_lookup() {
        let out = quote(json!({ "customer": { "tier": "platinum" }, "qty": 5000, "list_price": 20, "ship_to": { "state": "ZZ" } }));
        let r = &out["result"];
        assert_eq!(r["needs_approval"], json!(true)); // 0.15 + 0.12 > 0.2
        assert_eq!(r["tax"], json!(0));
        // Must not match: an unknown tier falls back to no discount via IFERROR.
        let unknown = quote(json!({ "customer": { "tier": "diamond" }, "qty": 10, "list_price": 10, "ship_to": { "state": "OR" } }));
        assert_eq!(unknown["result"]["tier_discount"], json!(0));
        assert_eq!(unknown["result"]["total"], json!(100));
    }

    #[test]
    fn batch_uses_fresh_copies() {
        let wb = workbook();
        let opts = json!({ "workbook": wb, "inputs": { "Quantity": "qty", "Tier": "tier" }, "outputs": { "total": "Total" } });
        let out = go(json!([
            { "qty": 100, "tier": "gold", "_options": opts.clone() },
            { "qty": 1, "_options": opts.clone() }
        ]));
        // 100 * ROUND(10 * 0.9 * 0.97, 2) = 873, plus TX tax 8.25%
        assert_eq!(out[0]["result"]["total"], json!(945.02));
        // The second record has no tier, so the workbook's own value (silver) applies again.
        assert_eq!(out[1]["missing_inputs"], json!(["Tier"]));
        assert_eq!(out[1]["result"]["total"], json!(10.28)); // 9.50 + 0.78 tax
    }

    #[test]
    fn line_items_as_objects_and_ranges_out() {
        let out = go(json!({
            "qty": 0,
            "lines": [{ "sku": "A-1", "qty": 2, "price": 9.99 }, { "sku": "B-2", "qty": 1, "price": 100 }],
            "_options": {
                "workbook": workbook(),
                "inputs": { "Quantity": "qty", "Lines!A2": { "field": "lines", "columns": ["sku", "qty", "price"] } },
                "outputs": { "lines_total": "Lines!E1", "lines": "Lines!A2:D21", "subtotal": "Quote!B11" }
            }
        }));
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["result"]["lines_total"], json!(119.98));
        assert_eq!(out["result"]["lines"], json!([["A-1", 2, 9.99, 19.98], ["B-2", 1, 100, 100]]));
        assert_eq!(out["result"]["subtotal"], json!(119.98));
        let bad = go(json!({ "lines": [{ "sku": "A" }], "_options": { "workbook": workbook(), "inputs": { "Lines!A2": "lines" }, "outputs": { "x": "Lines!E1" } } }));
        assert_eq!(bad["ok"], json!(false));
    }

    #[test]
    fn sheets_defined_as_json() {
        let out = go(json!({
            "hours": 37.5, "rate": 48, "start": "2026-11-02", "name": "=1+1",
            "_options": {
                "sheets": { "Pay": { "A1": "Hours", "B1": 0, "B2": 0, "B3": "=ROUND(B1*B2*IF(B1>40,1.5,1),2)", "B4": "=B5+14", "B6": "=UPPER(B7)", "B8": "=1/0", "B9": "=VLOOKUP(\"x\",A1:A1,1,FALSE)" } },
                "inputs": { "B1": "hours", "B2": "rate", "B5": "start", "B7": "name" },
                "outputs": { "pay": "B3", "paid_on": { "cell": "B4", "formatted": true }, "paid_on_serial": "B4", "name": "B6", "bad": "B8", "missing": "B9", "empty": "C1" }
            }
        }));
        assert_eq!(out["ok"], json!(true), "{out}");
        let r = &out["result"];
        assert_eq!(r["pay"], json!(1800));
        assert_eq!(r["paid_on_serial"], json!(46342)); // 2026-11-16
        // Text from the record is never run as a formula.
        assert_eq!(r["name"], json!("=1+1"));
        assert_eq!(r["bad"], Value::Null);
        assert_eq!(r["empty"], Value::Null);
        assert_eq!(out["cell_errors"]["bad"], json!("#DIV/0!"));
        assert_eq!(out["cell_errors"]["missing"], json!("#N/A"));
    }

    #[test]
    fn readme_functions_and_input_forms() {
        let out = go(json!({
            "skus": ["A", "B", "C"], "grid": [[1, 2], [3, 4]], "start": "2026-01-31",
            "_options": {
                "timezone": "America/Chicago",
                "now": "2026-11-24T12:00:00Z",
                "sheets": { "S": {
                    "D1": "=XLOOKUP(\"B\",A1:A3,B1:B3)",
                    "D2": "=INDEX(A1:A3,MATCH(\"C\",A1:A3,0))",
                    "D3": "=IFS(F1>10,\"big\",F1>1,\"mid\",TRUE,\"small\")",
                    "D4": "=CONCAT(A1,A2,A3)",
                    "D5": "=EDATE(G1,1)",
                    "D6": "=SUM(B1:C2)",
                    "D7": "=H1*2",
                    "D8": "=TODAY()"
                }},
                "inputs": { "A1": "skus", "B1": "grid", "G1": "start", "F1": { "value": 5 }, "H1": { "value": 0.07 } },
                "outputs": { "xlookup": "D1", "index_match": "D2", "ifs": "D3", "concat": "D4", "edate": { "cell": "D5", "formatted": true }, "edate_serial": "D5", "sum": "D6", "fixed": "D7", "today": "D8" }
            }
        }));
        assert_eq!(out["ok"], json!(true), "{out}");
        assert_eq!(out["cell_errors"], json!({}), "{out}");
        let r = &out["result"];
        assert_eq!(r["xlookup"], json!(3));
        assert_eq!(r["index_match"], json!("C"));
        assert_eq!(r["ifs"], json!("mid"));
        assert_eq!(r["concat"], json!("ABC"));
        assert_eq!(r["edate_serial"], json!(46081)); // 2026-02-28, month-end safe
        assert_eq!(r["sum"], json!(10));
        assert_eq!(r["fixed"], json!(0.14));
        assert_eq!(r["today"], json!(46350));
    }

    #[test]
    fn reference_parsing() {
        assert_eq!(parse_cell("B4"), Some((4, 2)));
        assert_eq!(parse_cell("aa10"), Some((10, 27)));
        assert_eq!(parse_cell("XFD1048576"), Some((1_048_576, 16_384)));
        assert_eq!(parse_cell("XFE1"), None);
        assert_eq!(parse_cell("B0"), None);
        assert_eq!(parse_cell("Total"), None);
        assert_eq!(iso_date_serial("2026-11-24"), Some(46350.0));
        assert_eq!(iso_date_serial("11/24/2026"), None);
        assert_eq!(number(0.1 + 0.2), json!(0.3));
        assert_eq!(number(3.0), json!(3));
    }

    #[test]
    fn errors_are_reported() {
        let wb = workbook();
        for (opts, needle) in [
            (json!({ "outputs": { "a": "A1" } }), "workbook"),
            (json!({ "workbook": "not base64!!", "outputs": { "a": "A1" } }), "base64"),
            (json!({ "workbook": "aGVsbG8=", "outputs": { "a": "A1" } }), ".xlsx"),
            (json!({ "workbook": wb.clone(), "outputs": { "a": "Nope!A1" } }), "no sheet"),
            (json!({ "workbook": wb.clone(), "outputs": { "a": "NoSuchName" } }), "defined name"),
            (json!({ "workbook": wb.clone(), "outputs": { "a": "TierTable" } }), ""),
            (json!({ "workbook": wb, "now": "soon", "outputs": { "a": "A1" } }), "now"),
        ] {
            let out = go(json!({ "_options": opts }));
            if needle.is_empty() {
                // A name that covers a range returns the range.
                assert_eq!(out["result"]["a"][3], json!(["platinum", 0.15]), "{out}");
                continue;
            }
            assert_eq!(out["ok"], json!(false), "{out}");
            assert!(out["error"].as_str().unwrap().contains(needle), "{out}");
        }
    }
}
