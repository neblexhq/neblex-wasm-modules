//! Neblex sandboxed module: evaluate a decision table (pricing, approval or routing rules) on each record.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the options.

use serde_json::{json, Map, Value};
use zen_expression::{evaluate_expression, evaluate_unary_expression, Variable};

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

/// The sandbox has no random source; the expression engine only needs random bytes to seed hash
/// tables, so a fixed-seed generator is enough (results never depend on it).
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
    match serde_json::from_slice::<Value>(input) {
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(decide).collect()),
        Ok(v) => decide(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

struct Column {
    /// Keys a rule may use for this column: id, field and name.
    keys: Vec<String>,
    field: String,
    label: String,
}

struct Table {
    collect: bool,
    inputs: Vec<Column>,
    outputs: Vec<Column>,
    rules: Vec<Map<String, Value>>,
}

/// Evaluates `_options.table` against the record and adds the result under `_options.into`
/// (default `decision`), plus `matched` and `matched_rule` (or `matched_rules` for collect).
fn decide(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let o = m.remove("_options").unwrap_or(Value::Null);
    let fail = |mut m: Map<String, Value>, msg: String| {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(msg));
        Value::Object(m)
    };
    let table = match parse_table(o.get("table"), o.get("node").and_then(Value::as_str)) {
        Ok(t) => t,
        Err(e) => return fail(m, e),
    };
    let into = o.get("into").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("decision").to_owned();
    zen_expression::set_utc_now_millis(match o.get("now") {
        None | Some(Value::Null) => 0,
        Some(v) => match now_millis(v) {
            Some(ms) => ms,
            None => return fail(m, format!("_options.now must be an ISO 8601 date or time, or Unix milliseconds; got {v}")),
        },
    });

    let context = Variable::from(Value::Object(m.clone()));
    // Each input column's value, evaluated once per record.
    let mut values = Vec::with_capacity(table.inputs.len());
    for c in &table.inputs {
        if c.field.trim().is_empty() {
            values.push(None);
            continue;
        }
        match evaluate_expression(&c.field, context.clone()) {
            Ok(v) => values.push(Some(v)),
            Err(e) => return fail(m, format!("input '{}': field expression '{}' failed: {e}", c.label, c.field)),
        }
    }

    let mut results = Vec::new();
    let mut matched = Vec::new();
    'rules: for (i, rule) in table.rules.iter().enumerate() {
        for (c, value) in table.inputs.iter().zip(&values) {
            let Some(cell) = cell(rule, &c.keys) else { continue };
            let hit = match value {
                Some(v) => {
                    let mut ctx = m.clone();
                    ctx.insert("$".into(), Value::from(v.clone()));
                    evaluate_unary_expression(&cell, Variable::from(Value::Object(ctx)))
                }
                // A column without a field holds whole conditions, such as `total > 100 and tier == "gold"`.
                None => evaluate_expression(&cell, context.clone()).map(|v| truthy(&Value::from(v))),
            };
            match hit {
                Ok(true) => {}
                Ok(false) => continue 'rules,
                // A condition that can't be applied to this value (a range check on a missing
                // field) doesn't match, as in the ZEN engine; a condition that doesn't parse is an error.
                Err(zen_expression::IsolateError::VMError { .. }) => continue 'rules,
                Err(e) => return fail(m, format!("rule {} ({}), input '{}': condition '{cell}' failed: {e}", i + 1, rule_label(rule, i), c.label)),
            }
        }
        let mut out = Value::Object(Map::new());
        for c in &table.outputs {
            let v = match rule_value(rule, &c.keys) {
                None => continue,
                Some(Value::String(expr)) if expr.trim().is_empty() => continue,
                Some(Value::String(expr)) => match evaluate_expression(&expr, context.clone()) {
                    Ok(v) => Value::from(v),
                    Err(e) => return fail(m, format!("rule {} ({}), output '{}': expression '{expr}' failed: {e}", i + 1, rule_label(rule, i), c.label)),
                },
                // Numbers, booleans and objects written as JSON are used as they are.
                Some(other) => other.clone(),
            };
            set_path(&mut out, &c.field, v);
        }
        matched.push(rule_info(rule, i));
        results.push(out);
        if !table.collect {
            break;
        }
    }

    m.insert("matched".into(), json!(!results.is_empty()));
    if table.collect {
        m.insert(into, Value::Array(results));
        m.insert("matched_rules".into(), Value::Array(matched));
    } else {
        m.insert(into, results.pop().unwrap_or(Value::Null));
        m.insert("matched_rule".into(), matched.pop().unwrap_or(Value::Null));
    }
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

/// Accepts the table as an object or JSON text, either in this module's shape or as a JDM
/// decision graph (the first decision table node, or the one named by `_options.node`).
fn parse_table(v: Option<&Value>, node: Option<&str>) -> Result<Table, String> {
    let v = match v {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).map_err(|e| format!("_options.table is not valid JSON: {e}"))?,
        Some(v @ Value::Object(_)) => v.clone(),
        _ => return Err("_options.table is missing; pass a decision table with inputs, outputs and rules".into()),
    };
    let v = if let Some(nodes) = v.get("nodes").and_then(Value::as_array) {
        let found = nodes.iter().find(|n| {
            n.get("type").and_then(Value::as_str) == Some("decisionTableNode")
                && node.is_none_or(|want| n.get("name").and_then(Value::as_str) == Some(want) || n.get("id").and_then(Value::as_str) == Some(want))
        });
        match found.and_then(|n| n.get("content")) {
            Some(c) => c.clone(),
            None => return Err(match node {
                Some(n) => format!("the decision graph has no decision table node named '{n}'"),
                None => "the decision graph has no decision table node".into(),
            }),
        }
    } else {
        v
    };
    let policy = v.get("hit_policy").or_else(|| v.get("hitPolicy")).and_then(Value::as_str).unwrap_or("first").to_ascii_lowercase();
    let collect = match policy.as_str() {
        "first" => false,
        "collect" => true,
        other => return Err(format!("unknown hit policy '{other}'; use first or collect")),
    };
    let columns = |key: &str, need_field: bool| -> Result<Vec<Column>, String> {
        let list = v.get(key).and_then(Value::as_array).ok_or_else(|| format!("the table needs an '{key}' list"))?;
        list.iter()
            .enumerate()
            .map(|(i, c)| {
                let c = match c {
                    Value::String(s) => json!({ "field": s }),
                    other => other.clone(),
                };
                let get = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
                let (id, field, name) = (get("id"), get("field"), get("name"));
                if need_field && field.is_empty() {
                    return Err(format!("{key} column {} needs a 'field'", i + 1));
                }
                let keys = [&id, &field, &name].into_iter().filter(|k| !k.is_empty()).cloned().collect();
                let label = [&name, &field, &id].into_iter().find(|k| !k.is_empty()).cloned().unwrap_or_else(|| format!("{}", i + 1));
                Ok(Column { keys, field, label })
            })
            .collect()
    };
    let inputs = columns("inputs", false)?;
    let outputs = columns("outputs", true)?;
    let rules = v
        .get("rules")
        .and_then(Value::as_array)
        .ok_or("the table needs a 'rules' list")?
        .iter()
        .enumerate()
        .map(|(i, r)| r.as_object().cloned().ok_or(format!("rule {} must be an object keyed by column", i + 1)))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Table { collect, inputs, outputs, rules })
}

fn rule_value<'a>(rule: &'a Map<String, Value>, keys: &[String]) -> Option<&'a Value> {
    keys.iter().find_map(|k| rule.get(k))
}

/// An input cell's condition; empty, `-` and missing cells match anything.
fn cell(rule: &Map<String, Value>, keys: &[String]) -> Option<String> {
    let s = match rule_value(rule, keys)? {
        Value::Null => return None,
        Value::String(s) => s.trim().to_owned(),
        // A bare JSON value means "equals this value".
        other => other.to_string(),
    };
    (!s.is_empty() && s != "-").then_some(s)
}

fn rule_label(rule: &Map<String, Value>, i: usize) -> String {
    rule.get("_description").or_else(|| rule.get("_id")).and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("#{}", i + 1))
}

fn rule_info(rule: &Map<String, Value>, i: usize) -> Value {
    let mut info = json!({ "index": i + 1 });
    for (k, out) in [("_id", "id"), ("_description", "description")] {
        if let Some(v) = rule.get(k).filter(|v| !v.is_null()) {
            info[out] = v.clone();
        }
    }
    info
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Null => false,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// Writes a value at a dotted path, creating objects on the way (`discount.rate`).
fn set_path(target: &mut Value, path: &str, value: Value) {
    let mut cur = target;
    let parts: Vec<&str> = path.split('.').collect();
    for (i, p) in parts.iter().enumerate() {
        if !cur.is_object() {
            *cur = Value::Object(Map::new());
        }
        let obj = cur.as_object_mut().expect("object");
        if i == parts.len() - 1 {
            obj.insert((*p).to_owned(), value);
            return;
        }
        cur = obj.entry((*p).to_owned()).or_insert_with(|| Value::Object(Map::new()));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // The clock behind `now` is global (one module instance runs one call at a time), so tests,
    // which run in parallel threads, take turns.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn go(v: Value) -> Value {
        let _turn = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    fn discounts() -> Value {
        json!({
            "hit_policy": "first",
            "inputs": [{ "field": "customer.tier", "name": "Tier" }, { "field": "order.total", "name": "Order total" }, { "field": "order.country" }],
            "outputs": [{ "field": "discount" }, { "field": "approval" }],
            "rules": [
                { "_description": "Gold, large order", "Tier": "\"gold\"", "Order total": ">= 10000", "discount": "0.15", "approval": "\"manager\"" },
                { "_description": "Gold", "Tier": "\"gold\"", "discount": 0.10, "approval": "\"none\"" },
                { "_description": "EU mid-size", "order.total": "[1000..9999]", "order.country": "\"DE\", \"FR\", \"NL\"", "discount": "0.05", "approval": "\"none\"" },
                { "_description": "Everyone else", "discount": 0, "approval": "\"none\"" }
            ]
        })
    }

    #[test]
    fn first_hit_wins() {
        let out = go(json!({ "customer": { "tier": "gold" }, "order": { "total": 12500, "country": "US" }, "_options": { "table": discounts() } }));
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["matched"], json!(true));
        assert_eq!(out["decision"], json!({ "discount": 0.15, "approval": "manager" }));
        assert_eq!(out["matched_rule"], json!({ "index": 1, "description": "Gold, large order" }));
        assert!(out.get("_options").is_none());
        let gold = go(json!({ "customer": { "tier": "gold" }, "order": { "total": 500 }, "_options": { "table": discounts() } }));
        assert_eq!(gold["decision"], json!({ "discount": 0.10, "approval": "none" }));
    }

    #[test]
    fn ranges_and_lists() {
        let de = go(json!({ "customer": { "tier": "silver" }, "order": { "total": 2500, "country": "DE" }, "_options": { "table": discounts() } }));
        assert_eq!(de["matched_rule"]["index"], json!(3));
        assert_eq!(de["decision"]["discount"], json!(0.05));
        // Must not match: right country, total outside the range.
        let big = go(json!({ "customer": { "tier": "silver" }, "order": { "total": 25000, "country": "DE" }, "_options": { "table": discounts() } }));
        assert_eq!(big["matched_rule"]["index"], json!(4));
        assert_eq!(big["decision"]["discount"], json!(0));
    }

    #[test]
    fn no_match_and_collect() {
        let table = json!({
            "hit_policy": "collect",
            "inputs": ["amount", "category"],
            "outputs": ["flag"],
            "rules": [
                { "amount": "> 5000", "flag": "\"large amount\"" },
                { "category": "\"travel\", \"gifts\"", "flag": "\"needs receipt\"" },
                { "amount": "< 0", "flag": "\"negative\"" }
            ]
        });
        let out = go(json!({ "amount": 7200, "category": "gifts", "_options": { "table": table.clone(), "into": "flags" } }));
        assert_eq!(out["flags"], json!([{ "flag": "large amount" }, { "flag": "needs receipt" }]));
        assert_eq!(out["matched_rules"], json!([{ "index": 1 }, { "index": 2 }]));
        let none = go(json!({ "amount": 10, "category": "software", "_options": { "table": table } }));
        assert_eq!(none["matched"], json!(false));
        assert_eq!(none["decision"], json!([]));
        let first = go(json!({ "amount": 10, "_options": { "table": { "inputs": ["amount"], "outputs": ["x"], "rules": [{ "amount": "> 100", "x": 1 }] } } }));
        assert_eq!(first["matched"], json!(false));
        assert_eq!(first["decision"], Value::Null);
    }

    #[test]
    fn computed_outputs_and_nested_fields() {
        let table = json!({
            "inputs": [{ "field": "qty" }],
            "outputs": [{ "field": "price.unit" }, { "field": "price.total" }, { "field": "label" }],
            "rules": [
                { "qty": ">= 100", "price.unit": "list_price * 0.8", "price.total": "list_price * 0.8 * qty", "label": "\"Bulk \" + string(qty)" },
                { "qty": "-", "price.unit": "list_price", "price.total": "list_price * qty", "label": "\"Standard\"" }
            ]
        });
        let out = go(json!({ "qty": 150, "list_price": 4.5, "_options": { "table": table } }));
        assert_eq!(out["decision"], json!({ "price": { "unit": 3.6, "total": 540 }, "label": "Bulk 150" }));
    }

    #[test]
    fn whole_condition_columns_and_functions() {
        let table = json!({
            "inputs": [{ "name": "Rule" }, { "field": "lower(email)" }],
            "outputs": ["route"],
            "rules": [
                { "Rule": "total > 1000 and tier == \"gold\"", "route": "\"priority\"" },
                { "lower(email)": "endsWith($, \"@example.com\")", "route": "\"internal\"" },
                { "route": "\"standard\"" }
            ]
        });
        let p = go(json!({ "total": 1500, "tier": "gold", "email": "a@x.com", "_options": { "table": table.clone() } }));
        assert_eq!(p["decision"]["route"], json!("priority"));
        let i = go(json!({ "total": 10, "tier": "gold", "email": "Ops@Example.com", "_options": { "table": table.clone() } }));
        assert_eq!(i["decision"]["route"], json!("internal"));
        let s = go(json!({ "total": 10, "tier": "silver", "email": "a@x.com", "_options": { "table": table } }));
        assert_eq!(s["decision"]["route"], json!("standard"));
    }

    #[test]
    fn jdm_graph_with_ids() {
        let graph = json!({
            "nodes": [
                { "id": "in", "type": "inputNode", "name": "Request" },
                { "id": "t1", "type": "decisionTableNode", "name": "Shipping", "content": {
                    "hitPolicy": "first",
                    "inputs": [{ "id": "c1", "name": "Weight", "field": "weight_kg" }, { "id": "c2", "name": "Zone", "field": "zone" }],
                    "outputs": [{ "id": "o1", "name": "Carrier", "field": "carrier" }, { "id": "o2", "name": "Cost", "field": "cost" }],
                    "rules": [
                        { "_id": "r1", "c1": "> 30", "c2": "", "o1": "\"freight\"", "o2": "120" },
                        { "_id": "r2", "c1": "<= 30", "c2": "\"domestic\"", "o1": "\"ground\"", "o2": "8 + weight_kg * 0.5" },
                        { "_id": "r3", "c1": "", "c2": "", "o1": "\"express\"", "o2": "25" }
                    ]
                }}
            ],
            "edges": []
        });
        let out = go(json!({ "weight_kg": 12, "zone": "domestic", "_options": { "table": graph.clone() } }));
        assert_eq!(out["decision"], json!({ "carrier": "ground", "cost": 14 }));
        assert_eq!(out["matched_rule"], json!({ "index": 2, "id": "r2" }));
        let text = go(json!({ "weight_kg": 40, "zone": "intl", "_options": { "table": graph.to_string(), "node": "Shipping" } }));
        assert_eq!(text["decision"]["carrier"], json!("freight"));
        let missing = go(json!({ "_options": { "table": graph, "node": "Nope" } }));
        assert_eq!(missing["ok"], json!(false));
    }

    #[test]
    fn readme_cell_syntax() {
        let table = json!({
            "inputs": [{ "field": "ref", "name": "Ref" }, { "field": "score", "name": "Score" }],
            "outputs": ["hit"],
            "rules": [
                { "Ref": "startsWith($, \"PO-\")", "Score": "(0..1)", "hit": "\"po-fraction\"" },
                { "Ref": "len($) > 10", "hit": "\"long\"" },
                { "Score": "[1..1]", "hit": "\"one\"" },
                { "hit": "\"none\"" }
            ]
        });
        let hit = |r: Value| go(json!({ "ref": r["ref"], "score": r["score"], "_options": { "table": table.clone() } }))["decision"]["hit"].clone();
        assert_eq!(hit(json!({ "ref": "PO-1", "score": 0.5 })), json!("po-fraction"));
        // Must not match: an exclusive range leaves out its ends.
        assert_eq!(hit(json!({ "ref": "PO-1", "score": 1 })), json!("one"));
        assert_eq!(hit(json!({ "ref": "PO-1", "score": 0 })), json!("none"));
        assert_eq!(hit(json!({ "ref": "INV-2026-0091", "score": 5 })), json!("long"));
    }

    #[test]
    fn dates_use_the_now_option() {
        let table = json!({
            "inputs": [{ "name": "Overdue", "field": "" }],
            "outputs": ["status"],
            "rules": [
                { "Overdue": "date(due) < date(\"now\")", "status": "\"overdue\"" },
                { "status": "\"open\"" }
            ]
        });
        let late = go(json!({ "due": "2026-11-01", "_options": { "table": table.clone(), "now": "2026-11-24" } }));
        assert_eq!(late["decision"]["status"], json!("overdue"));
        let open = go(json!({ "due": "2026-12-01", "_options": { "table": table.clone(), "now": "2026-11-24T10:00:00Z" } }));
        assert_eq!(open["decision"]["status"], json!("open"));
        let bad = go(json!({ "due": "2026-12-01", "_options": { "table": table, "now": "yesterday" } }));
        assert_eq!(bad["ok"], json!(false));
    }

    #[test]
    fn missing_fields_and_errors() {
        // A missing input field is null and doesn't match a comparison.
        let out = go(json!({ "_options": { "table": discounts() } }));
        assert_eq!(out["matched_rule"]["index"], json!(4), "{out}");
        let bad = go(json!({ "x": 1, "_options": { "table": { "inputs": ["x"], "outputs": ["y"], "rules": [{ "x": "> > 1", "y": 1 }] } } }));
        assert_eq!(bad["ok"], json!(false));
        assert!(bad["error"].as_str().unwrap().contains("rule 1"));
        for t in [json!(null), json!({ "inputs": [], "outputs": [] }), json!({ "hit_policy": "unique", "inputs": [], "outputs": [], "rules": [] }), json!("{not json")] {
            let r = go(json!({ "_options": { "table": t } }));
            assert_eq!(r["ok"], json!(false), "{r}");
        }
        assert_eq!(go(json!(["x"]))[0]["ok"], json!(false));
    }
}
