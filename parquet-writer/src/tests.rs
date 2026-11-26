use super::*;
use base64::engine::general_purpose::STANDARD;

fn call(v: Value) -> Value {
    run(serde_json::to_vec(&v).unwrap().as_slice())
}
/// Writes with the module, then reads the file back with parquet-reader.
fn roundtrip(v: Value) -> (Value, Value) {
    let w = call(v);
    assert_eq!(w["ok"], true, "{w}");
    let r = parquet_reader::run(serde_json::to_vec(&json!({ "f": w["file"] })).unwrap().as_slice());
    assert_eq!(r["ok"], true, "{r}");
    (w, r)
}
fn types(r: &Value) -> Vec<String> {
    r["columns"].as_array().unwrap().iter().map(|c| c["type"].as_str().unwrap().to_owned()).collect()
}

fn orders() -> Value {
    json!([
        { "order_id": 1001, "customer": "Acme Corp", "amount": "1250.00", "qty": 3, "unit_price": 416.6667, "paid": true,
          "order_date": "2026-01-31", "created_at": "2026-01-31T14:05:09Z", "tags": ["rush", "b2b"], "ship_to": { "city": "Springfield", "zip": "01101" } },
        { "order_id": 1002, "customer": "Globex", "amount": "99.95", "qty": 1, "unit_price": 99.95, "paid": false,
          "order_date": "2026-02-28", "created_at": "2026-02-28T00:00:00Z", "tags": [], "ship_to": { "city": "Zürich", "zip": "8001" } },
        { "order_id": 1003, "customer": null, "amount": "0.10", "qty": null, "unit_price": 0.1, "paid": null,
          "order_date": "1999-12-31", "created_at": null, "tags": null, "ship_to": null },
        { "order_id": 1004, "customer": "Initech", "qty": 12, "unit_price": 5.5, "paid": true,
          "created_at": "2026-03-01T23:59:59Z", "tags": ["b2b"], "ship_to": { "city": "Austin", "zip": null } }
    ])
}

#[test]
fn inferred_types_round_trip() {
    let (w, r) = roundtrip(json!({ "batch": 7, "records": orders() }));
    assert_eq!(w["row_count"], 4);
    assert_eq!(w["batch"], 7);
    assert!(w.get("records").is_none(), "rows are not passed on by default");
    assert_eq!(w["compression"], "snappy");
    assert!(w["file"].as_str().unwrap().starts_with("UEFSM"));
    assert_eq!(
        w["columns"],
        json!([{ "name": "order_id", "type": "int64" }, { "name": "customer", "type": "string" }, { "name": "amount", "type": "string" },
               { "name": "qty", "type": "int64" }, { "name": "unit_price", "type": "double" }, { "name": "paid", "type": "boolean" },
               { "name": "order_date", "type": "date" }, { "name": "created_at", "type": "timestamp" }, { "name": "tags", "type": "json" },
               { "name": "ship_to", "type": "json" }])
    );
    assert_eq!(types(&r), ["int64", "string", "string", "int64", "double", "boolean", "date", "timestamp", "json", "json"]);
    assert_eq!(r["created_by"], format!("Neblex parquet-writer {}", env!("CARGO_PKG_VERSION")));
    assert_eq!(
        r["rows"][0],
        json!({ "order_id": 1001, "customer": "Acme Corp", "amount": "1250.00", "qty": 3, "unit_price": 416.6667, "paid": true,
                "order_date": "2026-01-31", "created_at": "2026-01-31T14:05:09Z", "tags": "[\"rush\",\"b2b\"]",
                "ship_to": "{\"city\":\"Springfield\",\"zip\":\"01101\"}" })
    );
    // A missing field and an explicit null both come back as null.
    assert_eq!(r["rows"][3]["amount"], Value::Null);
    assert_eq!(r["rows"][3]["order_date"], Value::Null);
    assert_eq!(r["rows"][2]["customer"], Value::Null);
    assert_eq!(r["rows"][1]["tags"], "[]");
}

#[test]
fn explicit_columns_names_and_types() {
    let (w, r) = roundtrip(json!({
        "records": orders(),
        "_options": { "columns": [
            { "field": "order_id", "type": "int32" },
            { "field": "ship_to.city", "name": "city" },
            { "field": "amount", "type": "decimal(12,2)" },
            { "field": "unit_price", "type": "decimal(30,4)" },
            { "field": "order_date", "name": "ordered_on", "type": "timestamp_local" },
            "paid"
        ] }
    }));
    assert_eq!(types(&r), ["int32", "string", "decimal(12,2)", "decimal(30,4)", "timestamp", "boolean"]);
    assert_eq!(w["columns"][4], json!({ "name": "ordered_on", "type": "timestamp_local" }));
    assert_eq!(
        r["rows"][0],
        json!({ "order_id": 1001, "city": "Springfield", "amount": "1250.00", "unit_price": "416.6667", "ordered_on": "2026-01-31T00:00:00", "paid": true })
    );
    assert_eq!(r["rows"][1]["city"], "Zürich");
    assert_eq!(r["rows"][2]["unit_price"], "0.1000");
    assert_eq!(r["rows"][3]["amount"], Value::Null);
}

#[test]
fn decimals() {
    let d = |v: Value, p, s| parse_decimal(&v, p, s);
    assert_eq!(d(json!("1250.00"), 12, 2), Ok(125000));
    assert_eq!(d(json!(99.95), 12, 2), Ok(9995));
    assert_eq!(d(json!("-0.005"), 12, 2), Ok(-1)); // half away from zero
    assert_eq!(d(json!("0.004"), 12, 2), Ok(0));
    assert_eq!(d(json!("1,234.5"), 12, 2), Ok(123450));
    assert_eq!(d(json!(7), 5, 0), Ok(7));
    assert_eq!(d(json!(1e-7), 12, 8), Ok(10));
    assert!(d(json!("123456"), 5, 2).unwrap_err().contains("too many digits"));
    assert!(d(json!("12a"), 5, 2).unwrap_err().contains("expected a decimal"));
    assert!(d(json!(true), 5, 2).is_err());
    // 38 digits, the largest Parquet decimal.
    let big = "12345678901234567890123456789012345678";
    let (_, r) = roundtrip(json!({ "records": [{ "x": big }, { "x": "-1" }], "_options": { "columns": [{ "field": "x", "type": "decimal(38,0)" }] } }));
    assert_eq!((r["rows"][0]["x"].clone(), r["rows"][1]["x"].clone()), (json!(big), json!("-1")));
}

#[test]
fn timestamps_and_dates() {
    assert_eq!(parse_timestamp("2026-01-31T14:05:09Z"), Some((1_769_868_309_000_000, true)));
    assert_eq!(parse_timestamp("2026-01-31T16:05:09.5+02:00"), Some((1_769_868_309_500_000, true)));
    assert_eq!(parse_timestamp("2026-01-31 14:05:09"), Some((1_769_868_309_000_000, false)));
    assert_eq!(parse_timestamp("2026-01-31"), None);
    assert_eq!(parse_timestamp("Jan 31 2026 14:05"), None);
    assert_eq!(parse_date("1970-01-02"), Some(1));
    assert_eq!(parse_date("1969-12-31"), Some(-1));
    assert_eq!(parse_date("2026-02-30"), None);
    let (_, r) = roundtrip(json!({ "records": [
        { "utc": "2026-01-31T16:05:09.123456+02:00", "local": "2026-01-31 14:05:09", "d": "2026-01-31" },
        { "utc": "1969-12-31T23:59:59.5Z", "local": "1969-12-31T23:59:59", "d": "1969-12-31" }
    ] }));
    assert_eq!(types(&r), ["timestamp", "timestamp", "date"]);
    assert_eq!(r["rows"][0], json!({ "utc": "2026-01-31T14:05:09.123456Z", "local": "2026-01-31T14:05:09", "d": "2026-01-31" }));
    assert_eq!(r["rows"][1], json!({ "utc": "1969-12-31T23:59:59.500Z", "local": "1969-12-31T23:59:59", "d": "1969-12-31" }));
    // infer_dates off keeps them as text.
    let (_, r) = roundtrip(json!({ "records": [{ "d": "2026-01-31" }], "_options": { "infer_dates": false } }));
    assert_eq!(types(&r), ["string"]);
}

#[test]
fn inference_rules() {
    let k = |vals: Value| infer(vals.as_array().unwrap().iter(), true);
    assert_eq!(k(json!([1, 2, null])), Kind::Int64);
    assert_eq!(k(json!([1, 2.5])), Kind::Double);
    assert_eq!(k(json!([true, null, false])), Kind::Boolean);
    assert_eq!(k(json!([{ "a": 1 }, [1]])), Kind::Json);
    assert_eq!(k(json!(["2026-01-31", "2026-02-01"])), Kind::Date);
    assert_eq!(k(json!(["2026-01-31T14:05:09Z", "2026-01-31T14:05:09+01:00"])), Kind::Timestamp { utc: true });
    // Mixed kinds fall back to text rather than failing.
    assert_eq!(k(json!(["2026-01-31", "soon"])), Kind::String);
    assert_eq!(k(json!([1, "2"])), Kind::String);
    assert_eq!(k(json!(["2026-01-31T14:05:09Z", "2026-01-31 14:05:09"])), Kind::String);
    assert_eq!(k(json!([null, null])), Kind::String);
    assert_eq!(k(json!([])), Kind::String);
    // Mixed values in a text column are written as JSON text.
    let (_, r) = roundtrip(json!({ "records": [{ "v": 1 }, { "v": "x" }, { "v": true }] }));
    assert_eq!(r["rows"], json!([{ "v": "1" }, { "v": "x" }, { "v": "true" }]));
}

#[test]
fn every_compression_reads_back() {
    let (_, reference) = roundtrip(json!({ "records": orders(), "_options": { "compression": "none" } }));
    for c in ["snappy", "gzip", "zstd", "lz4", "brotli", "SNAPPY"] {
        let (w, r) = roundtrip(json!({ "records": orders(), "_options": { "compression": c } }));
        assert_eq!(r["rows"], reference["rows"], "{c}");
        assert_eq!(w["compression"], c.to_lowercase());
    }
    let bad = call(json!({ "records": orders(), "_options": { "compression": "lzo" } }));
    assert!(bad["error"].as_str().unwrap().contains("snappy, gzip, zstd"));
}

#[test]
fn row_groups_and_large_input() {
    let rows: Vec<Value> = (0..25).map(|i| json!({ "n": i, "label": format!("row {i}") })).collect();
    let (w, r) = roundtrip(json!({ "records": rows, "_options": { "row_group_size": 10 } }));
    assert_eq!((w["row_groups"].clone(), r["row_groups"].clone(), r["row_count"].clone()), (json!(3), json!(3), json!(25)));
    assert_eq!(r["rows"][24], json!({ "n": 24, "label": "row 24" }));
    // Repetitive data compresses: 20,000 rows of 3 columns.
    let rows: Vec<Value> = (0..20_000).map(|i| json!({ "id": i, "status": if i % 3 == 0 { "open" } else { "closed" }, "amount": i as f64 * 1.25 })).collect();
    let (w, r) = roundtrip(json!({ "records": rows, "_options": { "compression": "zstd" } }));
    assert_eq!((w["row_groups"].clone(), r["row_count"].clone()), (json!(2), json!(20_000)));
    assert!(w["bytes"].as_u64().unwrap() < 200_000, "{}", w["bytes"]);
    assert_eq!(r["rows"][9_999], json!({ "id": 9_999, "status": "open", "amount": 12_498.75 }));
}

#[test]
fn empty_list_with_columns() {
    let (w, r) = roundtrip(json!({ "records": [], "_options": { "columns": [{ "field": "id", "type": "int64" }, "name"] } }));
    assert_eq!((w["row_count"].clone(), w["row_groups"].clone()), (json!(0), json!(0)));
    assert_eq!((r["row_count"].clone(), types(&r)), (json!(0), vec!["int64".to_owned(), "string".to_owned()]));
}

#[test]
fn options_and_passthrough() {
    let w = call(json!({ "data": { "rows": [{ "a": 1 }] }, "name": "x", "_options": { "records_field": "data", "into": "parquet", "keep_source": true } }));
    // records_field names a top-level field, so data must be the list itself.
    assert_eq!(w["error"], "field 'data' must be a list of records");
    let w = call(json!({ "rows": [{ "a": 1 }], "name": "x", "_options": { "records_field": "rows", "into": "parquet", "keep_source": true } }));
    assert_eq!(w["ok"], true);
    assert!(w["parquet"].as_str().unwrap().starts_with("UEFSM") && w.get("file").is_none());
    assert_eq!((w["rows"].clone(), w["name"].clone()), (json!([{ "a": 1 }]), json!("x")));
    // A list of input records gives a list of results.
    let out = call(json!([{ "records": [{ "a": 1 }] }, { "nope": 1 }]));
    assert_eq!((out[0]["ok"].clone(), out[1]["ok"].clone()), (json!(true), json!(false)));
}

#[test]
fn must_not_write() {
    let err = |v: Value| {
        let out = call(v);
        assert_eq!(out["ok"], false, "{out}");
        out["error"].as_str().unwrap().to_owned()
    };
    assert_eq!(err(json!({ "rows": [] })), "field 'records' not found (put the rows to write in it, or set _options.records_field)");
    assert_eq!(err(json!({ "records": [{ "a": 1 }, 5] })), "row 2 is not a JSON object");
    assert_eq!(err(json!({ "records": [] })), "no columns: the records are empty and _options.columns is not set");
    assert_eq!(err(json!({ "records": [{ "qty": "3" }, { "qty": "three" }], "_options": { "columns": [{ "field": "qty", "type": "int64" }] } })),
               "row 2, column 'qty': expected a whole number, got \"three\"");
    assert_eq!(err(json!({ "records": [{ "n": 3_000_000_000u64 }], "_options": { "columns": [{ "field": "n", "type": "int32" }] } })),
               "row 1, column 'n': 3000000000 is too large for int32");
    assert_eq!(err(json!({ "records": [{ "d": "31/01/2026" }], "_options": { "columns": [{ "field": "d", "type": "date" }] } })),
               "row 1, column 'd': expected a date like 2026-01-31, got \"31/01/2026\"");
    assert_eq!(err(json!({ "records": [{ "p": "yes please" }], "_options": { "columns": [{ "field": "p", "type": "boolean" }] } })),
               "row 1, column 'p': expected true or false, got \"yes please\"");
    assert!(err(json!({ "records": [{ "a": 1 }], "_options": { "columns": [{ "field": "a", "type": "money" }] } })).starts_with("unknown column type 'money'"));
    assert!(err(json!({ "records": [{ "a": 1 }], "_options": { "columns": [{ "field": "a", "type": "decimal(40,2)" }] } })).starts_with("unknown column type"));
    assert_eq!(err(json!({ "records": [{ "a": 1, "b": 2 }], "_options": { "columns": ["a", { "field": "b", "name": "a" }] } })), "column 'a' appears twice");
    assert_eq!(err(json!({ "records": [{ "a": 1 }], "_options": { "row_group_size": 0 } })), "_options.row_group_size must be a whole number 1 or more");
    assert_eq!(err(json!({ "records": [{ "a": 1 }], "_options": { "columns": [{ "name": "a" }] } })), "each entry in _options.columns needs a \"field\"");
    assert!(run(b"oops")["error"].as_str().unwrap().starts_with("input is not JSON"));
    // The base64 output never decodes to anything but a Parquet file.
    let w = call(json!({ "records": [{ "a": 1 }] }));
    let bytes = STANDARD.decode(w["file"].as_str().unwrap()).unwrap();
    assert_eq!((&bytes[..4], &bytes[bytes.len() - 4..]), (&b"PAR1"[..], &b"PAR1"[..]));
}

#[test]
fn float_columns_use_the_older_column_order() {
    // 2 and 20 columns: short and long list headers in the footer.
    for n in [2usize, 20] {
        let rec: Map<String, Value> = (0..n).map(|i| (format!("c{i}"), if i % 2 == 0 { json!(1.5) } else { json!(1) })).collect();
        let w = call(json!({ "records": [Value::Object(rec)] }));
        let b = STANDARD.decode(w["file"].as_str().unwrap()).unwrap();
        let footer_len = u32::from_le_bytes(b[b.len() - 8..b.len() - 4].try_into().unwrap()) as usize;
        let tail = &b[b.len() - 8 - 1 - n * 3..b.len() - 8 - 1];
        assert!(tail.chunks(3).all(|u| u == [0x1c, 0, 0]), "{n} columns: {tail:02x?}");
        assert!(footer_len < b.len());
        // Statistics and values still read back.
        let r = parquet_reader::run(serde_json::to_vec(&json!({ "f": w["file"] })).unwrap().as_slice());
        assert_eq!((r["ok"].clone(), r["rows"][0]["c0"].clone()), (json!(true), json!(1.5)));
    }
}
