//! Neblex sandboxed module: turn almost-JSON (typically from an AI model) into valid JSON.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for what gets repaired.

use serde_json::{json, Map, Number, Value};

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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(repair_record).collect()),
        Ok(v) => repair_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// Reads the text in `_options.field` (default `text`), and adds `json` (the parsed value),
/// `repaired` and `fixes`. Set `_options.into` to write the value under another name.
fn repair_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("text").to_owned();
    let into = opt.get("into").and_then(Value::as_str).unwrap_or("json").to_owned();
    let Some(text) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    match repair(&text) {
        Ok((v, fixes)) => {
            m.insert(into, v);
            m.insert("repaired".into(), json!(!fixes.is_empty()));
            m.insert("fixes".into(), json!(fixes));
            m.insert("ok".into(), json!(true));
        }
        Err(e) => {
            m.insert("ok".into(), json!(false));
            m.insert("error".into(), json!(e));
        }
    }
    Value::Object(m)
}

pub fn repair(text: &str) -> Result<(Value, Vec<String>), String> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        return Ok((v, vec![]));
    }
    let mut fixes = Vec::new();
    let mut body = text.trim();
    if let Some(inner) = fenced(body) {
        fixes.push("removed code fence".to_owned());
        body = inner;
    }
    let start = body.find(['{', '[']).ok_or("no JSON object or array found")?;
    if start > 0 {
        fixes.push("removed text before the JSON".to_owned());
    }
    let mut p = Parser { s: body[start..].chars().collect(), i: 0, fixes: Vec::new() };
    let v = p.value()?;
    p.ws();
    if p.i < p.s.len() {
        p.fixes.push("removed text after the JSON".into());
    }
    fixes.extend(p.fixes);
    fixes.dedup();
    Ok((v, fixes))
}

/// The content of the first ``` fence, if the text has one.
fn fenced(s: &str) -> Option<&str> {
    let open = s.find("```")?;
    let after = &s[open + 3..];
    let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
    let body = &after[body_start..];
    Some(match body.find("```") {
        Some(close) => &body[..close],
        None => body,
    })
}

struct Parser {
    s: Vec<char>,
    i: usize,
    fixes: Vec<String>,
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn fix(&mut self, what: &str) {
        if !self.fixes.iter().any(|f| f == what) {
            self.fixes.push(what.to_owned());
        }
    }

    /// Skips whitespace and // or /* */ comments.
    fn ws(&mut self) {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.i += 1;
            }
            match (self.peek(), self.s.get(self.i + 1)) {
                (Some('/'), Some('/')) => {
                    self.fix("removed comments");
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                (Some('/'), Some('*')) => {
                    self.fix("removed comments");
                    self.i += 2;
                    while self.i < self.s.len() && !(self.s[self.i] == '*' && self.s.get(self.i + 1) == Some(&'/')) {
                        self.i += 1;
                    }
                    self.i = (self.i + 2).min(self.s.len());
                }
                _ => return,
            }
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        self.ws();
        match self.peek() {
            None => {
                self.fix("filled a missing value with null (text was cut off)");
                Ok(Value::Null)
            }
            Some('{') => self.object(),
            Some('[') => self.array(),
            Some(q @ ('"' | '\'')) => {
                if q == '\'' {
                    self.fix("replaced single quotes");
                }
                Ok(Value::String(self.string(q)))
            }
            Some(c) if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() => Ok(self.number()),
            Some(_) => {
                let word = self.word();
                match word.as_str() {
                    "true" | "True" | "TRUE" => Ok(json!(true)),
                    "false" | "False" | "FALSE" => Ok(json!(false)),
                    "null" | "None" | "NULL" | "nil" | "undefined" | "NaN" | "Infinity" => {
                        if word != "null" {
                            self.fix("replaced non-JSON literals");
                        }
                        Ok(Value::Null)
                    }
                    "" => Err(format!("unexpected character '{}' at position {}", self.s[self.i], self.i)),
                    _ => {
                        self.fix("quoted bare words");
                        Ok(Value::String(word))
                    }
                }
            }
        }
    }

    fn word(&mut self) -> String {
        let st = self.i;
        while self.peek().is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '$') {
            self.i += 1;
        }
        self.s[st..self.i].iter().collect()
    }

    fn object(&mut self) -> Result<Value, String> {
        self.i += 1;
        let mut m = Map::new();
        loop {
            self.ws();
            match self.peek() {
                None => {
                    self.fix("closed brackets the text left open (text was cut off)");
                    return Ok(Value::Object(m));
                }
                Some('}') => {
                    self.i += 1;
                    return Ok(Value::Object(m));
                }
                Some(',') => {
                    self.i += 1;
                    continue;
                }
                Some(']') => {
                    self.fix("fixed a mismatched bracket");
                    self.i += 1;
                    return Ok(Value::Object(m));
                }
                _ => {}
            }
            let key = match self.peek() {
                Some(q @ ('"' | '\'')) => {
                    if q == '\'' {
                        self.fix("replaced single quotes");
                    }
                    self.string(q)
                }
                _ => {
                    let w = self.word();
                    if w.is_empty() {
                        return Err(format!("expected a key at position {}", self.i));
                    }
                    self.fix("quoted unquoted keys");
                    w
                }
            };
            self.ws();
            if self.peek() == Some(':') || self.peek() == Some('=') {
                self.i += 1;
            } else {
                self.fix("added a missing colon");
            }
            let v = self.value()?;
            m.insert(key, v);
            self.ws();
            match self.peek() {
                Some(',') => {
                    self.i += 1;
                    self.ws();
                    if self.peek() == Some('}') {
                        self.fix("removed trailing commas");
                    }
                }
                Some('}') | None => {}
                _ => self.fix("added missing commas"),
            }
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.i += 1;
        let mut a = Vec::new();
        loop {
            self.ws();
            match self.peek() {
                None => {
                    self.fix("closed brackets the text left open (text was cut off)");
                    return Ok(Value::Array(a));
                }
                Some(']') => {
                    self.i += 1;
                    return Ok(Value::Array(a));
                }
                Some('}') => {
                    self.fix("fixed a mismatched bracket");
                    self.i += 1;
                    return Ok(Value::Array(a));
                }
                Some(',') => {
                    self.i += 1;
                    continue;
                }
                _ => {}
            }
            a.push(self.value()?);
            self.ws();
            match self.peek() {
                Some(',') => {
                    self.i += 1;
                    self.ws();
                    if self.peek() == Some(']') {
                        self.fix("removed trailing commas");
                    }
                }
                Some(']') | None => {}
                _ => self.fix("added missing commas"),
            }
        }
    }

    fn string(&mut self, q: char) -> String {
        self.i += 1;
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                c if c == q => return out,
                '\\' => {
                    let Some(e) = self.peek() else { break };
                    self.i += 1;
                    match e {
                        'n' => out.push('\n'),
                        't' => out.push('\t'),
                        'r' => out.push('\r'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'u' => {
                            let hex: String = self.s[self.i..(self.i + 4).min(self.s.len())].iter().collect();
                            match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                                Some(ch) if hex.len() == 4 => {
                                    out.push(ch);
                                    self.i += 4;
                                }
                                _ => out.push_str("\\u"),
                            }
                        }
                        other => out.push(other),
                    }
                }
                '\n' => {
                    self.fix("escaped line breaks inside strings");
                    out.push('\n');
                }
                _ => out.push(c),
            }
        }
        self.fix("closed an unterminated string (text was cut off)");
        out
    }

    fn number(&mut self) -> Value {
        let st = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')) {
            self.i += 1;
        }
        let raw: String = self.s[st..self.i].iter().collect();
        let mut t = raw.trim_start_matches('+').to_owned();
        if t.starts_with('.') {
            t.insert(0, '0');
        } else if t.starts_with("-.") {
            t.insert(1, '0');
        }
        if t.ends_with('.') {
            t.pop();
        }
        if t != raw {
            self.fix("normalized numbers");
        }
        if let Ok(i) = t.parse::<i64>() {
            return json!(i);
        }
        match t.parse::<f64>().ok().and_then(Number::from_f64) {
            Some(n) => Value::Number(n),
            None => {
                self.fix("quoted values that aren't numbers");
                Value::String(raw)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_json_is_untouched() {
        let (v, f) = repair(r#"{"a": [1, 2.5, "x"]}"#).unwrap();
        assert_eq!(v, json!({ "a": [1, 2.5, "x"] }));
        assert!(f.is_empty());
    }

    #[test]
    fn typical_model_output() {
        let text = "Sure! Here is the data:\n```json\n{\n  // the customer\n  'customer': 'Acme',\n  total: 1250.,\n  \"paid\": False,\n  \"lines\": [1, 2,],\n  \"note\": None,\n}\n```\nLet me know if you need more.";
        let (v, f) = repair(text).unwrap();
        assert_eq!(v, json!({ "customer": "Acme", "total": 1250, "paid": false, "lines": [1, 2], "note": null }));
        for want in ["removed code fence", "removed comments", "replaced single quotes", "quoted unquoted keys", "removed trailing commas", "replaced non-JSON literals", "normalized numbers"] {
            assert!(f.iter().any(|x| x == want), "missing fix {want}: {f:?}");
        }
    }

    #[test]
    fn truncated_output_is_closed() {
        let (v, f) = repair(r#"{"items": [{"sku": "A1", "qty": 2}, {"sku": "B2", "name": "Bolt, 5 m"#).unwrap();
        assert_eq!(v, json!({ "items": [{ "sku": "A1", "qty": 2 }, { "sku": "B2", "name": "Bolt, 5 m" }] }));
        assert!(f.iter().any(|x| x.contains("cut off")));
    }

    #[test]
    fn missing_commas_and_prose_after() {
        let (v, f) = repair("[{\"a\": 1 \"b\": 2} {\"a\": 3}] trailing words").unwrap();
        assert_eq!(v, json!([{ "a": 1, "b": 2 }, { "a": 3 }]));
        assert!(f.contains(&"added missing commas".to_owned()) && f.contains(&"removed text after the JSON".to_owned()));
    }

    #[test]
    fn record_interface() {
        let out = run(br#"{"text": "no json here"}"#);
        assert_eq!(out["ok"], json!(false));
        let out = run(br#"{"reply": "{a: 1}", "_options": {"field": "reply", "into": "parsed"}}"#);
        assert_eq!(out["parsed"], json!({ "a": 1 }));
        assert_eq!(out["repaired"], json!(true));
    }
}
