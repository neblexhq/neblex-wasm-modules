//! Neblex sandboxed module: JSON records -> Parquet file (base64), with column types inferred or
//! set per column, so flow data can be loaded into warehouses, data lakes and analytics tools.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the fields.

use base64::Engine;
use parquet::basic::{Compression, LogicalType, Repetition, TimeUnit, Type as Physical};
use parquet::data_type::{BoolType, ByteArray, ByteArrayType, DoubleType, FixedLenByteArray, FixedLenByteArrayType, Int32Type, Int64Type};
use parquet::file::properties::WriterProperties;
use parquet::file::writer::SerializedFileWriter;
use parquet::schema::types::Type;
use serde_json::{json, Map, Value};
use std::sync::Arc;

/// Rows per row group when `_options.row_group_size` is not set.
const DEFAULT_ROW_GROUP: usize = 10_000;

#[cfg_attr(not(test), no_mangle)] // tests link parquet-reader, which exports the same names
pub extern "C" fn alloc(len: i32) -> i32 {
    let mut buf: Vec<u8> = Vec::with_capacity(len.max(0) as usize);
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf);
    ptr as i32
}

#[cfg_attr(not(test), no_mangle)] // tests link parquet-reader, which exports the same names
pub extern "C" fn invoke(ptr: i32, len: i32) -> i64 {
    let input = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let out = run(input);
    let out = serde_json::to_vec(&out).unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"serialize\"}".to_vec()).leak();
    ((out.as_ptr() as i64) << 32) | (out.len() as i64)
}

pub fn run(input: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(input) {
        Ok(Value::Object(m)) => write_record(m),
        Ok(Value::Array(items)) => Value::Array(
            items
                .into_iter()
                .map(|v| match v {
                    Value::Object(m) => write_record(m),
                    _ => json!({ "ok": false, "error": "each input item must be a JSON object" }),
                })
                .collect(),
        ),
        Ok(_) => json!({ "ok": false, "error": "input must be a JSON object or a list of them" }),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// A column type the module can write.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    String,
    Int64,
    Int32,
    Double,
    Boolean,
    Date,
    /// Timestamp with a time zone (stored in UTC) or without one (local time).
    Timestamp { utc: bool },
    Decimal { precision: u32, scale: u32 },
    Json,
}

impl Kind {
    pub fn parse(s: &str) -> Result<Kind, String> {
        let t = s.trim().to_lowercase();
        Ok(match t.as_str() {
            "string" | "text" => Kind::String,
            "int64" | "integer" | "bigint" | "long" => Kind::Int64,
            "int32" | "int" => Kind::Int32,
            "double" | "float" | "number" => Kind::Double,
            "boolean" | "bool" => Kind::Boolean,
            "date" => Kind::Date,
            "timestamp" => Kind::Timestamp { utc: true },
            "timestamp_local" => Kind::Timestamp { utc: false },
            "json" => Kind::Json,
            _ => {
                let inner = t.strip_prefix("decimal(").and_then(|r| r.strip_suffix(')'));
                let parts: Option<(u32, u32)> = inner.and_then(|i| {
                    let (p, s) = i.split_once(',')?;
                    Some((p.trim().parse().ok()?, s.trim().parse().ok()?))
                });
                match parts {
                    Some((p, s)) if (1..=38).contains(&p) && s <= p => Kind::Decimal { precision: p, scale: s },
                    _ => return Err(format!(
                        "unknown column type '{s}' (use string, int64, int32, double, boolean, date, timestamp, timestamp_local, decimal(p,s) with p 1-38, or json)"
                    )),
                }
            }
        })
    }

    pub fn name(&self) -> String {
        match self {
            Kind::String => "string".into(),
            Kind::Int64 => "int64".into(),
            Kind::Int32 => "int32".into(),
            Kind::Double => "double".into(),
            Kind::Boolean => "boolean".into(),
            Kind::Date => "date".into(),
            Kind::Timestamp { utc: true } => "timestamp".into(),
            Kind::Timestamp { utc: false } => "timestamp_local".into(),
            Kind::Decimal { precision, scale } => format!("decimal({precision},{scale})"),
            Kind::Json => "json".into(),
        }
    }
}

struct Column {
    field: String,
    name: String,
    kind: Kind,
}

struct Opts {
    records_field: String,
    into: String,
    columns: Option<Vec<(String, Option<String>, Option<String>)>>,
    compression: Compression,
    compression_name: String,
    row_group_size: usize,
    infer_dates: bool,
    keep_source: bool,
}

fn opts(o: &Value) -> Result<Opts, String> {
    let s = |k: &str, d: &str| o.get(k).and_then(Value::as_str).unwrap_or(d).to_owned();
    let columns = match o.get("columns") {
        None | Some(Value::Null) => None,
        Some(Value::Array(a)) if !a.is_empty() => Some(
            a.iter()
                .map(|c| match c {
                    Value::String(f) => Ok((f.clone(), None, None)),
                    Value::Object(m) => {
                        let field = m.get("field").and_then(Value::as_str).ok_or("each entry in _options.columns needs a \"field\"")?;
                        let get = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_owned);
                        Ok((field.to_owned(), get("name"), get("type")))
                    }
                    _ => Err("_options.columns entries must be field names or { \"field\", \"name\", \"type\" } objects".to_owned()),
                })
                .collect::<Result<Vec<_>, String>>()?,
        ),
        Some(_) => return Err("_options.columns must be a non-empty list".into()),
    };
    let compression_name = s("compression", "snappy").to_lowercase();
    let compression = match compression_name.as_str() {
        "snappy" => Compression::SNAPPY,
        "gzip" => Compression::GZIP(Default::default()),
        "zstd" => Compression::ZSTD(Default::default()),
        "lz4" => Compression::LZ4_RAW,
        "brotli" => Compression::BROTLI(Default::default()),
        "none" | "uncompressed" => Compression::UNCOMPRESSED,
        other => return Err(format!("_options.compression must be snappy, gzip, zstd, lz4, brotli or none, not \"{other}\"")),
    };
    let row_group_size = match o.get("row_group_size") {
        None | Some(Value::Null) => DEFAULT_ROW_GROUP,
        Some(v) => match v.as_u64() {
            Some(n) if n >= 1 => n as usize,
            _ => return Err("_options.row_group_size must be a whole number 1 or more".into()),
        },
    };
    Ok(Opts {
        records_field: s("records_field", "records"),
        into: s("into", "file"),
        columns,
        compression,
        compression_name,
        row_group_size,
        infer_dates: o.get("infer_dates").and_then(Value::as_bool).unwrap_or(true),
        keep_source: o.get("keep_source").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn get_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    // A field whose name contains a dot is found before trying it as a path.
    if let Some(x) = v.get(path) {
        return Some(x);
    }
    path.split('.').try_fold(v, |cur, key| cur.get(key))
}

fn write_record(mut m: Map<String, Value>) -> Value {
    let o = match opts(&m.remove("_options").unwrap_or(Value::Null)) {
        Ok(o) => o,
        Err(e) => return fail(m, e),
    };
    let records = match m.get(&o.records_field) {
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return fail(m, format!("field '{}' must be a list of records", o.records_field)),
        None => return fail(m, format!("field '{}' not found (put the rows to write in it, or set _options.records_field)", o.records_field)),
    };
    if let Some(i) = records.iter().position(|r| !r.is_object()) {
        return fail(m, format!("row {} is not a JSON object", i + 1));
    }
    let columns = match build_columns(&records, &o) {
        Ok(c) => c,
        Err(e) => return fail(m, e),
    };
    match write_parquet(&records, &columns, &o) {
        Ok((bytes, groups)) => {
            if !o.keep_source {
                m.remove(&o.records_field);
            }
            m.insert(o.into.clone(), json!(base64::engine::general_purpose::STANDARD.encode(&bytes)));
            m.insert("row_count".into(), json!(records.len()));
            m.insert("row_groups".into(), json!(groups));
            m.insert("bytes".into(), json!(bytes.len()));
            m.insert("compression".into(), json!(o.compression_name));
            m.insert("columns".into(), Value::Array(columns.iter().map(|c| json!({ "name": c.name, "type": c.kind.name() })).collect()));
            m.insert("ok".into(), json!(true));
            Value::Object(m)
        }
        Err(e) => fail(m, e),
    }
}

fn fail(mut m: Map<String, Value>, error: String) -> Value {
    m.insert("ok".into(), json!(false));
    m.insert("error".into(), json!(error));
    Value::Object(m)
}

/// Columns from `_options.columns`, or every top-level field in order of first appearance.
/// Missing types are inferred from the values.
fn build_columns(records: &[Value], o: &Opts) -> Result<Vec<Column>, String> {
    let specs: Vec<(String, Option<String>, Option<String>)> = match &o.columns {
        Some(c) => c.clone(),
        None => {
            let mut seen: Vec<String> = Vec::new();
            for r in records {
                for k in r.as_object().into_iter().flat_map(|m| m.keys()) {
                    if !seen.contains(k) {
                        seen.push(k.clone());
                    }
                }
            }
            if seen.is_empty() {
                return Err("no columns: the records are empty and _options.columns is not set".into());
            }
            seen.into_iter().map(|f| (f, None, None)).collect()
        }
    };
    let mut cols: Vec<Column> = Vec::new();
    for (field, name, ty) in specs {
        let name = name.unwrap_or_else(|| field.clone());
        if name.is_empty() {
            return Err("column names can't be empty".into());
        }
        if cols.iter().any(|c| c.name == name) {
            return Err(format!("column '{name}' appears twice"));
        }
        let kind = match ty {
            Some(t) => Kind::parse(&t)?,
            None => infer(records.iter().filter_map(|r| get_path(r, &field)), o.infer_dates),
        };
        cols.push(Column { field, name, kind });
    }
    Ok(cols)
}

/// Type from the non-null values: all booleans, all whole numbers, numbers, ISO dates or
/// timestamps (when `infer_dates`), objects/lists (json); anything else, or nothing, is string.
pub fn infer<'a>(values: impl Iterator<Item = &'a Value>, infer_dates: bool) -> Kind {
    let mut kind: Option<Kind> = None;
    for v in values {
        let k = match v {
            Value::Null => continue,
            Value::Bool(_) => Kind::Boolean,
            Value::Number(n) if n.is_i64() => Kind::Int64,
            Value::Number(_) => Kind::Double,
            Value::Object(_) | Value::Array(_) => Kind::Json,
            Value::String(s) if infer_dates && parse_date(s).is_some() => Kind::Date,
            Value::String(s) if infer_dates && parse_timestamp(s).is_some() => Kind::Timestamp { utc: parse_timestamp(s).unwrap().1 },
            Value::String(_) => Kind::String,
        };
        kind = Some(match (kind, k) {
            (None, k) => k,
            (Some(a), b) if a == b => a,
            (Some(Kind::Int64), Kind::Double) | (Some(Kind::Double), Kind::Int64) => Kind::Double,
            _ => return Kind::String,
        });
    }
    kind.unwrap_or(Kind::String)
}

/// Days since 1970-01-01 for "YYYY-MM-DD".
pub fn parse_date(s: &str) -> Option<i32> {
    if s.len() != 10 {
        return None;
    }
    let d = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    Some((d - chrono::NaiveDate::from_ymd_opt(1970, 1, 1)?).num_days() as i32)
}

/// Microseconds since the epoch and whether the text had a time zone. Accepts ISO 8601 with
/// `T` or a space, optional fractions, and `Z` or an offset; without a zone it is a local time.
pub fn parse_timestamp(s: &str) -> Option<(i64, bool)> {
    if s.len() < 19 || !s.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let t = s.replacen(' ', "T", 1);
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&t) {
        return Some((dt.timestamp_micros(), true));
    }
    chrono::NaiveDateTime::parse_from_str(&t, "%Y-%m-%dT%H:%M:%S%.f").ok().map(|n| (n.and_utc().timestamp_micros(), false))
}

/// A decimal value scaled to `scale` digits as an integer, rounding half away from zero.
pub fn parse_decimal(v: &Value, precision: u32, scale: u32) -> Result<i128, String> {
    let text = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.trim().replace([',', '_'], ""),
        _ => return Err(format!("expected a decimal number, got {v}")),
    };
    let (neg, digits) = match text.strip_prefix('-') {
        Some(r) => (true, r.to_owned()),
        None => (false, text.trim_start_matches('+').to_owned()),
    };
    // Plain decimals only; JSON numbers like 1e-7 are expanded first.
    let digits = if digits.contains(['e', 'E']) {
        let f: f64 = digits.parse().map_err(|_| format!("expected a decimal number, got {v}"))?;
        format!("{:.*}", scale as usize + 1, f)
    } else {
        digits
    };
    let (int_part, frac_part) = digits.split_once('.').unwrap_or((&digits, ""));
    if int_part.is_empty() && frac_part.is_empty()
        || !int_part.chars().all(|c| c.is_ascii_digit())
        || !frac_part.chars().all(|c| c.is_ascii_digit())
    {
        return Err(format!("expected a decimal number, got {v}"));
    }
    let mut frac: String = frac_part.chars().take(scale as usize).collect();
    while frac.len() < scale as usize {
        frac.push('0');
    }
    let round_up = frac_part.chars().nth(scale as usize).is_some_and(|c| c >= '5');
    let int_part = int_part.trim_start_matches('0');
    if int_part.len() + scale as usize > 38 {
        return Err(format!("{v} has too many digits for decimal({precision},{scale})"));
    }
    let mut n: i128 = format!("0{int_part}{frac}").parse().map_err(|_| format!("expected a decimal number, got {v}"))?;
    if round_up {
        n += 1;
    }
    if n >= 10i128.pow(precision) {
        return Err(format!("{v} has too many digits for decimal({precision},{scale})"));
    }
    Ok(if neg { -n } else { n })
}

/// Bytes needed for a FIXED_LEN_BYTE_ARRAY decimal of this precision (as the Parquet spec gives).
fn decimal_len(precision: u32) -> usize {
    (1..=16).find(|n| 2f64.powi(8 * *n as i32 - 1) - 1.0 >= 10f64.powi(precision as i32) - 1.0).unwrap_or(16) as usize
}

fn schema(cols: &[Column]) -> Result<Type, String> {
    let mut fields = Vec::new();
    for c in cols {
        let b = |p: Physical| Type::primitive_type_builder(&c.name, p).with_repetition(Repetition::OPTIONAL);
        let t = match c.kind {
            Kind::String => b(Physical::BYTE_ARRAY).with_logical_type(Some(LogicalType::String)),
            Kind::Json => b(Physical::BYTE_ARRAY).with_logical_type(Some(LogicalType::Json)),
            Kind::Int64 => b(Physical::INT64),
            Kind::Int32 => b(Physical::INT32),
            Kind::Double => b(Physical::DOUBLE),
            Kind::Boolean => b(Physical::BOOLEAN),
            Kind::Date => b(Physical::INT32).with_logical_type(Some(LogicalType::Date)),
            Kind::Timestamp { utc } => b(Physical::INT64).with_logical_type(Some(LogicalType::timestamp(utc, TimeUnit::MICROS))),
            Kind::Decimal { precision, scale } => {
                let p = if precision <= 9 {
                    b(Physical::INT32)
                } else if precision <= 18 {
                    b(Physical::INT64)
                } else {
                    b(Physical::FIXED_LEN_BYTE_ARRAY).with_length(decimal_len(precision) as i32)
                };
                p.with_logical_type(Some(LogicalType::decimal(scale as i32, precision as i32)))
                    .with_precision(precision as i32)
                    .with_scale(scale as i32)
            }
        };
        fields.push(Arc::new(t.build().map_err(|e| format!("column '{}': {e}", c.name))?));
    }
    Type::group_type_builder("schema").with_fields(fields).build().map_err(|e| e.to_string())
}

fn write_parquet(records: &[Value], cols: &[Column], o: &Opts) -> Result<(Vec<u8>, usize), String> {
    let schema = Arc::new(schema(cols)?);
    let props = WriterProperties::builder()
        .set_compression(o.compression)
        .set_created_by(format!("Neblex parquet-writer {}", env!("CARGO_PKG_VERSION")))
        .build();
    let mut buf: Vec<u8> = Vec::new();
    let mut writer = SerializedFileWriter::new(&mut buf, schema, Arc::new(props)).map_err(|e| e.to_string())?;
    let mut groups = 0;
    for (gi, chunk) in records.chunks(o.row_group_size).enumerate() {
        let first_row = gi * o.row_group_size;
        let mut rg = writer.next_row_group().map_err(|e| e.to_string())?;
        for c in cols {
            let mut cw = rg.next_column().map_err(|e| e.to_string())?.ok_or("column count mismatch")?;
            let mut defs: Vec<i16> = Vec::with_capacity(chunk.len());
            let at = |i: usize| -> Option<&Value> { get_path(&chunk[i], &c.field).filter(|v| !v.is_null()) };
            let err = |i: usize, msg: String| format!("row {}, column '{}': {msg}", first_row + i + 1, c.name);
            macro_rules! collect {
                ($conv:expr) => {{
                    let mut vals = Vec::new();
                    for i in 0..chunk.len() {
                        match at(i) {
                            None => defs.push(0),
                            Some(v) => {
                                vals.push($conv(v).map_err(|m: String| err(i, m))?);
                                defs.push(1);
                            }
                        }
                    }
                    vals
                }};
            }
            let res = match c.kind {
                Kind::String => {
                    let v = collect!(|v: &Value| Ok(ByteArray::from(match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }
                    .into_bytes())));
                    cw.typed::<ByteArrayType>().write_batch(&v, Some(&defs), None)
                }
                Kind::Json => {
                    let v = collect!(|v: &Value| Ok(ByteArray::from(v.to_string().into_bytes())));
                    cw.typed::<ByteArrayType>().write_batch(&v, Some(&defs), None)
                }
                Kind::Int64 => {
                    let v = collect!(|v: &Value| to_int(v));
                    cw.typed::<Int64Type>().write_batch(&v, Some(&defs), None)
                }
                Kind::Int32 => {
                    let v = collect!(|v: &Value| to_int(v).and_then(|n| i32::try_from(n).map_err(|_| format!("{n} is too large for int32"))));
                    cw.typed::<Int32Type>().write_batch(&v, Some(&defs), None)
                }
                Kind::Double => {
                    let v = collect!(|v: &Value| match v {
                        Value::Number(n) => n.as_f64().ok_or_else(|| format!("expected a number, got {v}")),
                        Value::String(s) => s.trim().parse::<f64>().map_err(|_| format!("expected a number, got {v}")),
                        _ => Err(format!("expected a number, got {v}")),
                    });
                    cw.typed::<DoubleType>().write_batch(&v, Some(&defs), None)
                }
                Kind::Boolean => {
                    let v = collect!(|v: &Value| match v {
                        Value::Bool(b) => Ok(*b),
                        Value::String(s) if matches!(s.to_lowercase().as_str(), "true" | "yes" | "1") => Ok(true),
                        Value::String(s) if matches!(s.to_lowercase().as_str(), "false" | "no" | "0") => Ok(false),
                        Value::Number(n) if n.as_i64() == Some(1) => Ok(true),
                        Value::Number(n) if n.as_i64() == Some(0) => Ok(false),
                        _ => Err(format!("expected true or false, got {v}")),
                    });
                    cw.typed::<BoolType>().write_batch(&v, Some(&defs), None)
                }
                Kind::Date => {
                    let v = collect!(|v: &Value| v
                        .as_str()
                        .and_then(|s| parse_date(s).or_else(|| parse_timestamp(s).map(|(us, _)| us.div_euclid(86_400_000_000) as i32)))
                        .ok_or_else(|| format!("expected a date like 2026-01-31, got {v}")));
                    cw.typed::<Int32Type>().write_batch(&v, Some(&defs), None)
                }
                Kind::Timestamp { .. } => {
                    let v = collect!(|v: &Value| match v {
                        Value::String(s) => parse_timestamp(s)
                            .map(|t| t.0)
                            .or_else(|| parse_date(s).map(|d| d as i64 * 86_400_000_000))
                            .ok_or_else(|| format!("expected an ISO 8601 date and time like 2026-01-31T14:05:09Z, got {v}")),
                        _ => Err(format!("expected an ISO 8601 date and time like 2026-01-31T14:05:09Z, got {v}")),
                    });
                    cw.typed::<Int64Type>().write_batch(&v, Some(&defs), None)
                }
                Kind::Decimal { precision, scale } => {
                    if precision <= 9 {
                        let v = collect!(|v: &Value| parse_decimal(v, precision, scale).map(|n| n as i32));
                        cw.typed::<Int32Type>().write_batch(&v, Some(&defs), None)
                    } else if precision <= 18 {
                        let v = collect!(|v: &Value| parse_decimal(v, precision, scale).map(|n| n as i64));
                        cw.typed::<Int64Type>().write_batch(&v, Some(&defs), None)
                    } else {
                        let len = decimal_len(precision);
                        let v = collect!(|v: &Value| parse_decimal(v, precision, scale)
                            .map(|n| FixedLenByteArray::from(n.to_be_bytes()[16 - len..].to_vec())));
                        cw.typed::<FixedLenByteArrayType>().write_batch(&v, Some(&defs), None)
                    }
                }
            };
            res.map_err(|e| format!("column '{}': {e}", c.name))?;
            cw.close().map_err(|e| format!("column '{}': {e}", c.name))?;
        }
        rg.close().map_err(|e| e.to_string())?;
        groups += 1;
    }
    writer.close().map_err(|e| e.to_string())?;
    legacy_float_order(&mut buf, cols.len());
    Ok((buf, groups))
}

/// The parquet crate marks float and double columns with the IEEE 754 total-order column
/// order (Parquet format 2.11, 2025). Readers that predate it, such as Polars 1.35, reject the
/// whole file ("Invalid thrift: bad data"). pyarrow and older writers mark them with the
/// type-defined order instead, which means the same for min/max statistics apart from NaN.
///
/// `column_orders` is the last field of the footer: field header 0x19, a list header, one
/// 3-byte union per column (field 1 = type-defined `1C 00 00`, field 2 = IEEE 754 `2C 00 00`)
/// and the footer's closing 0x00. Both unions are 3 bytes, so the swap moves no offsets. When
/// the footer doesn't end exactly like that, the file is left as written.
fn legacy_float_order(buf: &mut [u8], columns: usize) {
    let n = buf.len();
    if n < 12 || &buf[n - 4..] != b"PAR1" {
        return;
    }
    let footer_len = u32::from_le_bytes([buf[n - 8], buf[n - 7], buf[n - 6], buf[n - 5]]) as usize;
    let end = n - 8; // one past the footer's last byte
    let list_bytes = columns * 3;
    let mut header = vec![0x19u8];
    if columns < 15 {
        header.push(((columns as u8) << 4) | 0x0c);
    } else {
        header.push(0xfc);
        let mut v = columns;
        while v >= 0x80 {
            header.push((v as u8 & 0x7f) | 0x80);
            v >>= 7;
        }
        header.push(v as u8);
    }
    let Some(start) = end.checked_sub(1 + list_bytes + header.len()) else { return };
    if footer_len > end || start < end - footer_len || buf[start..start + header.len()] != header[..] || buf[end - 1] != 0 {
        return;
    }
    let items = start + header.len();
    let ok = (0..columns).all(|i| {
        let u = &buf[items + i * 3..items + i * 3 + 3];
        matches!(u[0], 0x1c | 0x2c | 0x3c) && u[1] == 0 && u[2] == 0
    });
    if ok {
        for i in 0..columns {
            if buf[items + i * 3] == 0x2c {
                buf[items + i * 3] = 0x1c;
            }
        }
    }
}

fn to_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 9.2e18).map(|f| f as i64))
            .ok_or_else(|| format!("expected a whole number, got {v}")),
        Value::String(s) => s.trim().parse::<i64>().map_err(|_| format!("expected a whole number, got {v}")),
        _ => Err(format!("expected a whole number, got {v}")),
    }
}

#[cfg(test)]
mod tests;
