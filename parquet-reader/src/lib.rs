//! Neblex sandboxed module: Parquet file (base64 inside the step input JSON) -> JSON records,
//! with the schema, so warehouse and data-lake exports can flow into any other step.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the fields.

use base64::Engine;
use bytes::Bytes;
use parquet::basic::{LogicalType, Repetition, TimeUnit};
use parquet::record::{Field, Row};
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::schema::types::{Type, TypePtr};
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Rows returned when `_options.limit` is not set.
const DEFAULT_LIMIT: usize = 10_000;

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
        Ok(Value::Object(m)) => read_record(m),
        Ok(Value::Array(items)) => Value::Array(
            items
                .into_iter()
                .map(|v| match v {
                    Value::Object(m) => read_record(m),
                    _ => json!({ "ok": false, "error": "each input item must be a JSON object" }),
                })
                .collect(),
        ),
        Ok(_) => json!({ "ok": false, "error": "input must be a JSON object or a list of them" }),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

struct Opts {
    field: Option<String>,
    columns: Option<Vec<String>>,
    offset: usize,
    limit: usize,
    schema: bool,
    decimals_as_numbers: bool,
}

fn opts(o: &Value) -> Result<Opts, String> {
    let columns = match o.get("columns") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) => Some(
            a.iter()
                .map(|v| v.as_str().map(str::to_owned).ok_or("_options.columns must be a list of column names"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Some(_) => return Err("_options.columns must be a list of column names".into()),
    };
    let num = |k: &str, d: usize| -> Result<usize, String> {
        match o.get(k) {
            None | Some(Value::Null) => Ok(d),
            Some(v) => v.as_u64().map(|n| n as usize).ok_or(format!("_options.{k} must be a whole number 0 or more")),
        }
    };
    Ok(Opts {
        field: o.get("field").and_then(Value::as_str).map(str::to_owned),
        columns,
        offset: num("offset", 0)?,
        limit: num("limit", DEFAULT_LIMIT)?,
        schema: o.get("schema").and_then(Value::as_bool).unwrap_or(true),
        decimals_as_numbers: match o.get("decimals").and_then(Value::as_str) {
            None | Some("string") => false,
            Some("number") => true,
            Some(other) => return Err(format!("_options.decimals must be \"string\" or \"number\", not \"{other}\"")),
        },
    })
}

/// Reads the file named by `_options.field` (a dotted path), or else the first string that is a
/// base64 Parquet file (starts with `UEFSM`, the base64 of `PAR1`, or is a data: URL of one).
/// Adds `rows`, `row_count` (rows in the file), `returned`, `truncated`, `columns` and
/// `row_groups`, and keeps the other fields of the input except the file itself.
fn read_record(mut m: Map<String, Value>) -> Value {
    let o = match opts(&m.remove("_options").unwrap_or(Value::Null)) {
        Ok(o) => o,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let root = Value::Object(m);
    let found = match &o.field {
        Some(f) => match get_path(&root, f) {
            Some(Value::String(s)) => Some((f.clone(), strip_data_url(s).to_owned())),
            Some(_) => return json!({ "ok": false, "error": format!("field '{f}' is not text (expected the Parquet file as base64)") }),
            None => return json!({ "ok": false, "error": format!("field '{f}' not found in input") }),
        },
        None => find_parquet(&root, String::new()),
    };
    let Some((path, b64)) = found else {
        return json!({ "ok": false, "error": "no base64 Parquet file found in input (expected a string starting with UEFSM, or set _options.field)" });
    };
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(cleaned) {
        Ok(b) => b,
        Err(e) => return json!({ "ok": false, "error": format!("invalid base64 in {path}: {e}") }),
    };
    let mut out = match root {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    remove_path(&mut out, &path);
    match read_parquet(bytes, &o) {
        Ok(r) => {
            out.insert("ok".into(), json!(true));
            out.insert("source_field".into(), json!(path));
            for (k, v) in r {
                out.insert(k, v);
            }
        }
        Err(e) => {
            out.insert("ok".into(), json!(false));
            out.insert("error".into(), json!(e));
        }
    }
    Value::Object(out)
}

fn strip_data_url(s: &str) -> &str {
    match s.strip_prefix("data:") {
        Some(rest) => rest.split_once(";base64,").map(|(_, b)| b).unwrap_or(s),
        None => s,
    }
}

fn is_parquet_b64(s: &str) -> bool {
    strip_data_url(s).trim_start().starts_with("UEFSM")
}

/// Depth-first search for the first string that is a base64 Parquet file.
fn find_parquet(v: &Value, path: String) -> Option<(String, String)> {
    match v {
        Value::String(s) if is_parquet_b64(s) => Some((path, strip_data_url(s).to_owned())),
        Value::Object(m) => m.iter().find_map(|(k, v)| find_parquet(v, if path.is_empty() { k.clone() } else { format!("{path}.{k}") })),
        Value::Array(a) => a.iter().enumerate().find_map(|(i, v)| find_parquet(v, format!("{path}[{i}]"))),
        _ => None,
    }
}

fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(v, |cur, key| cur.get(key))
}

/// Drops the file from the output so a large base64 string isn't passed on twice. Only plain
/// dotted object paths are removed; a file found inside a list is left in place.
fn remove_path(m: &mut Map<String, Value>, path: &str) {
    if path.contains('[') {
        return;
    }
    match path.split_once('.') {
        None => {
            m.remove(path);
        }
        Some((head, rest)) => {
            if let Some(Value::Object(inner)) = m.get_mut(head) {
                remove_path(inner, rest);
            }
        }
    }
}

fn read_parquet(bytes: Vec<u8>, o: &Opts) -> Result<Vec<(String, Value)>, String> {
    if bytes.len() < 12 || &bytes[..4] != b"PAR1" || &bytes[bytes.len() - 4..] != b"PAR1" {
        return Err("not a Parquet file (missing PAR1 markers at the start and end; the file may be cut off)".into());
    }
    let reader = SerializedFileReader::new(Bytes::from(bytes)).map_err(|e| format!("cannot read Parquet file: {e}"))?;
    let meta = reader.metadata().file_metadata();
    let schema = meta.schema();
    let total = meta.num_rows().max(0) as usize;

    let projection = match &o.columns {
        None => None,
        Some(cols) => Some(project(schema, cols)?),
    };

    let read_schema: &Type = projection.as_ref().unwrap_or(schema);
    let mut rows = Vec::new();
    let mut skip = o.offset;
    for i in 0..reader.num_row_groups() {
        if rows.len() >= o.limit {
            break;
        }
        let rg = reader.get_row_group(i).map_err(|e| format!("row group {i}: {e}"))?;
        let n = rg.metadata().num_rows().max(0) as usize;
        // Whole row groups before the offset are skipped without decoding them.
        if skip >= n {
            skip -= n;
            continue;
        }
        let iter = rg.get_row_iter(projection.clone()).map_err(|e| format!("row group {i}: {e}"))?;
        for row in iter.skip(skip) {
            if rows.len() >= o.limit {
                break;
            }
            let row = row.map_err(|e| format!("row group {i}: {e}"))?;
            rows.push(row_json(&row, read_schema, o.decimals_as_numbers));
        }
        skip = 0;
    }
    let returned = rows.len();
    let truncated = o.offset + returned < total;

    let mut out = vec![
        ("row_count".into(), json!(total)),
        ("returned".into(), json!(returned)),
        ("truncated".into(), json!(truncated)),
    ];
    if o.schema {
        let cols: Vec<Value> = match &projection {
            Some(p) => p.get_fields().iter().map(|f| describe(f)).collect(),
            None => schema.get_fields().iter().map(|f| describe(f)).collect(),
        };
        out.push(("columns".into(), Value::Array(cols)));
        out.push(("row_groups".into(), json!(reader.num_row_groups())));
        if let Some(c) = meta.created_by() {
            out.push(("created_by".into(), json!(c)));
        }
    }
    out.push(("rows".into(), Value::Array(rows)));
    Ok(out)
}

/// A schema with only the named top-level columns, in the order given.
fn project(schema: &Type, cols: &[String]) -> Result<Type, String> {
    let mut fields: Vec<TypePtr> = Vec::new();
    for c in cols {
        let f = schema
            .get_fields()
            .iter()
            .find(|f| f.name() == c)
            .ok_or_else(|| {
                let names: Vec<&str> = schema.get_fields().iter().map(|f| f.name()).collect();
                format!("column '{c}' not in file (columns: {})", names.join(", "))
            })?;
        fields.push(Arc::clone(f));
    }
    Type::group_type_builder(schema.name())
        .with_fields(fields)
        .build()
        .map_err(|e| format!("cannot select columns: {e}"))
}

/// Name, readable type and nullability of a top-level column.
fn describe(t: &Type) -> Value {
    let info = t.get_basic_info();
    let nullable = !info.has_repetition() || info.repetition() != Repetition::REQUIRED;
    let ty = type_name(t);
    json!({ "name": t.name(), "type": ty, "nullable": nullable })
}

fn type_name(t: &Type) -> String {
    let info = t.get_basic_info();
    if let Some(l) = info.logical_type_ref() {
        return match l {
            LogicalType::String => "string".into(),
            LogicalType::Date => "date".into(),
            LogicalType::Decimal(d) => format!("decimal({},{})", d.precision, d.scale),
            LogicalType::Timestamp { .. } => "timestamp".into(),
            LogicalType::Time { .. } => "time".into(),
            LogicalType::Integer(i) => format!("{}int{}", if i.is_signed { "" } else { "u" }, i.bit_width),
            LogicalType::List => "list".into(),
            LogicalType::Map => "map".into(),
            LogicalType::Enum => "enum".into(),
            LogicalType::Json => "json".into(),
            LogicalType::Uuid => "uuid".into(),
            LogicalType::Float16 => "float16".into(),
            _ => format!("{l:?}").to_lowercase(),
        };
    }
    use parquet::basic::ConvertedType as C;
    match info.converted_type() {
        C::UTF8 => return "string".into(),
        C::DATE => return "date".into(),
        C::DECIMAL if !t.is_group() => {
            if let Type::PrimitiveType { precision, scale, .. } = t {
                return format!("decimal({precision},{scale})");
            }
        }
        C::TIMESTAMP_MILLIS | C::TIMESTAMP_MICROS => return "timestamp".into(),
        C::TIME_MILLIS | C::TIME_MICROS => return "time".into(),
        C::LIST => return "list".into(),
        C::MAP | C::MAP_KEY_VALUE => return "map".into(),
        C::ENUM => return "enum".into(),
        C::JSON => return "json".into(),
        _ => {}
    }
    if t.is_group() {
        return "struct".into();
    }
    use parquet::basic::Type as P;
    match t.get_physical_type() {
        P::BOOLEAN => "boolean",
        P::INT32 => "int32",
        P::INT64 => "int64",
        P::INT96 => "timestamp",
        P::FLOAT => "float",
        P::DOUBLE => "double",
        P::BYTE_ARRAY | P::FIXED_LEN_BYTE_ARRAY => "binary",
    }
    .into()
}

/// One row as a JSON object, using the schema for timestamps and decimals.
fn row_json(row: &Row, schema: &Type, dec_num: bool) -> Value {
    let fields = schema.get_fields();
    Value::Object(
        row.get_column_iter()
            .map(|(name, f)| {
                let t = fields.iter().find(|t| t.name() == name).map(|t| t.as_ref());
                (name.clone(), field_json(f, t, dec_num))
            })
            .collect(),
    )
}

/// Element type of a LIST column (standard three-level layout or the older two-level ones).
fn list_element(t: &Type) -> Option<&Type> {
    let rep = t.get_fields().first()?;
    if rep.is_group() && rep.get_fields().len() == 1 && rep.name() != "array" && !rep.name().ends_with("_tuple") {
        rep.get_fields().first().map(|e| e.as_ref())
    } else {
        Some(rep.as_ref())
    }
}

fn field_json(f: &Field, t: Option<&Type>, dec_num: bool) -> Value {
    match f {
        Field::Group(r) => match t {
            Some(t) if t.is_group() => row_json(r, t, dec_num),
            _ => r.to_json_value(),
        },
        Field::ListInternal(l) => {
            let et = t.filter(|t| t.is_group()).and_then(list_element);
            Value::Array(l.elements().iter().map(|e| field_json(e, et, dec_num)).collect())
        }
        Field::MapInternal(m) => {
            let kv = t.and_then(|t| t.get_fields().first()).filter(|kv| kv.is_group());
            let vt = kv.and_then(|kv| kv.get_fields().get(1)).map(|v| v.as_ref());
            Value::Object(
                m.entries()
                    .iter()
                    .map(|(k, v)| {
                        let k = match field_json(k, None, false) {
                            Value::String(s) => s,
                            other => other.to_string(),
                        };
                        (k, field_json(v, vt, dec_num))
                    })
                    .collect(),
            )
        }
        Field::Date(d) => chrono::NaiveDate::from_num_days_from_ce_opt(719_163i32.saturating_add(*d))
            .map(|d| Value::String(d.format("%Y-%m-%d").to_string()))
            .unwrap_or(json!(d)),
        Field::TimestampMillis(v) => iso(*v as i128 * 1_000_000, utc_flag(t)).unwrap_or(json!(v)),
        Field::TimestampMicros(v) => iso(*v as i128 * 1_000, utc_flag(t)).unwrap_or(json!(v)),
        Field::Long(v) => match t.and_then(|t| t.get_basic_info().logical_type_ref().cloned()) {
            // Nanosecond timestamps have no older annotation, so the record API leaves them as numbers.
            Some(LogicalType::Timestamp(ts)) => iso(*v as i128 * unit_nanos(&ts.unit), ts.is_adjusted_to_u_t_c).unwrap_or(json!(v)),
            _ => f.to_json_value(),
        },
        Field::Decimal(_) if dec_num => match f.to_json_value() {
            Value::String(s) => s.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::String(s)),
            v => v,
        },
        _ => f.to_json_value(),
    }
}

fn unit_nanos(u: &TimeUnit) -> i128 {
    match u {
        TimeUnit::MILLIS => 1_000_000,
        TimeUnit::MICROS => 1_000,
        TimeUnit::NANOS => 1,
    }
}

/// UTC unless the column says the time is local (no time zone). INT96 and the older
/// TIMESTAMP_* annotations count as UTC, which is how Spark, Hive and Impala write them.
fn utc_flag(t: Option<&Type>) -> bool {
    match t.and_then(|t| t.get_basic_info().logical_type_ref()) {
        Some(LogicalType::Timestamp(ts)) => ts.is_adjusted_to_u_t_c,
        _ => true,
    }
}

/// ISO 8601: `2026-01-31T14:05:09Z`, with fractional seconds only when present and no `Z` for
/// local times. None when the value is outside the dates chrono can show (about 262,000 years
/// either side of year 0); the caller then returns the raw number.
fn iso(nanos: i128, utc: bool) -> Option<Value> {
    let secs = nanos.div_euclid(1_000_000_000) as i64;
    let sub = nanos.rem_euclid(1_000_000_000) as u32;
    chrono::DateTime::from_timestamp(secs, sub).map(|dt| {
        let s = dt.naive_utc().format("%Y-%m-%dT%H:%M:%S%.f").to_string();
        Value::String(if utc { s + "Z" } else { s })
    })
}

#[cfg(test)]
mod tests;
