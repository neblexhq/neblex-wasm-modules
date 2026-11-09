//! Neblex sandboxed module: turn HTML (emails, web pages, rich-text fields) into clean text or Markdown.
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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(convert_record).collect()),
        Ok(v) => convert_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// Reads the HTML in `_options.field` (default `html`) and adds `text` (or `markdown` with
/// `"format": "markdown"`), `title` and `links`. `_options` is removed.
fn convert_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("html").to_owned();
    let markdown = opt.get("format").and_then(Value::as_str) == Some("markdown");
    let inline_links = opt.get("inline_links").and_then(Value::as_bool).unwrap_or(false);
    let Some(html) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    if !opt.get("keep_source").and_then(Value::as_bool).unwrap_or(false) {
        m.remove(&field);
    }
    let r = convert(&html, markdown, inline_links);
    m.insert(if markdown { "markdown" } else { "text" }.into(), json!(r.text));
    m.insert("title".into(), json!(r.title));
    m.insert("links".into(), json!(r.links.iter().map(|(t, u)| json!({ "text": t, "url": u })).collect::<Vec<_>>()));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

pub struct Converted {
    pub text: String,
    pub title: Option<String>,
    pub links: Vec<(String, String)>,
}

const BLOCK: [&str; 24] = [
    "p", "div", "section", "article", "header", "footer", "main", "aside", "nav", "blockquote", "pre", "table", "tr", "ul", "ol", "dl", "dt", "dd",
    "form", "fieldset", "figure", "figcaption", "address", "center",
];
const SKIP: [&str; 7] = ["script", "style", "head", "noscript", "template", "svg", "iframe"];

struct Out {
    buf: String,
    pending_space: bool,
}

impl Out {
    fn word(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.pending_space && !self.buf.is_empty() && !self.buf.ends_with('\n') && !self.buf.ends_with(' ') {
            self.buf.push(' ');
        }
        self.pending_space = false;
        self.buf.push_str(s);
    }
    fn newlines(&mut self, n: usize) {
        self.pending_space = false;
        while self.buf.ends_with(' ') {
            self.buf.pop();
        }
        if self.buf.is_empty() {
            return;
        }
        let have = self.buf.chars().rev().take_while(|c| *c == '\n').count();
        for _ in have..n {
            self.buf.push('\n');
        }
    }
}

pub fn convert(html: &str, markdown: bool, inline_links: bool) -> Converted {
    let s: Vec<char> = html.chars().collect();
    let mut out = Out { buf: String::new(), pending_space: false };
    let mut title = None;
    let mut links: Vec<(String, String)> = Vec::new();
    let mut lists: Vec<(bool, usize)> = Vec::new(); // (ordered, counter)
    let mut link_href: Option<String> = None;
    let mut link_start = 0usize;
    let mut pre = 0usize;
    let mut i = 0;
    while i < s.len() {
        if is_tag_start(&s, i) {
            // Comments and doctype.
            if s[i..].starts_with(&['<', '!', '-', '-']) {
                i = find(&s, i + 4, "-->").map(|p| p + 3).unwrap_or(s.len());
                continue;
            }
            let end = match find(&s, i, ">") {
                Some(e) => e,
                None => {
                    out.word(&s[i..].iter().collect::<String>());
                    break;
                }
            };
            let tag: String = s[i + 1..end].iter().collect();
            i = end + 1;
            let closing = tag.starts_with('/');
            let name: String = tag.trim_start_matches('/').chars().take_while(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
            if name.is_empty() {
                continue;
            }
            if !closing && SKIP.contains(&name.as_str()) {
                let close = format!("</{name}");
                let lower: String = s[i..].iter().collect::<String>().to_ascii_lowercase();
                let content_end = lower.find(&close).map(|p| i + lower[..p].chars().count()).unwrap_or(s.len());
                if name == "head" {
                    // Keep the <title> from the head.
                    let head: String = s[i..content_end].iter().collect();
                    let hl = head.to_ascii_lowercase();
                    if let (Some(a), Some(b)) = (hl.find("<title"), hl.find("</title")) {
                        if let Some(gt) = hl[a..].find('>') {
                            let t = collapse(&decode(&head[a + gt + 1..b]));
                            if !t.is_empty() {
                                title = Some(t);
                            }
                        }
                    }
                }
                i = find(&s, content_end, ">").map(|p| p + 1).unwrap_or(s.len());
                continue;
            }
            match (name.as_str(), closing) {
                ("title", false) => {
                    let close = find_ci(&s, i, "</title").unwrap_or(s.len());
                    let t = collapse(&decode(&s[i..close].iter().collect::<String>()));
                    if !t.is_empty() {
                        title = Some(t);
                    }
                    i = find(&s, close, ">").map(|p| p + 1).unwrap_or(s.len());
                }
                ("br", _) => out.newlines(1),
                ("hr", _) => {
                    out.newlines(2);
                    if markdown {
                        out.buf.push_str("---");
                        out.newlines(2);
                    }
                }
                (h, false) if h.len() == 2 && h.starts_with('h') && h.as_bytes()[1].is_ascii_digit() => {
                    out.newlines(2);
                    if markdown {
                        let level = (h.as_bytes()[1] - b'0').clamp(1, 6) as usize;
                        out.buf.push_str(&"#".repeat(level));
                        out.buf.push(' ');
                    }
                }
                (h, true) if h.len() == 2 && h.starts_with('h') && h.as_bytes()[1].is_ascii_digit() => out.newlines(2),
                ("ul", false) | ("ol", false) => {
                    lists.push((name == "ol", 0));
                    out.newlines(1);
                }
                ("ul", true) | ("ol", true) => {
                    lists.pop();
                    out.newlines(if lists.is_empty() { 2 } else { 1 });
                }
                ("li", false) => {
                    out.newlines(1);
                    let depth = lists.len().saturating_sub(1);
                    out.buf.push_str(&"  ".repeat(depth));
                    match lists.last_mut() {
                        Some((true, n)) => {
                            *n += 1;
                            out.buf.push_str(&format!("{n}. "));
                        }
                        _ => out.buf.push_str("- "),
                    }
                }
                ("li", true) => out.newlines(1),
                ("td", false) | ("th", false) => {
                    if !out.buf.ends_with('\n') && !out.buf.is_empty() {
                        out.buf.push_str(" | ");
                    }
                }
                ("pre", false) => {
                    out.newlines(2);
                    if markdown {
                        out.buf.push_str("```\n");
                    }
                    pre += 1;
                }
                ("pre", true) => {
                    pre = pre.saturating_sub(1);
                    if markdown {
                        out.newlines(1);
                        out.buf.push_str("```");
                    }
                    out.newlines(2);
                }
                ("strong", _) | ("b", _) if markdown => out.word("**"),
                ("em", _) | ("i", _) if markdown => out.word("*"),
                ("a", false) => {
                    link_href = attr(&tag, "href").filter(|h| !h.is_empty() && !h.starts_with('#') && !h.to_ascii_lowercase().starts_with("javascript:"));
                    if markdown && link_href.is_some() {
                        out.word("[");
                    }
                    link_start = out.buf.len();
                }
                ("a", true) => {
                    if let Some(href) = link_href.take() {
                        let text = collapse(out.buf[link_start.min(out.buf.len())..].trim_start_matches('['));
                        if markdown {
                            out.buf.push_str(&format!("]({href})"));
                        } else if inline_links && text != href {
                            out.buf.push_str(&format!(" ({href})"));
                        }
                        links.push((text, href));
                    }
                }
                ("img", _) => {
                    if let Some(alt) = attr(&tag, "alt").filter(|a| !a.trim().is_empty()) {
                        out.pending_space = true;
                        out.word(&if markdown { format!("![{}]", alt.trim()) } else { alt.trim().to_owned() });
                        out.pending_space = true;
                    }
                }
                (n, false) if BLOCK.contains(&n) => out.newlines(if n == "tr" || n == "dt" || n == "dd" { 1 } else { 2 }),
                (n, true) if BLOCK.contains(&n) => out.newlines(if n == "tr" || n == "dt" || n == "dd" { 1 } else { 2 }),
                _ => {}
            }
            continue;
        }
        // Text run up to the next tag (a "<" that doesn't start a tag is ordinary text).
        let next = (i + 1..s.len()).find(|&k| is_tag_start(&s, k)).unwrap_or(s.len());
        let raw: String = s[i..next].iter().collect();
        let text = decode(&raw);
        if pre > 0 {
            out.word(&text);
        } else {
            if text.starts_with(char::is_whitespace) {
                out.pending_space = true;
            }
            let words: Vec<&str> = text.split_whitespace().collect();
            for (k, w) in words.iter().enumerate() {
                if k > 0 {
                    out.pending_space = true;
                }
                out.word(w);
            }
            if text.ends_with(char::is_whitespace) {
                out.pending_space = true;
            }
        }
        i = next;
    }
    // Tidy: trim each line's trailing spaces, at most one blank line in a row, nbsp to space.
    let mut lines: Vec<String> = Vec::new();
    for l in out.buf.replace('\u{a0}', " ").lines() {
        let l = l.trim_end().to_owned();
        if l.is_empty() && lines.last().is_some_and(|x| x.is_empty()) {
            continue;
        }
        lines.push(l);
    }
    Converted { text: lines.join("\n").trim().to_owned(), title, links }
}

/// True when the "<" at `i` opens a tag, closing tag, comment or doctype.
fn is_tag_start(s: &[char], i: usize) -> bool {
    s[i] == '<' && s.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == '/' || *c == '!')
}

fn find(s: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    (from..s.len().saturating_sub(p.len() - 1)).find(|&k| s[k..k + p.len()] == p[..])
}

fn find_ci(s: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    (from..s.len().saturating_sub(p.len() - 1)).find(|&k| s[k..k + p.len()].iter().zip(&p).all(|(a, b)| a.eq_ignore_ascii_case(b)))
}

/// An attribute value from a tag's inner text, quoted or not.
fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find(name).map(|p| p + from) {
        let before_ok = p == 0 || lower.as_bytes()[p - 1].is_ascii_whitespace();
        let rest = lower[p + name.len()..].trim_start();
        if before_ok && rest.starts_with('=') {
            let start = tag.len() - rest.len() + 1;
            let v = tag[start..].trim_start();
            let value = match v.chars().next() {
                Some(q @ ('"' | '\'')) => v[1..].split(q).next().unwrap_or(""),
                _ => v.split(|c: char| c.is_whitespace() || c == '>').next().unwrap_or(""),
            };
            return Some(decode(value));
        }
        from = p + name.len();
    }
    None
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find('&') {
        out.push_str(&rest[..a]);
        let tail = &rest[a..];
        let semi = tail[..tail.len().min(12)].find(';');
        let decoded = semi.and_then(|e| {
            let name = &tail[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                "ndash" => Some('–'),
                "mdash" => Some('—'),
                "hellip" => Some('…'),
                "lsquo" => Some('‘'),
                "rsquo" => Some('’'),
                "ldquo" => Some('“'),
                "rdquo" => Some('”'),
                "copy" => Some('©'),
                "reg" => Some('®'),
                "trade" => Some('™'),
                "euro" => Some('€'),
                "pound" => Some('£'),
                "bull" => Some('•'),
                "middot" => Some('·'),
                n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            }?;
            Some((c, e + 1))
        });
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &tail[used..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMAIL: &str = r##"<!DOCTYPE html><html><head><title>Your order &amp; receipt</title><style>p{color:red}</style></head>
<body><!-- tracking pixel --><h1>Order update</h1><p>Your order <b>SO-1001</b> has
   shipped.</p><ul><li>Widget x 2</li><li>Bolt x 5<ul><li>Zinc</li></ul></li></ul>
<ol><li>Unpack</li><li>Enjoy</li></ol><p>Track it <a href="https://example.com/t/1">here</a>.&nbsp;Questions? Call&#160;us.</p>
<table><tr><th>Item</th><th>Qty</th></tr><tr><td>Widget</td><td>2</td></tr></table>
<p><img src="logo.png" alt="Acme logo"><a href="#top">Back to top</a> <a href="mailto:help@example.com">help@example.com</a></p>
<script>alert(1)</script></body></html>"##;

    #[test]
    fn text_output() {
        let r = convert(EMAIL, false, false);
        assert_eq!(r.title.as_deref(), Some("Your order & receipt"));
        assert_eq!(
            r.text,
            "Order update\n\nYour order SO-1001 has shipped.\n\n- Widget x 2\n- Bolt x 5\n  - Zinc\n\n1. Unpack\n2. Enjoy\n\nTrack it here. Questions? Call us.\n\nItem | Qty\nWidget | 2\n\nAcme logo Back to top help@example.com"
        );
        assert_eq!(r.links, vec![("here".to_owned(), "https://example.com/t/1".to_owned()), ("help@example.com".to_owned(), "mailto:help@example.com".to_owned())]);
    }

    #[test]
    fn markdown_output() {
        let r = convert(EMAIL, true, false);
        assert!(r.text.starts_with("# Order update\n\nYour order **SO-1001** has shipped."), "{}", r.text);
        assert!(r.text.contains("Track it [here](https://example.com/t/1)."), "{}", r.text);
        assert!(r.text.contains("![Acme logo]"));
        assert!(!r.text.contains("alert"));
    }

    #[test]
    fn inline_links_and_record_interface() {
        let out = run(br#"{"body": "<p>See <a href='https://acme.com/p?a=1&amp;b=2'>pricing</a></p>", "_options": {"field": "body", "inline_links": true}}"#);
        assert_eq!(out["text"], json!("See pricing (https://acme.com/p?a=1&b=2)"));
        assert_eq!(out["links"][0]["url"], json!("https://acme.com/p?a=1&b=2"));
        assert!(out.get("body").is_none());
        assert_eq!(run(br#"{"x": 1}"#)["ok"], json!(false));
    }

    #[test]
    fn plain_text_and_broken_html() {
        assert_eq!(convert("Just text &lt;b&gt; 5 &gt; 3", false, false).text, "Just text <b> 5 > 3");
        assert_eq!(convert("<p>unclosed <b>bold", false, false).text, "unclosed bold");
        assert_eq!(convert("a < b and <pre>  keep   spaces\n  here</pre>", false, false).text, "a < b and\n\n  keep   spaces\n  here");
    }
}
