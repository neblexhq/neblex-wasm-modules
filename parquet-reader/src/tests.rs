use super::*;
use base64::engine::general_purpose::STANDARD;

fn raw(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/test/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}
fn fixture(name: &str) -> String {
    STANDARD.encode(raw(name))
}
fn call(v: Value) -> Value {
    run(serde_json::to_vec(&v).unwrap().as_slice())
}
fn read(name: &str, options: Value) -> Value {
    let out = call(json!({ "file": fixture(name), "_options": options }));
    assert_eq!(out["ok"], true, "{name}: {out}");
    out
}

#[test]
fn orders_values() {
    let out = read("orders-snappy.parquet", json!({}));
    assert_eq!(out["row_count"], 4);
    assert_eq!(out["returned"], 4);
    assert_eq!(out["truncated"], false);
    assert_eq!(out["row_groups"], 1);
    assert_eq!(out["source_field"], "file");
    assert_eq!(out["created_by"], "parquet-cpp-arrow version 22.0.0");
    assert_eq!(
        out["rows"][0],
        json!({ "order_id": 1001, "customer": "Acme Corp", "amount": "1250.00", "qty": 3, "unit_price": 416.6667,
                "paid": true, "order_date": "2026-01-31", "created_at": "2026-01-31T14:05:09Z",
                "tags": ["rush", "b2b"], "ship_to": { "city": "Springfield", "zip": "01101" } })
    );
    // Unicode, empty list, nulls at every level.
    assert_eq!(out["rows"][1]["ship_to"]["city"], "Zürich");
    assert_eq!(out["rows"][1]["tags"], json!([]));
    assert_eq!(out["rows"][2], json!({ "order_id": 1003, "customer": null, "amount": "0.10", "qty": null, "unit_price": 0.1,
        "paid": null, "order_date": "1999-12-31", "created_at": null, "tags": null, "ship_to": null }));
    assert_eq!(out["rows"][3]["ship_to"], json!({ "city": "Austin", "zip": null }));
    // Columns keep the file's order.
    let keys: Vec<&String> = out["rows"][0].as_object().unwrap().keys().collect();
    assert_eq!(keys, ["order_id", "customer", "amount", "qty", "unit_price", "paid", "order_date", "created_at", "tags", "ship_to"]);
    assert_eq!(
        out["columns"],
        json!([
            { "name": "order_id", "type": "int64", "nullable": true }, { "name": "customer", "type": "string", "nullable": true },
            { "name": "amount", "type": "decimal(12,2)", "nullable": true }, { "name": "qty", "type": "int32", "nullable": true },
            { "name": "unit_price", "type": "double", "nullable": true }, { "name": "paid", "type": "boolean", "nullable": true },
            { "name": "order_date", "type": "date", "nullable": true }, { "name": "created_at", "type": "timestamp", "nullable": true },
            { "name": "tags", "type": "list", "nullable": true }, { "name": "ship_to", "type": "struct", "nullable": true }
        ])
    );
}

#[test]
fn every_compression_gives_the_same_rows() {
    let reference = read("orders-none.parquet", json!({}))["rows"].clone();
    for codec in ["snappy", "gzip", "zstd", "lz4", "brotli"] {
        assert_eq!(read(&format!("orders-{codec}.parquet"), json!({}))["rows"], reference, "{codec}");
    }
}

#[test]
fn decimals_as_numbers() {
    let out = read("orders-snappy.parquet", json!({ "decimals": "number" }));
    assert_eq!(out["rows"][0]["amount"], json!(1250.0));
    assert_eq!(out["rows"][1]["amount"], json!(99.95));
    assert_eq!(out["rows"][3]["amount"], Value::Null);
    let bad = call(json!({ "file": fixture("orders-snappy.parquet"), "_options": { "decimals": "float" } }));
    assert_eq!(bad["ok"], false);
    assert!(bad["error"].as_str().unwrap().contains("_options.decimals"));
}

#[test]
fn decimal_text() {
    use parquet::data_type::{ByteArray, Decimal};
    let t = |d: Decimal| decimal_string(&d).unwrap();
    assert_eq!(t(Decimal::from_i32(12345, 5, 3)), "12.345");
    assert_eq!(t(Decimal::from_i32(-5, 5, 2)), "-0.05");
    assert_eq!(t(Decimal::from_i32(0, 5, 2)), "0.00");
    assert_eq!(t(Decimal::from_i64(-7, 10, 0)), "-7");
    assert_eq!(t(Decimal::from_i64(i64::MAX, 19, 0)), "9223372036854775807");
    let big = 12345678901234567890123456789012345678i128;
    assert_eq!(t(Decimal::from_bytes(ByteArray::from(big.to_be_bytes().to_vec()), 38, 4)), "1234567890123456789012345678901234.5678");
    assert_eq!(t(Decimal::from_bytes(ByteArray::from(vec![0xff, 0xff]), 4, 1)), "-0.1");
    assert_eq!(decimal_string(&Decimal::from_bytes(ByteArray::from(vec![1u8; 17]), 38, 0)), None);
}

#[test]
fn select_columns() {
    let out = read("orders-zstd.parquet", json!({ "columns": ["ship_to", "order_id"] }));
    assert_eq!(out["rows"][0], json!({ "ship_to": { "city": "Springfield", "zip": "01101" }, "order_id": 1001 }));
    assert_eq!(out["columns"].as_array().unwrap().len(), 2);
    let bad = call(json!({ "file": fixture("orders-zstd.parquet"), "_options": { "columns": ["order_id", "total"] } }));
    assert_eq!(bad["ok"], false);
    assert_eq!(bad["error"], "column 'total' not in file (columns: order_id, customer, amount, qty, unit_price, paid, order_date, created_at, tags, ship_to)");
}

#[test]
fn offset_and_limit_across_row_groups() {
    let n = |out: &Value| out["rows"].as_array().unwrap().iter().map(|r| r["n"].as_i64().unwrap()).collect::<Vec<_>>();
    let all = read("rowgroups.parquet", json!({}));
    assert_eq!((all["row_groups"].clone(), all["row_count"].clone()), (json!(3), json!(25)));
    assert_eq!(n(&all), (0..25).collect::<Vec<_>>());
    // Starts in the second group and ends in the third.
    let mid = read("rowgroups.parquet", json!({ "offset": 15, "limit": 7 }));
    assert_eq!(n(&mid), (15..22).collect::<Vec<_>>());
    assert_eq!((mid["returned"].clone(), mid["truncated"].clone()), (json!(7), json!(true)));
    assert_eq!(mid["rows"][0]["label"], "row 15");
    // Exactly on a group boundary.
    assert_eq!(n(&read("rowgroups.parquet", json!({ "offset": 10, "limit": 1 }))), [10]);
    let tail = read("rowgroups.parquet", json!({ "offset": 20, "limit": 100 }));
    assert_eq!((n(&tail), tail["truncated"].clone()), ((20..25).collect::<Vec<_>>(), json!(false)));
    let past = read("rowgroups.parquet", json!({ "offset": 30 }));
    assert_eq!((past["returned"].clone(), past["truncated"].clone()), (json!(0), json!(false)));
    let none = read("rowgroups.parquet", json!({ "limit": 0 }));
    assert_eq!((none["returned"].clone(), none["truncated"].clone()), (json!(0), json!(true)));
    for bad in [json!({ "limit": -1 }), json!({ "offset": "5" }), json!({ "limit": 2.5 })] {
        let out = call(json!({ "file": fixture("rowgroups.parquet"), "_options": bad }));
        assert!(out["error"].as_str().unwrap().contains("whole number"), "{out}");
    }
}

#[test]
fn types_and_time_zones() {
    let out = read("types.parquet", json!({}));
    assert_eq!(
        out["rows"][0],
        json!({ "ts_ns_utc": "2026-01-31T14:05:09.123456Z", "ts_us_local": "2026-01-31T14:05:09",
                "ts_ms_utc": "1969-12-31T23:59:59.500Z", "t": "08:30:15.000000", "attrs": { "color": "red", "size": "L" },
                "blob": "AAFoaQ==", "matrix": [[1, 2], [3]], "big": 9007199254740993u64, "u8": 200 })
    );
    let types: Vec<&str> = out["columns"].as_array().unwrap().iter().map(|c| c["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["timestamp", "timestamp", "timestamp", "time", "map", "binary", "list", "int64", "uint8"]);
}

#[test]
fn spark_polars_and_duckdb_files() {
    // INT96 timestamps and two-level lists (Spark and Hive before 2020).
    assert_eq!(read("legacy-spark.parquet", json!({}))["rows"][0], json!({ "ts": "2026-01-31T14:05:09Z", "nums": [1, 2, 3] }));
    // Polars: zstd by default, timestamps without a time zone.
    let pl = read("polars-default.parquet", json!({}));
    assert_eq!(pl["rows"], json!([{ "sku": "A-1", "price": 9.99, "ts": "2026-05-01T08:30:00" }, { "sku": "B-2", "price": 15.0, "ts": "2026-05-02T09:45:00" }]));
    // DuckDB writes the older type annotations; types must still read as string, date and decimal.
    let dk = read("duckdb-default.parquet", json!({}));
    assert_eq!(dk["rows"][0], json!({ "id": 1, "name": "Café", "d": "2026-04-01", "price": "12.345", "note": null }));
    assert_eq!(dk["rows"][1]["price"], "1.500");
    let types: Vec<&str> = dk["columns"].as_array().unwrap().iter().map(|c| c["type"].as_str().unwrap()).collect();
    assert_eq!(types, ["int32", "string", "date", "decimal(10,3)", "string"]);
}

#[test]
fn empty_file_and_schema_off() {
    let out = read("empty.parquet", json!({}));
    assert_eq!((out["row_count"].clone(), out["rows"].clone(), out["truncated"].clone()), (json!(0), json!([]), json!(false)));
    assert_eq!(out["columns"].as_array().unwrap().len(), 10);
    let lean = read("orders-snappy.parquet", json!({ "schema": false }));
    assert!(lean.get("columns").is_none() && lean.get("row_groups").is_none() && lean.get("created_by").is_none());
    assert_eq!(lean["returned"], 4);
}

#[test]
fn finds_the_file_and_keeps_other_fields() {
    let b64 = fixture("duckdb-default.parquet");
    // Nested, found by its signature; the file itself is not passed on.
    let out = call(json!({ "batch": 7, "attachment": { "name": "x.parquet", "content": b64 } }));
    assert_eq!(out["ok"], true);
    assert_eq!(out["source_field"], "attachment.content");
    assert_eq!(out["batch"], 7);
    assert_eq!(out["attachment"], json!({ "name": "x.parquet" }));
    // Data URL and line-wrapped base64.
    let wrapped: String = b64.as_bytes().chunks(76).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("\n");
    let out = call(json!({ "f": format!("data:application/vnd.apache.parquet;base64,{wrapped}") }));
    assert_eq!(out["returned"], 2);
    // Named field; in a list the file is left where it is.
    let out = call(json!({ "files": [b64.clone()], "_options": { "field": "x.y" }, "x": { "y": b64.clone() } }));
    assert_eq!((out["source_field"].clone(), out["returned"].clone()), (json!("x.y"), json!(2)));
    let out = call(json!({ "files": [b64] }));
    assert_eq!((out["source_field"].clone(), out["files"].as_array().unwrap().len()), (json!("files[0]"), 1));
    // A list of records gives a list of results.
    let out = call(json!([{ "f": fixture("polars-default.parquet") }, { "f": "nothing" }]));
    assert_eq!((out[0]["ok"].clone(), out[1]["ok"].clone()), (json!(true), json!(false)));
}

#[test]
fn must_not_match() {
    let err = |v: Value| call(v)["error"].as_str().unwrap().to_owned();
    // CSV, PDF and plain text are not taken for Parquet.
    assert!(err(json!({ "f": STANDARD.encode("id,name\n1,Acme\n") })).starts_with("no base64 Parquet file found"));
    assert!(err(json!({ "f": "JVBERi0xLjQK" })).starts_with("no base64 Parquet file found"));
    assert!(err(json!({ "nothing": "here", "n": 1 })).starts_with("no base64 Parquet file found"));
    // A named field that holds something else.
    assert!(err(json!({ "f": STANDARD.encode("id,name\n1,Acme\n"), "_options": { "field": "f" } })).starts_with("not a Parquet file"));
    assert_eq!(err(json!({ "f": 1, "_options": { "field": "f" } })), "field 'f' is not text (expected the Parquet file as base64)");
    assert_eq!(err(json!({ "_options": { "field": "g" } })), "field 'g' not found in input");
    // A file cut off in transfer (footer missing) and a broken footer.
    let bytes = raw("orders-snappy.parquet");
    assert!(err(json!({ "f": STANDARD.encode(&bytes[..bytes.len() / 2]) })).starts_with("not a Parquet file"));
    let mut broken = bytes.clone();
    let n = broken.len();
    broken[n - 8..n - 4].copy_from_slice(&[0xff, 0xff, 0xff, 0x7f]);
    assert!(err(json!({ "f": STANDARD.encode(&broken) })).starts_with("cannot read Parquet file"));
    assert!(err(json!({ "f": "UEFSM!!!" })).starts_with("invalid base64 in f"));
    assert!(err(json!({ "f": fixture("orders-snappy.parquet"), "_options": { "columns": "order_id" } })).contains("list of column names"));
    assert_eq!(run(b"[1]")[0]["error"], "each input item must be a JSON object");
    assert!(run(b"not json")["error"].as_str().unwrap().starts_with("input is not JSON"));
}

#[test]
fn damaged_footer_gives_an_error_not_a_crash() {
    // Release builds abort on a panic, which stops the step, so a damaged footer (schema, row
    // group and page locations) must come back as ok: false. Damage inside the data pages
    // themselves can still stop the step: the parquet crate's row reader asserts on
    // inconsistent page data (see README limits).
    for codec in ["none", "snappy", "zstd"] {
        let bytes = raw(&format!("orders-{codec}.parquet"));
        let n = bytes.len();
        let footer_len = u32::from_le_bytes(bytes[n - 8..n - 4].try_into().unwrap()) as usize;
        for at in (n - 8 - footer_len..n - 8).step_by(3) {
            let mut b = bytes.clone();
            b[at] ^= 0x5a;
            let out = std::panic::catch_unwind(|| call(json!({ "f": STANDARD.encode(&b) })));
            match out {
                Ok(v) => assert!(v["ok"].is_boolean(), "{codec} @{at}: {v}"),
                Err(_) => panic!("{codec}: panic with footer byte damaged at {at}"),
            }
        }
    }
}
