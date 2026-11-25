//! Neblex sandboxed module: decode Sparkplug B MQTT messages (the protobuf payloads that
//! factory-floor devices, PLC gateways and SCADA systems publish) into JSON metrics with names,
//! data types, values and times, and encode command payloads (NCMD/DCMD) to send back.
//! Handles every Sparkplug B data type, including arrays, DataSets, Templates (UDTs) and
//! property sets, and resolves metric aliases with a map saved from the BIRTH message.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the fields.

mod pb;

use pb::{Reader, Wire, Writer};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

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

/// One record in, one record out; with `_options.rows` a record becomes one record per metric,
/// and an array of records comes back as one flat array.
pub fn run(input: &[u8]) -> Value {
    match serde_json::from_slice::<Value>(input) {
        Ok(Value::Array(items)) => {
            let mut out = Vec::new();
            for item in items {
                match handle(item) {
                    Value::Array(rows) => out.extend(rows),
                    v => out.push(v),
                }
            }
            Value::Array(out)
        }
        Ok(v) => handle(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

// ---------- data types ----------

const TYPES: [&str; 35] = [
    "Unknown", "Int8", "Int16", "Int32", "Int64", "UInt8", "UInt16", "UInt32", "UInt64", "Float", "Double",
    "Boolean", "String", "DateTime", "Text", "UUID", "DataSet", "Bytes", "File", "Template", "PropertySet",
    "PropertySetList", "Int8Array", "Int16Array", "Int32Array", "Int64Array", "UInt8Array", "UInt16Array",
    "UInt32Array", "UInt64Array", "FloatArray", "DoubleArray", "BooleanArray", "StringArray", "DateTimeArray",
];

/// Sparkplug data type name for a code (10 -> "Double").
pub fn type_name(code: u32) -> String {
    TYPES.get(code as usize).map(|s| s.to_string()).unwrap_or_else(|| format!("Type{code}"))
}

/// Data type code for a name ("double" or "Double" -> 10), or a number given as text.
pub fn type_code(name: &str) -> Option<u32> {
    TYPES.iter().position(|t| t.eq_ignore_ascii_case(name.trim())).map(|i| i as u32).or_else(|| name.trim().parse().ok())
}

/// A value as found on the wire, before the data type is applied.
enum Raw<'a> {
    None,
    Int(u32),
    Long(u64),
    Float(f32),
    Double(f64),
    Bool(bool),
    Str(String),
    Bytes(&'a [u8]),
    DataSet(&'a [u8]),
    Template(&'a [u8]),
    Set(&'a [u8]),
    SetList(&'a [u8]),
    Ext,
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// f32 as the shortest decimal that reads back the same (0.1f32 -> 0.1, not 0.100000001...).
fn f32_json(f: f32) -> Value {
    if !f.is_finite() {
        return Value::Null;
    }
    format!("{f}").parse::<f64>().map(|d| json!(d)).unwrap_or(Value::Null)
}

fn f64_json(f: f64) -> Value {
    if f.is_finite() { json!(f) } else { Value::Null }
}

/// ISO 8601 UTC time for epoch milliseconds (1732443330123 -> "2024-11-24T10:15:30.123Z").
pub fn iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

#[derive(Clone, Copy, PartialEq)]
pub enum ArrayStyle {
    /// Sparkplug 3.0 spec and Eclipse Tahu Java: booleans most significant bit first, strings
    /// as UTF-8 each ended by a zero byte.
    Spec,
    /// Eclipse Tahu Python: booleans least significant bit first, strings hex-encoded.
    Python,
}

struct Ctx {
    style: ArrayStyle,
    /// alias -> (name, data type) from `_options.aliases`.
    known: HashMap<u64, (String, Option<u32>)>,
    /// name -> data type, for DATA messages that leave the data type out.
    types_by_name: HashMap<String, u32>,
}

/// Applies a data type to a wire value. Signed integers are read from their low bits, so both
/// sign-extended (Tahu Java) and masked encodings decode the same.
fn scalar(dt: u32, raw: &Raw, ctx: &Ctx) -> Result<Value, String> {
    let int = match raw {
        Raw::Int(v) => Some(u64::from(*v)),
        Raw::Long(v) => Some(*v),
        _ => None,
    };
    Ok(match (dt, raw) {
        (_, Raw::None) => Value::Null,
        (1, _) if int.is_some() => json!(int.unwrap() as u8 as i8),
        (2, _) if int.is_some() => json!(int.unwrap() as u16 as i16),
        (3, _) if int.is_some() => json!(int.unwrap() as u32 as i32),
        (4 | 13, _) if int.is_some() => json!(int.unwrap() as i64),
        (5, _) if int.is_some() => json!(int.unwrap() as u8),
        (6, _) if int.is_some() => json!(int.unwrap() as u16),
        (7, _) if int.is_some() => json!(int.unwrap() as u32),
        (_, Raw::Int(v)) => json!(v),
        (_, Raw::Long(v)) => json!(v),
        (_, Raw::Float(f)) => f32_json(*f),
        (_, Raw::Double(f)) => f64_json(*f),
        (_, Raw::Bool(b)) => json!(b),
        (_, Raw::Str(s)) => json!(s),
        (22..=34, Raw::Bytes(b)) => array(dt, b, ctx.style)?,
        (_, Raw::Bytes(b)) => json!(b64_encode(b)),
        (_, Raw::DataSet(b)) => dataset(b, ctx)?,
        (_, Raw::Template(b)) => template(b, ctx)?,
        (_, Raw::Set(b)) => property_set(b, ctx)?,
        (_, Raw::SetList(b)) => property_set_list(b, ctx)?,
        (_, Raw::Ext) => Value::Null,
    })
}

/// Decodes a Sparkplug array (data types 22-34) packed little-endian into bytes.
fn array(dt: u32, b: &[u8], style: ArrayStyle) -> Result<Value, String> {
    fn chunks<const N: usize>(b: &[u8], name: &str) -> Result<Vec<[u8; N]>, String> {
        if b.len() % N != 0 {
            return Err(format!("{name} has {} bytes, not a multiple of {N}", b.len()));
        }
        Ok(b.chunks_exact(N).map(|c| c.try_into().unwrap()).collect())
    }
    let name = type_name(dt);
    Ok(match dt {
        22 => json!(b.iter().map(|x| *x as i8).collect::<Vec<_>>()),
        26 => json!(b.to_vec()),
        23 => json!(chunks::<2>(b, &name)?.into_iter().map(i16::from_le_bytes).collect::<Vec<_>>()),
        27 => json!(chunks::<2>(b, &name)?.into_iter().map(u16::from_le_bytes).collect::<Vec<_>>()),
        24 => json!(chunks::<4>(b, &name)?.into_iter().map(i32::from_le_bytes).collect::<Vec<_>>()),
        28 => json!(chunks::<4>(b, &name)?.into_iter().map(u32::from_le_bytes).collect::<Vec<_>>()),
        25 | 34 => json!(chunks::<8>(b, &name)?.into_iter().map(i64::from_le_bytes).collect::<Vec<_>>()),
        29 => json!(chunks::<8>(b, &name)?.into_iter().map(u64::from_le_bytes).collect::<Vec<_>>()),
        30 => Value::Array(chunks::<4>(b, &name)?.into_iter().map(|c| f32_json(f32::from_le_bytes(c))).collect()),
        31 => Value::Array(chunks::<8>(b, &name)?.into_iter().map(|c| f64_json(f64::from_le_bytes(c))).collect()),
        32 => {
            if b.len() < 4 {
                return Err("BooleanArray is shorter than its 4-byte count".into());
            }
            let n = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
            let bits = &b[4..];
            if bits.len() * 8 < n {
                return Err(format!("BooleanArray says {n} values but has {} bits", bits.len() * 8));
            }
            json!((0..n)
                .map(|i| {
                    let byte = bits[i / 8];
                    match style {
                        ArrayStyle::Spec => byte & (0x80 >> (i % 8)) != 0,
                        ArrayStyle::Python => byte & (1 << (i % 8)) != 0,
                    }
                })
                .collect::<Vec<_>>())
        }
        33 => {
            let body = b.strip_suffix(&[0]).unwrap_or(b);
            if body.is_empty() && b.len() <= 1 {
                return Ok(json!(if b.is_empty() { vec![] } else { vec![String::new()] }));
            }
            let parts = body.split(|x| *x == 0).map(|s| match style {
                ArrayStyle::Spec => Ok(text(s)),
                ArrayStyle::Python => hex_decode(s).map(|v| text(&v)).ok_or_else(|| "StringArray is not hex (try array_style spec)".to_owned()),
            });
            Value::Array(parts.collect::<Result<Vec<_>, _>>()?.into_iter().map(Value::String).collect())
        }
        _ => json!(b64_encode(b)),
    })
}

fn hex_decode(s: &[u8]) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    s.chunks(2).map(|c| u8::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok()).collect()
}

// ---------- messages ----------

/// A metric as JSON: name, alias, datatype, value, timestamp/time, flags that are set,
/// properties and metadata.
fn metric(buf: &[u8], ctx: &Ctx, aliases_out: &mut Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut r = Reader::new(buf);
    let (mut name, mut alias, mut ts, mut dt) = (None, None, None, None);
    let (mut hist, mut trans, mut null) = (false, false, false);
    let (mut meta, mut props) = (None, None);
    let mut raw = Raw::None;
    while let Some((f, w)) = r.next()? {
        match (f, w) {
            (1, Wire::Bytes(b)) => name = Some(text(b)),
            (2, Wire::Varint(v)) => alias = Some(v),
            (3, Wire::Varint(v)) => ts = Some(v as i64),
            (4, Wire::Varint(v)) => dt = Some(v as u32),
            (5, Wire::Varint(v)) => hist = v != 0,
            (6, Wire::Varint(v)) => trans = v != 0,
            (7, Wire::Varint(v)) => null = v != 0,
            (8, Wire::Bytes(b)) => meta = Some(metadata(b)?),
            (9, Wire::Bytes(b)) => props = Some(property_set(b, ctx)?),
            (10, Wire::Varint(v)) => raw = Raw::Int(v as u32),
            (11, Wire::Varint(v)) => raw = Raw::Long(v),
            (12, Wire::Fixed32(v)) => raw = Raw::Float(f32::from_bits(v)),
            (13, Wire::Fixed64(v)) => raw = Raw::Double(f64::from_bits(v)),
            (14, Wire::Varint(v)) => raw = Raw::Bool(v != 0),
            (15, Wire::Bytes(b)) => raw = Raw::Str(text(b)),
            (16, Wire::Bytes(b)) => raw = Raw::Bytes(b),
            (17, Wire::Bytes(b)) => raw = Raw::DataSet(b),
            (18, Wire::Bytes(b)) => raw = Raw::Template(b),
            (19, _) => raw = Raw::Ext,
            _ => {}
        }
    }
    let mut from_alias = false;
    if let (None, Some(a)) = (&name, alias) {
        if let Some((n, t)) = ctx.known.get(&a) {
            name = Some(n.clone());
            from_alias = true;
            dt = dt.or(*t);
        }
    }
    if dt.is_none() {
        dt = name.as_ref().and_then(|n| ctx.types_by_name.get(n).copied());
    }
    if let (Some(n), Some(a)) = (&name, alias) {
        if !from_alias {
            aliases_out.insert(a.to_string(), json!({ "name": n, "datatype": dt.map(type_name) }));
        }
    }
    let mut m = Map::new();
    m.insert("name".into(), json!(name));
    m.insert("alias".into(), json!(alias));
    m.insert("datatype".into(), json!(dt.map(type_name)));
    let value = if null { Ok(Value::Null) } else { scalar(dt.unwrap_or(0), &raw, ctx) };
    match value {
        Ok(v) => {
            if dt == Some(13) {
                if let Some(ms) = v.as_i64() {
                    m.insert("value_time".into(), json!(iso(ms)));
                }
            }
            m.insert("value".into(), v);
        }
        Err(e) => {
            m.insert("value".into(), Value::Null);
            m.insert("error".into(), json!(e));
        }
    }
    m.insert("timestamp".into(), json!(ts));
    m.insert("time".into(), json!(ts.map(iso)));
    if hist {
        m.insert("is_historical".into(), json!(true));
    }
    if trans {
        m.insert("is_transient".into(), json!(true));
    }
    if null {
        m.insert("is_null".into(), json!(true));
    }
    if let Some(p) = props {
        m.insert("properties".into(), p);
    }
    if let Some(md) = meta {
        m.insert("metadata".into(), md);
    }
    Ok(m)
}

fn metadata(buf: &[u8]) -> Result<Value, String> {
    let mut r = Reader::new(buf);
    let mut m = Map::new();
    while let Some((f, w)) = r.next()? {
        match (f, w) {
            (1, Wire::Varint(v)) => m.insert("is_multi_part".into(), json!(v != 0)),
            (2, Wire::Bytes(b)) => m.insert("content_type".into(), json!(text(b))),
            (3, Wire::Varint(v)) => m.insert("size".into(), json!(v)),
            (4, Wire::Varint(v)) => m.insert("seq".into(), json!(v)),
            (5, Wire::Bytes(b)) => m.insert("file_name".into(), json!(text(b))),
            (6, Wire::Bytes(b)) => m.insert("file_type".into(), json!(text(b))),
            (7, Wire::Bytes(b)) => m.insert("md5".into(), json!(text(b))),
            (8, Wire::Bytes(b)) => m.insert("description".into(), json!(text(b))),
            _ => None,
        };
    }
    Ok(Value::Object(m))
}

/// A property set as an object ({"engUnit": "degC", "engHigh": 150}).
fn property_set(buf: &[u8], ctx: &Ctx) -> Result<Value, String> {
    let mut r = Reader::new(buf);
    let (mut keys, mut values) = (Vec::new(), Vec::new());
    while let Some((f, w)) = r.next()? {
        match (f, w) {
            (1, Wire::Bytes(b)) => keys.push(text(b)),
            (2, Wire::Bytes(b)) => values.push(property_value(b, ctx)?),
            _ => {}
        }
    }
    let mut m = Map::new();
    for (i, k) in keys.into_iter().enumerate() {
        m.insert(k, values.get(i).cloned().unwrap_or(Value::Null));
    }
    Ok(Value::Object(m))
}

fn property_set_list(buf: &[u8], ctx: &Ctx) -> Result<Value, String> {
    let mut r = Reader::new(buf);
    let mut out = Vec::new();
    while let Some((f, w)) = r.next()? {
        if let (1, Wire::Bytes(b)) = (f, w) {
            out.push(property_set(b, ctx)?);
        }
    }
    Ok(Value::Array(out))
}

fn property_value(buf: &[u8], ctx: &Ctx) -> Result<Value, String> {
    let mut r = Reader::new(buf);
    let (mut dt, mut null, mut raw) = (0, false, Raw::None);
    while let Some((f, w)) = r.next()? {
        match (f, w) {
            (1, Wire::Varint(v)) => dt = v as u32,
            (2, Wire::Varint(v)) => null = v != 0,
            (3, Wire::Varint(v)) => raw = Raw::Int(v as u32),
            (4, Wire::Varint(v)) => raw = Raw::Long(v),
            (5, Wire::Fixed32(v)) => raw = Raw::Float(f32::from_bits(v)),
            (6, Wire::Fixed64(v)) => raw = Raw::Double(f64::from_bits(v)),
            (7, Wire::Varint(v)) => raw = Raw::Bool(v != 0),
            (8, Wire::Bytes(b)) => raw = Raw::Str(text(b)),
            (9, Wire::Bytes(b)) => raw = Raw::Set(b),
            (10, Wire::Bytes(b)) => raw = Raw::SetList(b),
            (11, _) => raw = Raw::Ext,
            _ => {}
        }
    }
    if null { Ok(Value::Null) } else { scalar(dt, &raw, ctx) }
}

/// A DataSet as {columns, types, rows: [{column: value}]}.
fn dataset(buf: &[u8], ctx: &Ctx) -> Result<Value, String> {
    let mut r = Reader::new(buf);
    let (mut cols, mut types, mut rows) = (Vec::new(), Vec::new(), Vec::new());
    while let Some((f, w)) = r.next()? {
        match (f, w) {
            (2, Wire::Bytes(b)) => cols.push(text(b)),
            (3, Wire::Varint(v)) => types.push(v as u32),
            (3, Wire::Bytes(b)) => {
                // Packed repeated varints.
                let mut i = 0;
                while i < b.len() {
                    let (mut v, mut shift) = (0u64, 0);
                    loop {
                        let byte = *b.get(i).ok_or("DataSet types end inside a number")?;
                        i += 1;
                        v |= u64::from(byte & 0x7f) << shift;
                        shift += 7;
                        if byte & 0x80 == 0 || shift >= 64 {
                            break;
                        }
                    }
                    types.push(v as u32);
                }
            }
            (4, Wire::Bytes(b)) => rows.push(b),
            _ => {}
        }
    }
    let mut out_rows = Vec::new();
    for row in rows {
        let mut rr = Reader::new(row);
        let mut obj = Map::new();
        let mut i = 0;
        while let Some((f, w)) = rr.next()? {
            if let (1, Wire::Bytes(el)) = (f, w) {
                let mut er = Reader::new(el);
                let mut raw = Raw::None;
                while let Some((ef, ew)) = er.next()? {
                    raw = match (ef, ew) {
                        (1, Wire::Varint(v)) => Raw::Int(v as u32),
                        (2, Wire::Varint(v)) => Raw::Long(v),
                        (3, Wire::Fixed32(v)) => Raw::Float(f32::from_bits(v)),
                        (4, Wire::Fixed64(v)) => Raw::Double(f64::from_bits(v)),
                        (5, Wire::Varint(v)) => Raw::Bool(v != 0),
                        (6, Wire::Bytes(b)) => Raw::Str(text(b)),
                        _ => raw,
                    };
                }
                let key = cols.get(i).cloned().unwrap_or_else(|| format!("column_{}", i + 1));
                obj.insert(key, scalar(types.get(i).copied().unwrap_or(0), &raw, ctx)?);
                i += 1;
            }
        }
        out_rows.push(Value::Object(obj));
    }
    Ok(json!({ "columns": cols, "types": types.iter().map(|t| type_name(*t)).collect::<Vec<_>>(), "rows": out_rows }))
}

/// A Template (UDT) as {template_ref, version, is_definition, parameters, metrics}.
fn template(buf: &[u8], ctx: &Ctx) -> Result<Value, String> {
    let mut r = Reader::new(buf);
    let mut m = Map::new();
    let mut params = Map::new();
    let mut metrics = Vec::new();
    let mut ignored = Map::new();
    while let Some((f, w)) = r.next()? {
        match (f, w) {
            (1, Wire::Bytes(b)) => {
                m.insert("version".into(), json!(text(b)));
            }
            (2, Wire::Bytes(b)) => metrics.push(Value::Object(metric(b, ctx, &mut ignored)?)),
            (3, Wire::Bytes(b)) => {
                let mut pr = Reader::new(b);
                let (mut name, mut dt, mut raw) = (String::new(), 0, Raw::None);
                while let Some((pf, pw)) = pr.next()? {
                    match (pf, pw) {
                        (1, Wire::Bytes(s)) => name = text(s),
                        (2, Wire::Varint(v)) => dt = v as u32,
                        (3, Wire::Varint(v)) => raw = Raw::Int(v as u32),
                        (4, Wire::Varint(v)) => raw = Raw::Long(v),
                        (5, Wire::Fixed32(v)) => raw = Raw::Float(f32::from_bits(v)),
                        (6, Wire::Fixed64(v)) => raw = Raw::Double(f64::from_bits(v)),
                        (7, Wire::Varint(v)) => raw = Raw::Bool(v != 0),
                        (8, Wire::Bytes(s)) => raw = Raw::Str(text(s)),
                        _ => {}
                    }
                }
                params.insert(name, scalar(dt, &raw, ctx)?);
            }
            (4, Wire::Bytes(b)) => {
                m.insert("template_ref".into(), json!(text(b)));
            }
            (5, Wire::Varint(v)) => {
                m.insert("is_definition".into(), json!(v != 0));
            }
            _ => {}
        }
    }
    m.insert("parameters".into(), Value::Object(params));
    m.insert("metrics".into(), Value::Array(metrics));
    Ok(Value::Object(m))
}

/// Topic parts: spBv1.0/{group}/{message type}/{edge node}[/{device}], or the STATE topic.
pub fn parse_topic(topic: &str) -> Result<Map<String, Value>, String> {
    let parts: Vec<&str> = topic.split('/').collect();
    let mut m = Map::new();
    match parts.as_slice() {
        ["spBv1.0", "STATE", host] | ["STATE", host] => {
            m.insert("namespace".into(), json!(if parts.len() == 3 { "spBv1.0" } else { "" }));
            m.insert("message_type".into(), json!("STATE"));
            m.insert("host_id".into(), json!(host));
        }
        ["spBv1.0", group, kind, edge, rest @ ..] if rest.len() <= 1 => {
            let kinds = ["NBIRTH", "NDEATH", "DBIRTH", "DDEATH", "NDATA", "DDATA", "NCMD", "DCMD"];
            if !kinds.contains(kind) {
                return Err(format!("unknown Sparkplug message type '{kind}' in topic"));
            }
            if kind.starts_with('D') && rest.is_empty() {
                return Err(format!("{kind} topic has no device id"));
            }
            m.insert("namespace".into(), json!("spBv1.0"));
            m.insert("group_id".into(), json!(group));
            m.insert("message_type".into(), json!(kind));
            m.insert("edge_node_id".into(), json!(edge));
            m.insert("device_id".into(), json!(rest.first()));
        }
        _ => return Err(format!("'{topic}' is not a Sparkplug B topic (spBv1.0/group/TYPE/edge_node[/device])")),
    }
    Ok(m)
}

// ---------- base64 ----------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

/// Standard or URL-safe base64, padding optional, whitespace ignored.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

// ---------- records ----------

fn fail(mut m: Map<String, Value>, error: String) -> Value {
    m.insert("ok".into(), json!(false));
    m.insert("error".into(), json!(error));
    Value::Object(m)
}

fn handle(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    match opt.get("op").and_then(Value::as_str).unwrap_or("decode") {
        "decode" => decode_record(m, &opt),
        "encode" => encode_record(m, &opt),
        other => fail(m, format!("unknown op '{other}': use decode or encode")),
    }
}

/// Decodes `payload` (base64, or hex with `_options.encoding`) using `topic` for the message
/// parts. Options: `aliases` from an earlier BIRTH, `last_seq` to flag gaps, `rows` for one
/// record per metric, `array_style` spec|python.
fn decode_record(mut m: Map<String, Value>, opt: &Value) -> Value {
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("payload").to_owned();
    let topic_field = opt.get("topic_field").and_then(Value::as_str).unwrap_or("topic").to_owned();
    let style = match opt.get("array_style").and_then(Value::as_str).unwrap_or("spec") {
        "spec" | "java" => ArrayStyle::Spec,
        "python" => ArrayStyle::Python,
        other => return fail(m, format!("unknown array_style '{other}': use spec or python")),
    };
    let mut ctx = Ctx { style, known: HashMap::new(), types_by_name: HashMap::new() };
    if let Some(Value::Object(a)) = opt.get("aliases") {
        for (k, v) in a {
            let Ok(alias) = k.parse::<u64>() else {
                return fail(m, format!("alias '{k}' in _options.aliases is not a number"));
            };
            let (name, dt) = match v {
                Value::String(n) => (n.clone(), None),
                Value::Object(o) => (
                    o.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    o.get("datatype").and_then(Value::as_str).and_then(type_code),
                ),
                _ => continue,
            };
            if let Some(t) = dt {
                ctx.types_by_name.insert(name.clone(), t);
            }
            ctx.known.insert(alias, (name, dt));
        }
    }

    // Topic (optional, but needed for message type, seq checks and STATE).
    let topic = m.get(&topic_field).and_then(Value::as_str).map(str::to_owned);
    let mut out = Map::new();
    if let Some(t) = &topic {
        match parse_topic(t) {
            Ok(parts) => out.extend(parts),
            Err(e) => return fail(m, e),
        }
    }
    let kind = out.get("message_type").and_then(Value::as_str).unwrap_or("").to_owned();

    let raw = match m.get(&field) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => return fail(m, format!("field '{field}' is missing")),
        Some(_) => return fail(m, format!("field '{field}' is not text: pass the payload as base64")),
    };
    let bytes = match opt.get("encoding").and_then(Value::as_str).unwrap_or("base64") {
        "base64" => b64_decode(&raw),
        "hex" => hex_decode(raw.replace([' ', ':'], "").as_bytes()),
        "text" => Some(raw.clone().into_bytes()),
        other => return fail(m, format!("unknown encoding '{other}': use base64, hex or text")),
    };
    let Some(bytes) = bytes else {
        return fail(m, format!("field '{field}' is not valid {}", opt.get("encoding").and_then(Value::as_str).unwrap_or("base64")));
    };
    let keep = opt.get("keep_payload").and_then(Value::as_bool).unwrap_or(false);
    if !keep {
        m.remove(&field);
    }

    if kind == "STATE" {
        // STATE payloads are JSON (Sparkplug 3.0) or the text ONLINE/OFFLINE (2.x), not protobuf.
        let s = text(&bytes);
        let state = serde_json::from_str::<Value>(&s).unwrap_or_else(|_| json!({ "online": s.trim().eq_ignore_ascii_case("ONLINE") }));
        out.insert("state".into(), state);
        m.extend(out);
        m.insert("ok".into(), json!(true));
        return Value::Object(m);
    }

    let mut r = Reader::new(&bytes);
    let (mut ts, mut seq, mut uuid, mut body) = (None, None, None, None);
    let mut metrics = Vec::new();
    let mut aliases = Map::new();
    let res: Result<(), String> = (|| {
        while let Some((f, w)) = r.next()? {
            match (f, w) {
                (1, Wire::Varint(v)) => ts = Some(v as i64),
                (2, Wire::Bytes(b)) => metrics.push(metric(b, &ctx, &mut aliases)?),
                (3, Wire::Varint(v)) => seq = Some(v),
                (4, Wire::Bytes(b)) => uuid = Some(text(b)),
                (5, Wire::Bytes(b)) => body = Some(b64_encode(b)),
                _ => {}
            }
        }
        Ok(())
    })();
    if let Err(e) = res {
        return fail(m, format!("payload could not be decoded: {e}"));
    }

    out.insert("timestamp".into(), json!(ts));
    out.insert("time".into(), json!(ts.map(iso)));
    out.insert("seq".into(), json!(seq));
    if let Some(u) = uuid {
        out.insert("uuid".into(), json!(u));
    }
    if let Some(b) = body {
        out.insert("body".into(), json!(b));
    }
    if let (Some(last), Some(s)) = (opt.get("last_seq").and_then(Value::as_u64), seq) {
        // seq counts 0-255 and wraps; a BIRTH starts again at 0.
        let gap = !kind.ends_with("BIRTH") && s != (last + 1) % 256;
        out.insert("seq_gap".into(), json!(gap));
    }
    if kind.ends_with("BIRTH") || (!aliases.is_empty() && kind.is_empty()) {
        out.insert("aliases".into(), Value::Object(aliases));
    }
    let unresolved = metrics.iter().filter(|x| x.get("name").is_none_or(Value::is_null)).count();
    if unresolved > 0 {
        out.insert(
            "warning".into(),
            json!(format!("{unresolved} metric(s) have only an alias: pass the aliases map from this node's BIRTH message in _options.aliases")),
        );
    }

    if opt.get("rows").and_then(Value::as_bool).unwrap_or(false) {
        let mut base = m.clone();
        for k in ["namespace", "group_id", "message_type", "edge_node_id", "device_id", "seq", "seq_gap"] {
            if let Some(v) = out.get(k) {
                base.insert(k.into(), v.clone());
            }
        }
        let rows: Vec<Value> = metrics
            .into_iter()
            .map(|mut mm| {
                // A metric without its own time takes the payload's time.
                if mm.get("timestamp").is_none_or(Value::is_null) {
                    mm.insert("timestamp".into(), json!(ts));
                    mm.insert("time".into(), json!(ts.map(iso)));
                }
                let mut row = base.clone();
                row.extend(mm);
                row.insert("ok".into(), json!(true));
                Value::Object(row)
            })
            .collect();
        return Value::Array(rows);
    }
    out.insert("metrics".into(), Value::Array(metrics.into_iter().map(Value::Object).collect()));
    m.extend(out);
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

// ---------- encode ----------

fn to_i128(v: &Value) -> Option<i128> {
    v.as_i64().map(i128::from).or_else(|| v.as_u64().map(i128::from)).or_else(|| {
        v.as_f64().filter(|f| f.fract() == 0.0 && f.abs() < 1e19).map(|f| f as i128)
    })
}

fn int_in(v: &Value, lo: i128, hi: i128, name: &str) -> Result<i128, String> {
    let n = to_i128(v).ok_or_else(|| format!("{name} needs a whole number, got {v}"))?;
    if n < lo || n > hi {
        return Err(format!("{n} is out of range for {name} ({lo} to {hi})"));
    }
    Ok(n)
}

/// Packs a JSON array as a Sparkplug array value (spec layout).
fn pack_array(dt: u32, v: &Value, tn: &str) -> Result<Vec<u8>, String> {
    let items = v.as_array().ok_or_else(|| format!("{tn} needs an array"))?;
    let mut b = Vec::new();
    for x in items {
        match dt {
            22 => b.push(int_in(x, -128, 127, tn)? as i8 as u8),
            26 => b.push(int_in(x, 0, 255, tn)? as u8),
            23 => b.extend((int_in(x, -32768, 32767, tn)? as i16).to_le_bytes()),
            27 => b.extend((int_in(x, 0, 65535, tn)? as u16).to_le_bytes()),
            24 => b.extend((int_in(x, i32::MIN.into(), i32::MAX.into(), tn)? as i32).to_le_bytes()),
            28 => b.extend((int_in(x, 0, u32::MAX.into(), tn)? as u32).to_le_bytes()),
            25 | 34 => b.extend((int_in(x, i64::MIN.into(), i64::MAX.into(), tn)? as i64).to_le_bytes()),
            29 => b.extend((int_in(x, 0, u64::MAX.into(), tn)? as u64).to_le_bytes()),
            30 => b.extend((x.as_f64().ok_or(format!("{tn} needs numbers"))? as f32).to_le_bytes()),
            31 => b.extend(x.as_f64().ok_or(format!("{tn} needs numbers"))?.to_le_bytes()),
            33 => {
                b.extend(x.as_str().ok_or(format!("{tn} needs text items"))?.as_bytes());
                b.push(0);
            }
            _ => {}
        }
    }
    if dt == 32 {
        let bools = items.iter().map(|x| x.as_bool().ok_or(format!("{tn} needs true/false items"))).collect::<Result<Vec<_>, _>>()?;
        b.extend((bools.len() as u32).to_le_bytes());
        let mut bits = vec![0u8; bools.len().div_ceil(8)];
        for (i, on) in bools.iter().enumerate() {
            if *on {
                bits[i / 8] |= 0x80 >> (i % 8);
            }
        }
        b.extend(bits);
    }
    Ok(b)
}

fn encode_metric(mv: &Value) -> Result<Vec<u8>, String> {
    let o = mv.as_object().ok_or("each metric must be an object")?;
    let label = o.get("name").and_then(Value::as_str).map(str::to_owned).or_else(|| o.get("alias").map(|a| format!("alias {a}"))).unwrap_or_else(|| "metric".into());
    let tn = o.get("datatype").and_then(Value::as_str).ok_or_else(|| format!("{label}: datatype is required (Int32, Double, Boolean, String...)"))?;
    let dt = type_code(tn).filter(|c| (*c as usize) < TYPES.len()).ok_or_else(|| format!("{label}: unknown datatype '{tn}'"))?;
    let tn = type_name(dt);
    let mut w = Writer::default();
    if let Some(n) = o.get("name").and_then(Value::as_str) {
        w.bytes(1, n.as_bytes());
    }
    if let Some(a) = o.get("alias") {
        w.varint(2, a.as_u64().ok_or_else(|| format!("{label}: alias must be a whole number"))?);
    }
    if o.get("name").is_none() && o.get("alias").is_none() {
        return Err("each metric needs a name or an alias".into());
    }
    if let Some(t) = o.get("timestamp") {
        w.varint(3, t.as_u64().ok_or_else(|| format!("{label}: timestamp must be epoch milliseconds"))?);
    }
    w.varint(4, u64::from(dt));
    let value = o.get("value").unwrap_or(&Value::Null);
    if value.is_null() {
        w.varint(7, 1);
        return Ok(w.buf);
    }
    let e = |r: Result<i128, String>| r.map_err(|e| format!("{label}: {e}"));
    match dt {
        // Signed values are sign-extended to 32 bits like Tahu Java.
        1 => w.varint(10, u64::from(e(int_in(value, -128, 127, &tn))? as i32 as u32)),
        2 => w.varint(10, u64::from(e(int_in(value, -32768, 32767, &tn))? as i32 as u32)),
        3 => w.varint(10, u64::from(e(int_in(value, i32::MIN.into(), i32::MAX.into(), &tn))? as i32 as u32)),
        5 => w.varint(10, e(int_in(value, 0, 255, &tn))? as u64),
        6 => w.varint(10, e(int_in(value, 0, 65535, &tn))? as u64),
        7 => w.varint(11, e(int_in(value, 0, u32::MAX.into(), &tn))? as u64),
        4 | 13 => w.varint(11, e(int_in(value, i64::MIN.into(), i64::MAX.into(), &tn))? as i64 as u64),
        8 => w.varint(11, e(int_in(value, 0, u64::MAX.into(), &tn))? as u64),
        9 => w.fixed32(12, (value.as_f64().ok_or_else(|| format!("{label}: Float needs a number"))? as f32).to_bits()),
        10 => w.fixed64(13, value.as_f64().ok_or_else(|| format!("{label}: Double needs a number"))?.to_bits()),
        11 => w.varint(14, u64::from(value.as_bool().ok_or_else(|| format!("{label}: Boolean needs true or false"))?)),
        12 | 14 | 15 => w.bytes(15, value.as_str().ok_or_else(|| format!("{label}: {tn} needs text"))?.as_bytes()),
        17 | 18 => {
            let b = value.as_str().and_then(b64_decode).ok_or_else(|| format!("{label}: {tn} needs base64 text"))?;
            w.bytes(16, &b);
        }
        22..=34 => w.bytes(16, &pack_array(dt, value, &tn).map_err(|e| format!("{label}: {e}"))?),
        _ => return Err(format!("{label}: encoding {tn} is not supported (use scalar, Bytes or array types)")),
    }
    Ok(w.buf)
}

/// Builds a payload from `metrics` [{name|alias, datatype, value, timestamp?}], with optional
/// `timestamp` (default none), `seq` and `uuid`; adds `payload` (base64) and, when group_id,
/// message_type and edge_node_id are given, `topic`.
fn encode_record(mut m: Map<String, Value>, opt: &Value) -> Value {
    let Some(metrics) = m.get("metrics").and_then(Value::as_array).cloned() else {
        return fail(m, "encode needs a 'metrics' array".into());
    };
    let mut w = Writer::default();
    if let Some(t) = m.get("timestamp") {
        match t.as_u64() {
            Some(t) => w.varint(1, t),
            None => return fail(m, "timestamp must be epoch milliseconds".into()),
        }
    }
    for mv in &metrics {
        match encode_metric(mv) {
            Ok(b) => w.bytes(2, &b),
            Err(e) => return fail(m, e),
        }
    }
    if let Some(s) = m.get("seq") {
        match s.as_u64().filter(|s| *s <= 255) {
            Some(s) => w.varint(3, s),
            None => return fail(m, "seq must be 0-255".into()),
        }
    }
    if let Some(u) = m.get("uuid").and_then(Value::as_str) {
        w.bytes(4, u.as_bytes());
    }
    let g = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_owned);
    if let (Some(group), Some(kind), Some(edge)) = (g("group_id"), g("message_type"), g("edge_node_id")) {
        let topic = match g("device_id") {
            Some(d) => format!("spBv1.0/{group}/{kind}/{edge}/{d}"),
            None => format!("spBv1.0/{group}/{kind}/{edge}"),
        };
        if let Err(e) = parse_topic(&topic) {
            return fail(m, e);
        }
        m.insert("topic".into(), json!(topic));
    }
    let field = opt.get("into").and_then(Value::as_str).unwrap_or("payload").to_owned();
    m.insert("payload_size".into(), json!(w.buf.len()));
    m.insert(field, json!(b64_encode(&w.buf)));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

#[cfg(test)]
mod tests;
