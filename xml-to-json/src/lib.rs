//! Neblex sandboxed module: convert XML documents to JSON, optionally one record per repeating element.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the options.

use roxmltree::{Document, Node, ParsingOptions};
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

struct Options {
    field: String,
    records: Option<String>,
    include: Vec<(String, String)>,
    always_array: Vec<String>,
    keep_namespaces: bool,
    numbers: bool,
    keep_source: bool,
}

fn options(v: Option<Value>) -> Options {
    let v = v.unwrap_or(Value::Null);
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
    let b = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    Options {
        field: s("field").unwrap_or_else(|| "xml".into()),
        records: s("records"),
        include: v
            .get("include")
            .and_then(Value::as_object)
            .map(|m| m.iter().filter_map(|(k, p)| Some((k.clone(), p.as_str()?.to_owned()))).collect())
            .unwrap_or_default(),
        always_array: v
            .get("always_array")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            .unwrap_or_default(),
        keep_namespaces: b("keep_namespaces"),
        numbers: b("numbers"),
        keep_source: b("keep_source"),
    }
}

/// Reads the XML from `_options.field` (default `xml`) and adds `data` (the whole document)
/// or `records` (one entry per element matched by `_options.records`). `_options` is removed.
fn convert_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = options(m.remove("_options"));
    let Some(text) = m.get(&opt.field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{}' is missing or not text", opt.field)));
        return Value::Object(m);
    };
    // DTDs are refused: no entity expansion, so no billion-laughs input.
    let parsed = Document::parse_with_options(text.trim_start_matches('\u{feff}'), ParsingOptions { allow_dtd: false, ..Default::default() });
    let doc = match parsed {
        Ok(d) => d,
        Err(e) => {
            m.insert("ok".into(), json!(false));
            m.insert("error".into(), json!(format!("not well-formed XML: {e}")));
            return Value::Object(m);
        }
    };
    let root = doc.root_element();
    if !opt.keep_source {
        m.remove(&opt.field);
    }
    match &opt.records {
        None => {
            let mut data = Map::new();
            data.insert(name(root, &opt), element(root, &opt));
            m.insert("data".into(), Value::Object(data));
        }
        Some(path) => {
            let header: Vec<(String, Value)> = opt
                .include
                .iter()
                .map(|(k, p)| (k.clone(), find(root, p, &opt).first().map(|n| element(*n, &opt)).unwrap_or(Value::Null)))
                .collect();
            let records: Vec<Value> = find(root, path, &opt)
                .into_iter()
                .map(|n| {
                    let mut r = match element(n, &opt) {
                        Value::Object(o) => o,
                        other => {
                            let mut o = Map::new();
                            o.insert(name(n, &opt), other);
                            o
                        }
                    };
                    for (k, v) in &header {
                        r.insert(k.clone(), v.clone());
                    }
                    Value::Object(r)
                })
                .collect();
            m.insert("record_count".into(), json!(records.len()));
            m.insert("records".into(), Value::Array(records));
        }
    }
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

fn name(n: Node, opt: &Options) -> String {
    let local = n.tag_name().name();
    match (opt.keep_namespaces, n.tag_name().namespace().and_then(|ns| n.lookup_prefix(ns))) {
        (true, Some(p)) if !p.is_empty() => format!("{p}:{local}"),
        _ => local.to_owned(),
    }
}

/// An element with no attributes and no child elements becomes its text (null when empty).
/// Otherwise an object: attributes as "@name", child elements by name (arrays when repeated),
/// and any text as "#text".
fn element(n: Node, opt: &Options) -> Value {
    let kids: Vec<Node> = n.children().filter(|c| c.is_element()).collect();
    let text: String = n.children().filter(|c| c.is_text()).filter_map(|c| c.text()).collect();
    let text = text.trim();
    let attrs: Vec<_> = n.attributes().filter(|a| a.namespace() != Some("http://www.w3.org/2000/xmlns/")).collect();
    if kids.is_empty() && attrs.is_empty() {
        return scalar(text, opt);
    }
    let mut obj = Map::new();
    for a in attrs {
        obj.insert(format!("@{}", a.name()), scalar(a.value(), opt));
    }
    for k in kids {
        let key = name(k, opt);
        let v = element(k, opt);
        match obj.get_mut(&key) {
            Some(Value::Array(arr)) => arr.push(v),
            Some(existing) => {
                let first = existing.take();
                *existing = Value::Array(vec![first, v]);
            }
            None => {
                let forced = opt.always_array.iter().any(|a| a == &key || a == k.tag_name().name());
                obj.insert(key, if forced { Value::Array(vec![v]) } else { v });
            }
        }
    }
    if !text.is_empty() {
        obj.insert("#text".into(), scalar(text, opt));
    }
    Value::Object(obj)
}

fn scalar(s: &str, opt: &Options) -> Value {
    if s.is_empty() {
        return Value::Null;
    }
    if opt.numbers {
        let plain = !s.starts_with('+') && !(s.len() > 1 && s.starts_with('0') && !s.starts_with("0."));
        if plain {
            if let Ok(i) = s.parse::<i64>() {
                return json!(i);
            }
            if s.contains('.') && !s.ends_with('.') {
                if let Ok(f) = s.parse::<f64>() {
                    if f.is_finite() {
                        return json!(f);
                    }
                }
            }
        }
    }
    json!(s)
}

/// Path of local element names separated by "/", relative to the root element (the root's own
/// name may lead). "//Name" matches every element with that name at any depth.
fn find<'a, 'i>(root: Node<'a, 'i>, path: &str, opt: &Options) -> Vec<Node<'a, 'i>> {
    if let Some(want) = path.strip_prefix("//") {
        return root.descendants().filter(|d| d.is_element() && (d.tag_name().name() == want || name(*d, opt) == want)).collect();
    }
    let mut segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.first().is_some_and(|s| *s == root.tag_name().name()) {
        segs.remove(0);
    }
    let mut cur = vec![root];
    for seg in segs {
        cur = cur
            .into_iter()
            .flat_map(|n| n.children().filter(move |c| c.is_element() && (c.tag_name().name() == seg)))
            .collect();
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOD: &str = r#"<?xml version="1.0"?>
<SyncSalesOrder xmlns="http://schema.infor.com/InforOAGIS/2" releaseID="9.2">
  <DataArea>
    <SalesOrder>
      <SalesOrderHeader>
        <DocumentID><ID accountingEntity="100">SO-1001</ID></DocumentID>
        <CustomerParty><Name>Acme &amp; Sons</Name></CustomerParty>
      </SalesOrderHeader>
      <SalesOrderLine><LineNumber>1</LineNumber><Item><ItemID><ID>WIDGET-9</ID></ItemID></Item><Quantity unitCode="EA">5</Quantity></SalesOrderLine>
      <SalesOrderLine><LineNumber>2</LineNumber><Item><ItemID><ID>BOLT-2</ID></ItemID></Item><Quantity unitCode="BX">2.5</Quantity><Note/></SalesOrderLine>
    </SalesOrder>
  </DataArea>
</SyncSalesOrder>"#;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    #[test]
    fn whole_document() {
        let out = go(json!({ "xml": BOD }));
        assert_eq!(out["ok"], json!(true));
        let so = &out["data"]["SyncSalesOrder"];
        assert_eq!(so["@releaseID"], json!("9.2"));
        let h = &so["DataArea"]["SalesOrder"]["SalesOrderHeader"];
        assert_eq!(h["DocumentID"]["ID"], json!({ "@accountingEntity": "100", "#text": "SO-1001" }));
        assert_eq!(h["CustomerParty"]["Name"], json!("Acme & Sons"));
        assert_eq!(so["DataArea"]["SalesOrder"]["SalesOrderLine"].as_array().unwrap().len(), 2);
        assert!(out.get("xml").is_none());
    }

    #[test]
    fn records_with_header_fields_and_numbers() {
        let out = go(json!({ "xml": BOD, "_options": {
            "records": "DataArea/SalesOrder/SalesOrderLine",
            "include": { "order_id": "DataArea/SalesOrder/SalesOrderHeader/DocumentID/ID", "customer": "//Name" },
            "numbers": true
        }}));
        assert_eq!(out["record_count"], json!(2));
        let r = &out["records"][1];
        assert_eq!(r["LineNumber"], json!(2));
        assert_eq!(r["Quantity"], json!({ "@unitCode": "BX", "#text": 2.5 }));
        assert_eq!(r["Item"]["ItemID"]["ID"], json!("BOLT-2"));
        assert_eq!(r["Note"], Value::Null);
        assert_eq!(r["order_id"]["#text"], json!("SO-1001"));
        assert_eq!(r["customer"], json!("Acme & Sons"));
    }

    #[test]
    fn always_array_and_leading_zeros() {
        let out = go(json!({ "doc": "<r><line><sku>007</sku></line></r>", "_options": { "field": "doc", "always_array": ["line"], "numbers": true } }));
        assert_eq!(out["data"]["r"]["line"], json!([{ "sku": "007" }]));
    }

    #[test]
    fn errors() {
        assert_eq!(go(json!({ "xml": "<a><b></a>" }))["ok"], json!(false));
        let dtd = go(json!({ "xml": "<!DOCTYPE a [<!ENTITY x \"y\">]><a>&x;</a>" }));
        assert_eq!(dtd["ok"], json!(false));
        assert_eq!(go(json!({ "other": 1 }))["ok"], json!(false));
    }
}
