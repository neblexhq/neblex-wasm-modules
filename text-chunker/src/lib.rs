//! Neblex sandboxed module: split long text into overlapping chunks for embedding and retrieval (RAG).
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the options.

use serde_json::{json, Value};

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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(chunk_record).collect()),
        Ok(v) => chunk_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

struct Options {
    field: String,
    max: usize,
    overlap: usize,
    id_field: Option<String>,
    keep_source: bool,
}

/// Splits the text in `_options.field` (default `text`) and adds `chunks` and `chunk_count`.
fn chunk_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let o = m.remove("_options").unwrap_or(Value::Null);
    let num = |k: &str, d: usize| o.get(k).and_then(Value::as_u64).map(|n| n as usize).unwrap_or(d);
    let opt = Options {
        field: o.get("field").and_then(Value::as_str).unwrap_or("text").to_owned(),
        max: num("max_chars", 1500).max(100),
        overlap: num("overlap_chars", 150),
        id_field: o.get("id_field").and_then(Value::as_str).map(str::to_owned),
        keep_source: o.get("keep_source").and_then(Value::as_bool).unwrap_or(false),
    };
    let Some(text) = m.get(&opt.field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{}' is missing or not text", opt.field)));
        return Value::Object(m);
    };
    let id = opt.id_field.as_ref().and_then(|f| m.get(f)).map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string()));
    if !opt.keep_source {
        m.remove(&opt.field);
    }
    let chunks: Vec<Value> = chunk(&text, opt.max, opt.overlap.min(opt.max / 2))
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let chars = c.text.chars().count();
            let mut v = json!({
                "index": i, "text": c.text, "heading": c.heading, "chars": chars,
                "approx_tokens": chars.div_ceil(4), "start": c.start, "end": c.end, "overlap_chars": c.overlap
            });
            if let Some(id) = &id {
                v["chunk_id"] = json!(format!("{id}-{i}"));
                v["source_id"] = json!(id);
            }
            v
        })
        .collect();
    m.insert("chunk_count".into(), json!(chunks.len()));
    m.insert("chunks".into(), Value::Array(chunks));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

struct Unit {
    start: usize,
    end: usize,
    para: usize,
    section: usize,
}

struct Chunk {
    text: String,
    heading: Option<String>,
    start: usize,
    end: usize,
    overlap: usize,
}

/// Markdown headings (# ...) start sections, blank lines separate paragraphs, list items and
/// sentences are the smallest units; anything longer than `max` is split at word boundaries.
fn chunk(text: &str, max: usize, overlap: usize) -> Vec<Chunk> {
    let s: Vec<char> = text.replace("\r\n", "\n").chars().collect();
    let mut units: Vec<Unit> = Vec::new();
    let mut headings: Vec<Option<String>> = vec![None];
    let (mut para, mut section) = (0, 0);
    let mut i = 0;
    let mut blank_before = true;
    while i < s.len() {
        let line_end = s[i..].iter().position(|c| *c == '\n').map(|p| i + p).unwrap_or(s.len());
        let line: String = s[i..line_end].iter().collect();
        let t = line.trim();
        if t.is_empty() {
            blank_before = true;
        } else if t.starts_with('#') && t.trim_start_matches('#').starts_with(' ') {
            section += 1;
            para += 1;
            headings.push(Some(t.trim_start_matches('#').trim().to_owned()));
            units.push(Unit { start: i, end: line_end, para, section });
            blank_before = true;
        } else {
            let list_item = t.starts_with("- ") || t.starts_with("* ") || t.split_once(". ").is_some_and(|(n, _)| !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit()));
            if blank_before || list_item {
                para += 1;
            }
            blank_before = false;
            for (a, b) in sentences(&s, i, line_end) {
                split_long(&s, a, b, max, &mut |a, b| units.push(Unit { start: a, end: b, para, section }));
            }
        }
        i = line_end + 1;
    }

    let mut chunks = Vec::new();
    let mut k = 0;
    while k < units.len() {
        let first = k;
        // Overlap: whole trailing units of the previous chunk in the same section, up to `overlap`
        // chars, and never so many that the first new unit no longer fits within `max`.
        let mut start = units[first].start;
        if overlap > 0 && first > 0 && units[first - 1].section == units[first].section {
            let mut j = first;
            while j > 0
                && units[j - 1].section == units[first].section
                && units[first].start - units[j - 1].start <= overlap
                && units[first].end - units[j - 1].start <= max
            {
                j -= 1;
            }
            start = units[j].start;
        }
        let ov = units[first].start - start;
        let mut end = units[k].end;
        k += 1;
        while k < units.len() && units[k].section == units[first].section {
            if units[k].end - start > max {
                break;
            }
            // Prefer to end a well-filled chunk at a paragraph boundary.
            if units[k].para != units[k - 1].para && end - start >= max * 6 / 10 {
                break;
            }
            end = units[k].end;
            k += 1;
        }
        let body: String = s[start..end].iter().collect();
        chunks.push(Chunk { text: body.trim().to_owned(), heading: headings[units[first].section].clone(), start, end, overlap: ov });
    }
    chunks
}

/// Sentence ranges within one line: a split after . ! or ? followed by a space and an
/// uppercase letter, digit or quote.
fn sentences(s: &[char], a: usize, b: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut st = a;
    while st < b && s[st].is_whitespace() {
        st += 1;
    }
    let mut i = st;
    while i < b {
        if matches!(s[i], '.' | '!' | '?') && i + 2 < b && s[i + 1] == ' ' && (s[i + 2].is_uppercase() || s[i + 2].is_ascii_digit() || s[i + 2] == '"') {
            out.push((st, i + 1));
            st = i + 2;
            i = st;
            continue;
        }
        i += 1;
    }
    let mut e = b;
    while e > st && s[e - 1].is_whitespace() {
        e -= 1;
    }
    if e > st {
        out.push((st, e));
    }
    out
}

fn split_long(s: &[char], a: usize, b: usize, max: usize, push: &mut dyn FnMut(usize, usize)) {
    let mut st = a;
    while b - st > max {
        let mut cut = st + max;
        while cut > st + max / 2 && !s[cut].is_whitespace() {
            cut -= 1;
        }
        if cut == st + max / 2 {
            cut = st + max;
        }
        push(st, cut);
        st = cut;
        while st < b && s[st].is_whitespace() {
            st += 1;
        }
    }
    if st < b {
        push(st, b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }

    fn doc() -> String {
        let para = |w: &str, n: usize| (0..n).map(|i| format!("{w} sentence number {i} explains one detail.")).collect::<Vec<_>>().join(" ");
        format!("# Returns\n\n{}\n\n{}\n\n## Refunds\n\n{}\n\n- Item one is a list entry.\n- Item two is another.\n", para("Returns", 8), para("Shipping", 8), para("Refund", 5))
    }

    #[test]
    fn chunks_respect_size_sections_and_overlap() {
        let out = go(json!({ "doc_id": "kb-17", "text": doc(), "_options": { "max_chars": 400, "overlap_chars": 90, "id_field": "doc_id" } }));
        let chunks = out["chunks"].as_array().unwrap();
        assert!(chunks.len() >= 4, "{out}");
        for c in chunks {
            assert!(c["chars"].as_u64().unwrap() <= 400, "{c}");
        }
        assert_eq!(chunks[0]["heading"], json!("Returns"));
        assert_eq!(chunks[0]["chunk_id"], json!("kb-17-0"));
        assert!(chunks[0]["text"].as_str().unwrap().starts_with("# Returns"));
        let refunds: Vec<_> = chunks.iter().filter(|c| c["heading"] == "Refunds").collect();
        assert!(!refunds.is_empty());
        // The first chunk of a new section never overlaps into the previous section.
        assert_eq!(refunds[0]["overlap_chars"], json!(0));
        assert!(refunds[0]["text"].as_str().unwrap().starts_with("## Refunds"));
        // Later chunks inside a section repeat the end of the one before.
        assert!(chunks.iter().any(|c| c["overlap_chars"].as_u64().unwrap() > 0));
        assert!(out.get("text").is_none());
    }

    #[test]
    fn short_text_is_one_chunk_and_long_words_split() {
        let out = go(json!({ "text": "Just one short note.", "_options": {} }));
        assert_eq!(out["chunk_count"], json!(1));
        assert_eq!(out["chunks"][0]["text"], json!("Just one short note."));
        assert_eq!(out["chunks"][0]["heading"], Value::Null);
        let long = "x".repeat(450);
        let out = go(json!({ "text": long, "_options": { "max_chars": 200, "overlap_chars": 0 } }));
        let sizes: Vec<_> = out["chunks"].as_array().unwrap().iter().map(|c| c["chars"].clone()).collect();
        assert_eq!(sizes, vec![json!(200), json!(200), json!(50)]);
    }

    #[test]
    fn missing_field() {
        assert_eq!(go(json!({ "body": "x" }))["ok"], json!(false));
    }
}
