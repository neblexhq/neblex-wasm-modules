use super::*;

/// Payloads built by Google's protobuf library from the official sparkplug_b.proto
/// (test/make_fixtures.py).
fn fixture(key: &str) -> (String, String) {
    let all: Value = serde_json::from_str(include_str!("../test/fixtures/payloads.json")).unwrap();
    let f = all.as_array().unwrap().iter().find(|f| f["key"] == key).unwrap();
    (f["topic"].as_str().unwrap().to_owned(), f["payload"].as_str().unwrap().to_owned())
}

fn go(v: Value) -> Value {
    run(serde_json::to_vec(&v).unwrap().as_slice())
}

fn decode(key: &str, opts: Value) -> Value {
    let (topic, payload) = fixture(key);
    go(json!({ "topic": topic, "payload": payload, "_options": opts }))
}

fn metric<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["metrics"].as_array().unwrap().iter().find(|m| m["name"] == name).unwrap_or_else(|| panic!("no metric {name} in {r}"))
}

#[test]
fn birth_scalars() {
    let r = decode("nbirth_scalars", json!({}));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["group_id"], json!("Plant1"));
    assert_eq!(r["message_type"], json!("NBIRTH"));
    assert_eq!(r["edge_node_id"], json!("Edge01"));
    assert_eq!(r["device_id"], Value::Null);
    assert_eq!(r["seq"], json!(0));
    assert_eq!(r["timestamp"], json!(1764000000123u64));
    assert_eq!(r["time"], json!("2025-11-24T16:00:00.123Z"));
    assert!(r.get("payload").is_none(), "payload is dropped by default");
    let v = |n: &str| metric(&r, n)["value"].clone();
    assert_eq!(v("bdSeq"), json!(3));
    assert_eq!(v("Line1/Temp"), json!(72.5));
    assert_eq!(v("Line1/Pressure"), json!(101.325));
    assert_eq!(v("Line1/Offset"), json!(-5));
    assert_eq!(v("Line1/Delta"), json!(-300));
    assert_eq!(v("Line1/Count"), json!(-70000));
    assert_eq!(v("Line1/Total"), json!(-5000000000i64));
    assert_eq!(v("Line1/Mode"), json!(200));
    assert_eq!(v("Line1/Port"), json!(65000));
    assert_eq!(v("Line1/Serial"), json!(4000000000u64));
    assert_eq!(v("Line1/Energy"), json!(18000000000000000000u64));
    assert_eq!(v("Line1/Running"), json!(true));
    assert_eq!(v("Line1/Recipe"), json!("PVC-40mm"));
    assert_eq!(v("Line1/LastStart"), json!(1763990000000i64));
    assert_eq!(metric(&r, "Line1/LastStart")["value_time"], json!("2025-11-24T13:13:20.000Z"));
    assert_eq!(v("Line1/Notes"), json!("Ölwechsel fällig"));
    assert_eq!(v("Line1/Batch"), json!("0f8e4b9a-3c1d-4e5f-9a7b-1c2d3e4f5a6b"));
    assert_eq!(v("Line1/Raw"), json!("AAH+/w=="));
    assert_eq!(v("Line1/Alarm"), Value::Null);
    assert_eq!(metric(&r, "Line1/Alarm")["is_null"], json!(true));
    let p = metric(&r, "Line1/Pressure");
    assert_eq!(p["datatype"], json!("Double"));
    assert_eq!(p["alias"], json!(3));
    assert_eq!(p["properties"], json!({ "engUnit": "kPa", "engHigh": 150.0, "documentation": null }));
    // The BIRTH gives the alias map to save for later DATA messages.
    assert_eq!(r["aliases"]["4"], json!({ "name": "Line1/Offset", "datatype": "Int8" }));
    assert_eq!(r["aliases"].as_object().unwrap().len(), 19);
}

#[test]
fn birth_arrays_dataset_template() {
    let r = decode("dbirth_complex", json!({}));
    assert_eq!(r["ok"], json!(true), "{r}");
    assert_eq!(r["device_id"], json!("Extruder2"));
    let v = |n: &str| metric(&r, n)["value"].clone();
    assert_eq!(v("Zones/Int8s"), json!([-1, 0, 127]));
    assert_eq!(v("Zones/Int16s"), json!([-32768, 32767]));
    assert_eq!(v("Zones/Temps"), json!([20.5, 21.25, -3.5]));
    assert_eq!(v("Zones/Doubles"), json!([1.5, -2.25]));
    assert_eq!(v("Zones/UInt32s"), json!([0, 4294967295u32]));
    assert_eq!(v("Zones/Int64s"), json!([-9007199254740993i64, 42]));
    // The spec's own example: [F,F,T,T,F,T,F,F,T,T,F,T] packs to 0x34 0xD0.
    assert_eq!(v("Zones/Valves"), json!([false, false, true, true, false, true, false, false, true, true, false, true]));
    assert_eq!(v("Zones/Names"), json!(["North", "South", "", "East"]));
    assert_eq!(v("Zones/Times"), json!([1763990000000i64, 1764000000000i64]));
    assert_eq!(
        v("Line1/Schedule"),
        json!({ "columns": ["shift", "target", "active"], "types": ["String", "Int32", "Boolean"],
                "rows": [{ "shift": "A", "target": 1200, "active": true }, { "shift": "B", "target": -1, "active": false }] })
    );
    let t = v("Line1/Motor");
    assert_eq!(t["template_ref"], json!("Motor"));
    assert_eq!(t["version"], json!("1.2"));
    assert_eq!(t["is_definition"], json!(false));
    assert_eq!(t["parameters"], json!({ "ratedKw": 7.5 }));
    assert_eq!(t["metrics"][0]["name"], json!("Speed"));
    assert_eq!(t["metrics"][0]["value"], json!(1450));
    assert_eq!(t["metrics"][1]["value"], json!(false));
}

#[test]
fn data_with_aliases() {
    // Without the alias map: values come back by alias, with a warning.
    let r = decode("ddata_aliases", json!({}));
    assert_eq!(r["ok"], json!(true));
    assert_eq!(r["metrics"][0]["name"], Value::Null);
    assert_eq!(r["metrics"][0]["alias"], json!(4));
    // Without a data type the raw unsigned number is all there is: must not guess the sign.
    assert_eq!(r["metrics"][0]["value"], json!(4294967289u64));
    assert!(r["warning"].as_str().unwrap().contains("aliases"));
    assert!(r.get("aliases").is_none());
    // With the map saved from the BIRTH: names and signed types are restored.
    let birth = decode("nbirth_scalars", json!({}));
    let r = decode("ddata_aliases", json!({ "aliases": birth["aliases"], "last_seq": 1 }));
    assert_eq!(r["metrics"][0]["name"], json!("Line1/Offset"));
    assert_eq!(r["metrics"][0]["datatype"], json!("Int8"));
    assert_eq!(r["metrics"][0]["value"], json!(-7));
    assert_eq!(r["metrics"][0]["time"], json!("2025-11-24T16:00:01.023Z"));
    assert_eq!(r["metrics"][1]["name"], json!("Line1/Temp"));
    assert_eq!(r["metrics"][1]["value"], json!(73.0));
    // Alias 99 is unknown even with the map.
    assert_eq!(r["metrics"][2]["name"], Value::Null);
    assert!(r["warning"].as_str().unwrap().starts_with("1 metric"));
    assert_eq!(r["seq_gap"], json!(false));
    // A plain name -> alias map works too.
    let r = decode("ddata_aliases", json!({ "aliases": { "2": "Temp" } }));
    assert_eq!(r["metrics"][1]["name"], json!("Temp"));
}

#[test]
fn seq_gaps_and_wrap() {
    assert_eq!(decode("ddata_aliases", json!({ "last_seq": 0 }))["seq_gap"], json!(true));
    assert_eq!(decode("ddata_aliases", json!({ "last_seq": 1 }))["seq_gap"], json!(false));
    assert_eq!(decode("nbirth_scalars", json!({ "last_seq": 200 }))["seq_gap"], json!(false));
    assert!(decode("ddata_aliases", json!({}))["seq_gap"].is_null());
    // 255 wraps to 0.
    let enc = go(json!({ "metrics": [], "seq": 0, "group_id": "G", "message_type": "NDATA", "edge_node_id": "E", "_options": { "op": "encode" } }));
    let r = go(json!({ "topic": enc["topic"], "payload": enc["payload"], "_options": { "last_seq": 255 } }));
    assert_eq!(r["seq_gap"], json!(false));
}

#[test]
fn rows_mode() {
    let (topic, payload) = fixture("ddata_aliases");
    let birth = decode("nbirth_scalars", json!({}));
    let r = go(json!([{ "topic": topic, "payload": payload, "site": "Austin", "_options": { "rows": true, "aliases": birth["aliases"] } }]));
    let rows = r.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["site"], json!("Austin"));
    assert_eq!(rows[0]["device_id"], json!("Extruder2"));
    assert_eq!(rows[0]["name"], json!("Line1/Offset"));
    assert_eq!(rows[0]["time"], json!("2025-11-24T16:00:01.023Z"));
    // A metric without its own timestamp takes the payload's.
    assert_eq!(rows[1]["time"], json!("2025-11-24T16:00:01.123Z"));
    assert!(rows[0].get("payload").is_none());
}

#[test]
fn python_array_style() {
    let r = decode("python_arrays", json!({ "array_style": "python" }));
    assert_eq!(metric(&r, "Valves")["value"], json!([true, false, false, false, false, false, false, false, true]));
    assert_eq!(metric(&r, "Names")["value"], json!(["Ab", "Zone 2"]));
    // Read with the spec layout, the same bytes give different booleans and hex text.
    let s = decode("python_arrays", json!({}));
    assert_eq!(metric(&s, "Valves")["value"][0], json!(false));
    assert_eq!(metric(&s, "Names")["value"], json!(["4162", "5a6f6e652032"]));
    assert_eq!(decode("python_arrays", json!({ "array_style": "x" }))["ok"], json!(false));
}

#[test]
fn death_flags_body_state() {
    let r = decode("ndeath", json!({}));
    assert_eq!(r["message_type"], json!("NDEATH"));
    assert_eq!(r["seq"], Value::Null);
    assert_eq!(metric(&r, "bdSeq")["value"], json!(3));
    let r = decode("flags_body", json!({}));
    let m = metric(&r, "Hist");
    assert_eq!(m["is_historical"], json!(true));
    assert_eq!(m["is_transient"], json!(true));
    assert_eq!(m["time"], json!("2025-11-24T15:59:00.123Z"));
    assert_eq!(r["uuid"], json!("custom-1"));
    assert_eq!(r["body"], json!("aGVsbG8="));
    // STATE (3.0 JSON and 2.x text).
    let r = go(json!({ "topic": "spBv1.0/STATE/scada-1", "payload": b64_encode(br#"{"online":true,"timestamp":1764000000000}"#) }));
    assert_eq!(r["message_type"], json!("STATE"));
    assert_eq!(r["host_id"], json!("scada-1"));
    assert_eq!(r["state"]["online"], json!(true));
    let r = go(json!({ "topic": "STATE/scada-1", "payload": "OFFLINE", "_options": { "encoding": "text" } }));
    assert_eq!(r["state"]["online"], json!(false));
}

#[test]
fn encode_round_trip() {
    let cmd = json!({
        "group_id": "Plant1", "message_type": "DCMD", "edge_node_id": "Edge01", "device_id": "Extruder2",
        "timestamp": 1764000000123u64, "seq": 7,
        "metrics": [
            { "name": "Line1/Setpoint", "datatype": "Float", "value": 75.5 },
            { "name": "Line1/Offset", "datatype": "Int8", "value": -5 },
            { "alias": 10, "datatype": "UInt32", "value": 4000000000u64 },
            { "name": "Line1/Recipe", "datatype": "String", "value": "PVC-50mm" },
            { "name": "Zones/Valves", "datatype": "BooleanArray", "value": [false, false, true, true, false, true, false, false, true, true, false, true] },
            { "name": "Zones/Names", "datatype": "StringArray", "value": ["North", "South"] },
            { "name": "Zones/Int16s", "datatype": "Int16Array", "value": [-1, 2] },
            { "name": "Reset", "datatype": "Boolean", "value": null }
        ],
        "_options": { "op": "encode" }
    });
    let e = go(cmd);
    assert_eq!(e["ok"], json!(true), "{e}");
    assert_eq!(e["topic"], json!("spBv1.0/Plant1/DCMD/Edge01/Extruder2"));
    // Spec example bytes for the boolean array: count 12, then 0x34 0xD0.
    let bytes = b64_decode(e["payload"].as_str().unwrap()).unwrap();
    assert!(bytes.windows(6).any(|w| w == [12, 0, 0, 0, 0x34, 0xD0]));
    let d = go(json!({ "topic": e["topic"], "payload": e["payload"] }));
    assert_eq!(d["seq"], json!(7));
    assert_eq!(d["time"], json!("2025-11-24T16:00:00.123Z"));
    let v = |i: usize| d["metrics"][i]["value"].clone();
    assert_eq!(v(0), json!(75.5));
    assert_eq!(v(1), json!(-5));
    assert_eq!(v(2), json!(4000000000u64));
    assert_eq!(v(3), json!("PVC-50mm"));
    assert_eq!(v(4), json!([false, false, true, true, false, true, false, false, true, true, false, true]));
    assert_eq!(v(5), json!(["North", "South"]));
    assert_eq!(v(6), json!([-1, 2]));
    assert_eq!(d["metrics"][7]["is_null"], json!(true));
}

#[test]
fn must_not_decode() {
    // Not Sparkplug: JSON text, random bytes, wrong topics, bad base64, wrong encode input.
    let r = go(json!({ "topic": "spBv1.0/G/NDATA/E", "payload": b64_encode(br#"{"temp": 20}"#) }));
    assert_eq!(r["ok"], json!(false), "{r}");
    assert!(r["error"].as_str().unwrap().contains("not"), "{r}");
    assert_eq!(go(json!({ "payload": b64_encode(&[0xff, 0xff, 0xff]) }))["ok"], json!(false));
    assert_eq!(go(json!({ "topic": "factory/line1/temp", "payload": "" }))["ok"], json!(false));
    assert_eq!(go(json!({ "topic": "spBv1.0/G/XDATA/E", "payload": "" }))["ok"], json!(false));
    assert_eq!(go(json!({ "topic": "spBv1.0/G/DDATA/E", "payload": "" }))["ok"], json!(false));
    assert_eq!(go(json!({ "topic": "spBv1.0/G/NDATA/E", "payload": "%%%" }))["ok"], json!(false));
    assert_eq!(go(json!({ "topic": "spBv1.0/G/NDATA/E" }))["ok"], json!(false));
    assert_eq!(go(json!({ "payload": 5 }))["ok"], json!(false));
    assert_eq!(go(json!({ "payload": "", "_options": { "op": "x" } }))["ok"], json!(false));
    assert_eq!(go(json!({ "payload": "", "_options": { "aliases": { "a": "x" } } }))["ok"], json!(false));
    let enc = |m: Value| go(json!({ "metrics": [m], "_options": { "op": "encode" } }));
    assert_eq!(enc(json!({ "name": "x", "datatype": "Int8", "value": 300 }))["ok"], json!(false));
    assert_eq!(enc(json!({ "name": "x", "datatype": "UInt8", "value": -1 }))["ok"], json!(false));
    assert_eq!(enc(json!({ "name": "x", "datatype": "Float", "value": "hot" }))["ok"], json!(false));
    assert_eq!(enc(json!({ "name": "x", "datatype": "Nope", "value": 1 }))["ok"], json!(false));
    assert_eq!(enc(json!({ "name": "x", "datatype": "DataSet", "value": {} }))["ok"], json!(false));
    assert_eq!(enc(json!({ "datatype": "Int8", "value": 1 }))["ok"], json!(false));
    assert_eq!(go(json!({ "_options": { "op": "encode" } }))["ok"], json!(false));
    assert_eq!(go(json!("text"))["ok"], json!(false));
    assert_eq!(run(b"nope")["ok"], json!(false));
    // An empty payload is a valid (empty) message.
    let r = go(json!({ "topic": "spBv1.0/G/NDATA/E", "payload": "" }));
    assert_eq!(r["ok"], json!(true));
    assert_eq!(r["metrics"], json!([]));
}

#[test]
fn helpers() {
    assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
    assert_eq!(iso(-1), "1969-12-31T23:59:59.999Z");
    assert_eq!(b64_decode(&b64_encode(b"Sparkplug")).unwrap(), b"Sparkplug");
    assert_eq!(b64_decode("aGVsbG8").unwrap(), b"hello");
    assert_eq!(b64_decode("-_8").unwrap(), vec![0xfb, 0xff]);
    assert_eq!(type_code("double"), Some(10));
    assert_eq!(type_name(99), "Type99");
    let r = go(json!({ "topic": "spBv1.0/G/NDATA/E", "payload": "08 01", "_options": { "encoding": "hex" } }));
    assert_eq!(r["timestamp"], json!(1));
}
