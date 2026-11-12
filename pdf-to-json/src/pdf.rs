//! Minimal text-only PDF reader: scans objects (incl. object streams), walks the
//! page tree in order, decodes Flate content streams and maps glyphs to Unicode
//! through ToUnicode CMaps, /Differences or WinAnsi.

use std::collections::HashMap;

use serde_json::{Map, Value};

const INFLATE_LIMIT: usize = 64 * 1024 * 1024;
const MAX_PAGES: usize = 5000;

type Dict = HashMap<String, Obj>;

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum Obj {
    Null,
    Bool(bool),
    Num(f64),
    Str(Vec<u8>),
    Name(String),
    Arr(Vec<Obj>),
    Dict(Dict),
    Ref(u32),
    Stream(Dict, Vec<u8>),
    Op(String),
}

static NULL: Obj = Obj::Null;

pub struct Doc {
    pub pages: Vec<String>,
    pub metadata: Value,
}

pub fn extract(bytes: &[u8]) -> Result<Doc, String> {
    if !bytes.starts_with(b"%PDF") && find(bytes, b"%PDF", 0).map_or(true, |p| p > 1024) {
        return Err("input is not a PDF (no %PDF header)".into());
    }
    let (objs, trailers) = scan(bytes);
    if objs.is_empty() {
        return Err("could not read any PDF objects".into());
    }
    let pdf = Pdf { objs };
    if trailers.iter().any(|t| t.contains_key("Encrypt")) {
        return Err("encrypted PDFs are not supported".into());
    }

    let mut pages = Vec::new();
    if let Some(root) = trailers.iter().rev().find_map(|t| t.get("Root")) {
        if let Some(tree) = pdf.dict(root).and_then(|c| c.get("Pages")) {
            pdf.walk_pages(tree, None, &mut pages, 0);
        }
    }
    if pages.is_empty() {
        // No usable page tree: fall back to every /Type /Page object in number order.
        let mut nums: Vec<&u32> = pdf.objs.keys().collect();
        nums.sort();
        for n in nums {
            if let Some(d) = as_dict(&pdf.objs[n]) {
                if d.get("Type").and_then(as_name) == Some("Page") {
                    pages.push((d, d.get("Resources")));
                }
            }
        }
    }

    let texts = pages
        .iter()
        .map(|(page, res)| {
            let mut w = Writer::default();
            for data in pdf.contents(page) {
                pdf.run(&data, *res, &mut w, ID, 0);
            }
            w.finish()
        })
        .collect();

    let info = trailers.iter().rev().find_map(|t| t.get("Info")).and_then(|i| pdf.dict(i));
    Ok(Doc { pages: texts, metadata: metadata(&pdf, info) })
}

// ---------------------------------------------------------------- lexer

fn is_ws(c: u8) -> bool {
    matches!(c, 0 | 9 | 10 | 12 | 13 | 32)
}

fn is_delim(c: u8) -> bool {
    matches!(c, b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%')
}

fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    if from >= h.len() {
        return None;
    }
    h[from..].windows(n.len()).position(|w| w == n).map(|p| p + from)
}

struct Lex<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Lex<'a> {
    fn new(b: &'a [u8], i: usize) -> Self {
        Lex { b, i }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        loop {
            while self.peek().is_some_and(is_ws) {
                self.i += 1;
            }
            if self.peek() == Some(b'%') {
                while self.peek().is_some_and(|c| c != b'\n' && c != b'\r') {
                    self.i += 1;
                }
            } else {
                break;
            }
        }
    }

    fn word(&mut self) -> &'a [u8] {
        let s = self.i;
        while self.peek().is_some_and(|c| !is_ws(c) && !is_delim(c)) {
            self.i += 1;
        }
        &self.b[s..self.i]
    }

    /// Parses one object; `refs` enables `N G R` references (off in content streams).
    /// Returns None at the end or on a stray delimiter (which is consumed).
    fn parse(&mut self, refs: bool, depth: usize) -> Option<Obj> {
        self.skip_ws();
        let c = self.peek()?;
        if depth > 64 {
            self.i += 1;
            return None;
        }
        match c {
            b'/' => {
                self.i += 1;
                Some(Obj::Name(decode_name(self.word())))
            }
            b'(' => Some(Obj::Str(self.literal())),
            b'<' if self.b.get(self.i + 1) == Some(&b'<') => {
                self.i += 2;
                let mut d = Dict::new();
                loop {
                    self.skip_ws();
                    match self.peek() {
                        None => break,
                        Some(b'>') => {
                            self.i += if self.b.get(self.i + 1) == Some(&b'>') { 2 } else { 1 };
                            break;
                        }
                        _ => {}
                    }
                    let key = match self.parse(refs, depth + 1) {
                        Some(Obj::Name(k)) => k,
                        _ => continue,
                    };
                    let val = self.parse(refs, depth + 1).unwrap_or(Obj::Null);
                    d.insert(key, val);
                }
                Some(Obj::Dict(d))
            }
            b'<' => {
                self.i += 1;
                let mut hex = Vec::new();
                while let Some(c) = self.peek() {
                    self.i += 1;
                    if c == b'>' {
                        break;
                    }
                    if c.is_ascii_hexdigit() {
                        hex.push(c);
                    }
                }
                if hex.len() % 2 == 1 {
                    hex.push(b'0');
                }
                Some(Obj::Str(hex.chunks(2).map(|p| hexval(p[0]) << 4 | hexval(p[1])).collect()))
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                loop {
                    self.skip_ws();
                    match self.peek() {
                        None => break,
                        Some(b']') => {
                            self.i += 1;
                            break;
                        }
                        _ => {}
                    }
                    if let Some(o) = self.parse(refs, depth + 1) {
                        a.push(o);
                    }
                }
                Some(Obj::Arr(a))
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                let w = self.word();
                let n: f64 = std::str::from_utf8(w).ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
                if refs && w.iter().all(u8::is_ascii_digit) {
                    let save = self.i;
                    self.skip_ws();
                    let gen = self.word();
                    if !gen.is_empty() && gen.iter().all(u8::is_ascii_digit) {
                        self.skip_ws();
                        if self.word() == b"R" {
                            return Some(Obj::Ref(n as u32));
                        }
                    }
                    self.i = save;
                }
                Some(Obj::Num(n))
            }
            _ if is_delim(c) => {
                self.i += 1;
                None
            }
            _ => {
                let w = self.word();
                if w.is_empty() {
                    self.i += 1;
                    return None;
                }
                Some(match w {
                    b"true" => Obj::Bool(true),
                    b"false" => Obj::Bool(false),
                    b"null" => Obj::Null,
                    _ => Obj::Op(String::from_utf8_lossy(w).into_owned()),
                })
            }
        }
    }

    fn literal(&mut self) -> Vec<u8> {
        self.i += 1;
        let mut out = Vec::new();
        let mut depth = 1;
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'\\' => {
                    let Some(e) = self.peek() else { break };
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.i += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(v as u8);
                        }
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.i += 1;
                            }
                        }
                        b'\n' => {}
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    out.push(c);
                }
                _ => out.push(c),
            }
        }
        out
    }
}

fn hexval(c: u8) -> u8 {
    (c as char).to_digit(16).unwrap_or(0) as u8
}

fn decode_name(w: &[u8]) -> String {
    let mut out = Vec::with_capacity(w.len());
    let mut i = 0;
    while i < w.len() {
        if w[i] == b'#' && i + 2 < w.len() && w[i + 1].is_ascii_hexdigit() && w[i + 2].is_ascii_hexdigit() {
            out.push(hexval(w[i + 1]) << 4 | hexval(w[i + 2]));
            i += 3;
        } else {
            out.push(w[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Tokenizes a content stream or CMap, skipping inline image data.
fn tokens(data: &[u8]) -> Vec<Obj> {
    let mut lx = Lex::new(data, 0);
    let mut out = Vec::new();
    while lx.i < data.len() {
        let Some(t) = lx.parse(false, 0) else { continue };
        if matches!(&t, Obj::Op(op) if op == "ID") {
            // Inline image: binary data runs until whitespace + "EI".
            let mut j = lx.i + 1;
            while j + 2 < data.len() && !(is_ws(data[j]) && &data[j + 1..j + 3] == b"EI" && data.get(j + 3).map_or(true, |&c| is_ws(c))) {
                j += 1;
            }
            lx.i = j + 3;
            continue;
        }
        out.push(t);
    }
    out
}

// ---------------------------------------------------------------- objects

fn as_dict(o: &Obj) -> Option<&Dict> {
    match o {
        Obj::Dict(d) | Obj::Stream(d, _) => Some(d),
        _ => None,
    }
}

fn as_name(o: &Obj) -> Option<&str> {
    match o {
        Obj::Name(n) => Some(n),
        _ => None,
    }
}

fn as_num(o: &Obj) -> Option<f64> {
    match o {
        Obj::Num(n) => Some(*n),
        _ => None,
    }
}

/// Finds every `N G obj` in the file (later definitions win, like incremental
/// updates), every `trailer` dict and every cross-reference stream dict, then
/// unpacks object streams.
fn scan(b: &[u8]) -> (HashMap<u32, Obj>, Vec<Dict>) {
    let mut objs = HashMap::new();
    let mut trailers = Vec::new();
    let mut p = 0;
    while let Some(at) = find(b, b"obj", p) {
        p = at + 3;
        if at >= 3 && &b[at - 3..at] == b"end" {
            continue;
        }
        if b.get(at + 3).is_some_and(|&c| !is_ws(c) && !is_delim(c)) {
            continue;
        }
        let Some(num) = obj_number(b, at) else { continue };
        let mut lx = Lex::new(b, at + 3);
        let Some(obj) = lx.parse(true, 0) else { continue };
        lx.skip_ws();
        let obj = match obj {
            Obj::Dict(d) if b[lx.i..].starts_with(b"stream") => {
                let mut s = lx.i + 6;
                if b.get(s) == Some(&b'\r') {
                    s += 1;
                }
                if b.get(s) == Some(&b'\n') {
                    s += 1;
                }
                let declared = d.get("Length").and_then(as_num).map(|n| n as usize);
                let end = declared
                    .filter(|&n| s + n <= b.len() && find(b, b"endstream", s + n).is_some_and(|e| e - (s + n) <= 4))
                    .map(|n| s + n)
                    .or_else(|| {
                        find(b, b"endstream", s).map(|mut e| {
                            while e > s && (b[e - 1] == b'\n' || b[e - 1] == b'\r') {
                                e -= 1;
                            }
                            e
                        })
                    })
                    .unwrap_or(b.len());
                p = end;
                if d.get("Type").and_then(as_name) == Some("XRef") {
                    trailers.push(d.clone());
                }
                Obj::Stream(d, b[s..end].to_vec())
            }
            o => o,
        };
        objs.insert(num, obj);
    }

    let mut p = 0;
    while let Some(at) = find(b, b"trailer", p) {
        p = at + 7;
        if let Some(Obj::Dict(d)) = Lex::new(b, p).parse(true, 0) {
            trailers.push(d);
        }
    }

    let pdf = Pdf { objs };
    let mut extra = Vec::new();
    for o in pdf.objs.values() {
        let Obj::Stream(d, _) = o else { continue };
        if d.get("Type").and_then(as_name) != Some("ObjStm") {
            continue;
        }
        let (Some(n), Some(first), Some(data)) = (
            d.get("N").and_then(|x| as_num(pdf.get(x))),
            d.get("First").and_then(|x| as_num(pdf.get(x))),
            pdf.decode(o),
        ) else {
            continue;
        };
        let mut head = Lex::new(&data, 0);
        let mut pairs = Vec::new();
        for _ in 0..(n as usize).min(100_000) {
            match (head.parse(false, 0), head.parse(false, 0)) {
                (Some(Obj::Num(num)), Some(Obj::Num(off))) => pairs.push((num as u32, off as usize)),
                _ => break,
            }
        }
        for (num, off) in pairs {
            if let Some(obj) = Lex::new(&data, first as usize + off).parse(true, 0) {
                extra.push((num, obj));
            }
        }
    }
    let mut objs = pdf.objs;
    for (num, obj) in extra {
        objs.entry(num).or_insert(obj);
    }
    (objs, trailers)
}

/// Reads `N G` backwards from the position of `obj`.
fn obj_number(b: &[u8], at: usize) -> Option<u32> {
    let mut j = at;
    let ws_end = j;
    while j > 0 && is_ws(b[j - 1]) {
        j -= 1;
    }
    if j == ws_end {
        return None;
    }
    let gen_end = j;
    while j > 0 && b[j - 1].is_ascii_digit() {
        j -= 1;
    }
    if j == gen_end {
        return None;
    }
    let ws2 = j;
    while j > 0 && is_ws(b[j - 1]) {
        j -= 1;
    }
    if j == ws2 {
        return None;
    }
    let num_end = j;
    while j > 0 && b[j - 1].is_ascii_digit() {
        j -= 1;
    }
    std::str::from_utf8(&b[j..num_end]).ok()?.parse().ok()
}

struct Pdf {
    objs: HashMap<u32, Obj>,
}

impl Pdf {
    fn get<'a>(&'a self, mut o: &'a Obj) -> &'a Obj {
        for _ in 0..16 {
            match o {
                Obj::Ref(n) => o = self.objs.get(n).unwrap_or(&NULL),
                _ => return o,
            }
        }
        &NULL
    }

    fn dict<'a>(&'a self, o: &'a Obj) -> Option<&'a Dict> {
        as_dict(self.get(o))
    }

    fn decode(&self, o: &Obj) -> Option<Vec<u8>> {
        let Obj::Stream(d, data) = self.get(o) else { return None };
        let filters: Vec<&str> = match d.get("Filter").map(|f| self.get(f)) {
            Some(Obj::Name(n)) => vec![n.as_str()],
            Some(Obj::Arr(a)) => a.iter().filter_map(|f| as_name(self.get(f))).collect(),
            _ => vec![],
        };
        let mut out = data.clone();
        for f in filters {
            out = match f {
                "FlateDecode" | "Fl" => inflate(&out)?,
                "ASCII85Decode" | "A85" => ascii85(&out),
                "ASCIIHexDecode" | "AHx" => {
                    let hex: Vec<u8> = out.iter().copied().take_while(|&c| c != b'>').filter(u8::is_ascii_hexdigit).collect();
                    hex.chunks(2).map(|p| hexval(p[0]) << 4 | hexval(*p.get(1).unwrap_or(&b'0'))).collect()
                }
                _ => return None,
            };
        }
        Some(out)
    }

    fn walk_pages<'a>(&'a self, node: &'a Obj, inherited: Option<&'a Obj>, out: &mut Vec<(&'a Dict, Option<&'a Obj>)>, depth: usize) {
        if depth > 64 || out.len() >= MAX_PAGES {
            return;
        }
        let Some(d) = self.dict(node) else { return };
        let res = d.get("Resources").or(inherited);
        match d.get("Kids").map(|k| self.get(k)) {
            Some(Obj::Arr(kids)) => {
                for kid in kids {
                    self.walk_pages(kid, res, out, depth + 1);
                }
            }
            _ => out.push((d, res)),
        }
    }

    fn contents(&self, page: &Dict) -> Vec<Vec<u8>> {
        match page.get("Contents").map(|c| self.get(c)) {
            Some(Obj::Arr(parts)) => parts.iter().filter_map(|p| self.decode(p)).collect(),
            Some(o @ Obj::Stream(..)) => self.decode(o).into_iter().collect(),
            _ => vec![],
        }
    }

    /// Interprets one content stream, tracking text and graphics matrices so
    /// spaces and line breaks come from where glyphs actually land.
    fn run(&self, data: &[u8], resources: Option<&Obj>, w: &mut Writer, ctm: Mat, depth: usize) {
        let res = resources.and_then(|r| self.dict(r));
        let sub = |key: &str| res.and_then(|r| r.get(key)).and_then(|x| self.dict(x));
        let fonts: HashMap<&str, Font> = sub("Font")
            .map(|fd| fd.iter().filter_map(|(k, v)| self.dict(v).map(|d| (k.as_str(), self.font(d)))).collect())
            .unwrap_or_default();
        let xobjects = sub("XObject");

        let toks = tokens(data);
        let mut g = Gfx { ctm, ..Gfx::default() };
        let mut stack: Vec<Mat> = Vec::new();
        let mut font: Option<&Font> = None;
        let mut start = 0;
        for (i, t) in toks.iter().enumerate() {
            let Obj::Op(op) = t else { continue };
            let args = &toks[start..i];
            start = i + 1;
            let num = |back: usize| args.len().checked_sub(back).and_then(|j| as_num(&args[j])).unwrap_or(0.0);
            let six = || [num(6), num(5), num(4), num(3), num(2), num(1)];
            match op.as_str() {
                "q" => stack.push(g.ctm),
                "Q" => g.ctm = stack.pop().unwrap_or(g.ctm),
                "cm" => g.ctm = mul(six(), g.ctm),
                "BT" => {
                    g.tm = ID;
                    g.tlm = ID;
                }
                "Tf" => {
                    font = args.len().checked_sub(2).and_then(|j| as_name(&args[j])).and_then(|n| fonts.get(n));
                    g.fs = num(1);
                }
                "Tc" => g.tc = num(1),
                "Tw" => g.tw = num(1),
                "TL" => g.tl = num(1),
                "Tz" => g.th = num(1) / 100.0,
                "Td" => g.td(num(2), num(1)),
                "TD" => {
                    g.tl = -num(1);
                    g.td(num(2), num(1));
                }
                "Tm" => {
                    g.tlm = six();
                    g.tm = g.tlm;
                }
                "T*" => g.td(0.0, -g.tl),
                "Tj" => {
                    if let Some(Obj::Str(s)) = args.last() {
                        g.show(font, s, w);
                    }
                }
                "'" | "\"" => {
                    if op == "\"" {
                        g.tw = num(3);
                        g.tc = num(2);
                    }
                    g.td(0.0, -g.tl);
                    if let Some(Obj::Str(s)) = args.last() {
                        g.show(font, s, w);
                    }
                }
                "TJ" => {
                    if let Some(Obj::Arr(a)) = args.last() {
                        for el in a {
                            match el {
                                Obj::Str(s) => g.show(font, s, w),
                                Obj::Num(n) => g.advance(-n / 1000.0 * g.fs * g.th),
                                _ => {}
                            }
                        }
                    }
                }
                "Do" if depth < 4 => {
                    let form = args.last().and_then(as_name).and_then(|n| xobjects?.get(n));
                    let Some(fd) = form.and_then(|f| self.dict(f)) else { continue };
                    if fd.get("Subtype").and_then(as_name) != Some("Form") {
                        continue;
                    }
                    if let Some(inner) = form.and_then(|f| self.decode(f)) {
                        let m = match fd.get("Matrix").map(|m| self.get(m)) {
                            Some(Obj::Arr(a)) if a.len() == 6 => {
                                let v: Vec<f64> = a.iter().map(|x| as_num(x).unwrap_or(0.0)).collect();
                                [v[0], v[1], v[2], v[3], v[4], v[5]]
                            }
                            _ => ID,
                        };
                        let inner_res = fd.get("Resources").or(resources);
                        self.run(&inner, inner_res, w, mul(m, g.ctm), depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    fn font(&self, d: &Dict) -> Font {
        let mut f = Font {
            two_byte: d.get("Subtype").and_then(as_name) == Some("Type0"),
            default_width: 500.0,
            ..Font::default()
        };
        self.widths(d, &mut f);
        if let Some(cmap) = d.get("ToUnicode").and_then(|t| self.decode(t)) {
            let (map, width) = parse_cmap(&cmap);
            f.cmap = map;
            if let Some(width) = width {
                f.two_byte = width == 2;
            }
        }
        if let Some(enc) = d.get("Encoding").and_then(|e| self.dict(e)) {
            if let Some(Obj::Arr(diffs)) = enc.get("Differences").map(|x| self.get(x)) {
                let mut code = 0u32;
                for el in diffs {
                    match el {
                        Obj::Num(n) => code = *n as u32,
                        Obj::Name(n) => {
                            if let Some(s) = glyph(n) {
                                f.diffs.insert(code, s);
                            }
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        f
    }

    /// Glyph widths: /FirstChar + /Widths for simple fonts, /DW + /W for CID fonts.
    fn widths(&self, d: &Dict, f: &mut Font) {
        let arr = |o: Option<&Obj>| match o.map(|x| self.get(x)) {
            Some(Obj::Arr(a)) => Some(a.clone()),
            _ => None,
        };
        let n = |o: &Obj| as_num(self.get(o));
        if let Some(widths) = arr(d.get("Widths")) {
            let first = d.get("FirstChar").and_then(n).unwrap_or(0.0) as u32;
            for (i, wd) in widths.iter().enumerate() {
                if let Some(wd) = n(wd) {
                    f.widths.insert(first + i as u32, wd);
                }
            }
            if let Some(mw) = d.get("FontDescriptor").and_then(|x| self.dict(x)).and_then(|fd| fd.get("MissingWidth")).and_then(n) {
                f.default_width = mw;
            }
        }
        let Some(desc) = arr(d.get("DescendantFonts")).and_then(|a| a.first().cloned()) else { return };
        let Some(cid) = self.dict(&desc) else { return };
        f.default_width = cid.get("DW").and_then(n).unwrap_or(1000.0);
        let Some(w) = arr(cid.get("W")) else { return };
        let mut i = 0;
        while i + 1 < w.len() {
            let Some(c) = n(&w[i]) else { break };
            let c = c as u32;
            match self.get(&w[i + 1]) {
                Obj::Arr(list) => {
                    for (k, wd) in list.iter().enumerate() {
                        if let Some(wd) = n(wd) {
                            f.widths.insert(c + k as u32, wd);
                        }
                    }
                    i += 2;
                }
                other => {
                    let (Some(last), Some(wd)) = (as_num(other), w.get(i + 2).and_then(n)) else { break };
                    for code in c..=(last as u32).min(c + 65535) {
                        f.widths.insert(code, wd);
                    }
                    i += 3;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- layout

type Mat = [f64; 6];

const ID: Mat = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mul(a: Mat, b: Mat) -> Mat {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

/// Text state: matrices, font size, spacing and leading.
struct Gfx {
    ctm: Mat,
    tm: Mat,
    tlm: Mat,
    fs: f64,
    tc: f64,
    tw: f64,
    tl: f64,
    th: f64,
}

impl Default for Gfx {
    fn default() -> Self {
        Gfx { ctm: ID, tm: ID, tlm: ID, fs: 12.0, tc: 0.0, tw: 0.0, tl: 0.0, th: 1.0 }
    }
}

impl Gfx {
    fn td(&mut self, tx: f64, ty: f64) {
        self.tlm = mul([1.0, 0.0, 0.0, 1.0, tx, ty], self.tlm);
        self.tm = self.tlm;
    }

    fn advance(&mut self, tx: f64) {
        self.tm = mul([1.0, 0.0, 0.0, 1.0, tx, 0.0], self.tm);
    }

    fn show(&mut self, font: Option<&Font>, s: &[u8], w: &mut Writer) {
        let m = mul(self.tm, self.ctm);
        let size = (self.fs * m[2].hypot(m[3])).abs().max(0.5);
        w.place(m[4], m[5], size);

        let two = font.is_some_and(|f| f.two_byte);
        let codes: Vec<u32> = if two {
            s.chunks(2).map(|c| c.iter().fold(0u32, |a, &b| a << 8 | b as u32)).collect()
        } else {
            s.iter().map(|&c| c as u32).collect()
        };
        for code in codes {
            w.text(&decode_code(font, code));
            let width = font.map_or(500.0, |f| *f.widths.get(&code).unwrap_or(&f.default_width));
            let ws = if !two && code == 32 { self.tw } else { 0.0 };
            self.advance((width / 1000.0 * self.fs + self.tc + ws) * self.th);
        }
        let end = mul(self.tm, self.ctm);
        w.end = Some((end[4], end[5]));
    }
}

fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    use miniz_oxide::inflate::{decompress_to_vec_with_limit, decompress_to_vec_zlib_with_limit};
    match decompress_to_vec_zlib_with_limit(data, INFLATE_LIMIT) {
        Ok(v) => Some(v),
        // Truncated or slightly corrupt streams are common; keep what decoded.
        Err(e) if !e.output.is_empty() => Some(e.output),
        Err(_) => decompress_to_vec_with_limit(data, INFLATE_LIMIT).ok(),
    }
}

// ASCII85 as PDF uses it (ReportLab writes it in front of Flate): "z" is four zero bytes,
// "~>" ends the data, a final partial group is padded with "u".
fn ascii85(data: &[u8]) -> Vec<u8> {
    let data = data.strip_prefix(b"<~").unwrap_or(data);
    let mut out = Vec::with_capacity(data.len() * 4 / 5);
    let (mut group, mut n) = (0u64, 0);
    for &c in data {
        match c {
            b'~' => break,
            b'z' if n == 0 => out.extend_from_slice(&[0; 4]),
            b'!'..=b'u' => {
                group = group * 85 + (c - b'!') as u64;
                n += 1;
                if n == 5 {
                    out.extend_from_slice(&(group as u32).to_be_bytes());
                    (group, n) = (0, 0);
                }
            }
            _ => {}
        }
    }
    if n > 1 {
        for _ in n..5 {
            group = group * 85 + 84;
        }
        out.extend_from_slice(&(group as u32).to_be_bytes()[..n - 1]);
    }
    out
}

// ---------------------------------------------------------------- fonts

#[derive(Default)]
struct Font {
    two_byte: bool,
    cmap: HashMap<u32, String>,
    diffs: HashMap<u32, String>,
    widths: HashMap<u32, f64>,
    default_width: f64,
}

fn decode_code(font: Option<&Font>, code: u32) -> String {
    let byte = || u8::try_from(code).ok().and_then(win_ansi).map(String::from);
    match font {
        None => byte(),
        Some(f) if f.two_byte => f.cmap.get(&code).cloned(),
        Some(f) => f.cmap.get(&code).or_else(|| f.diffs.get(&code)).cloned().or_else(byte),
    }
    .unwrap_or_default()
}

/// Parses bfchar/bfrange entries; also returns the code width from codespacerange.
fn parse_cmap(data: &[u8]) -> (HashMap<u32, String>, Option<usize>) {
    let toks = tokens(data);
    let mut map = HashMap::new();
    let mut width = None;
    let mut mode = "";
    let mut args: Vec<&Obj> = Vec::new();
    let code = |b: &[u8]| b.iter().fold(0u32, |a, &x| a << 8 | x as u32);
    for t in &toks {
        if let Obj::Op(op) = t {
            match op.as_str() {
                "begincodespacerange" | "beginbfchar" | "beginbfrange" => mode = op_mode(op),
                "endcodespacerange" | "endbfchar" | "endbfrange" => mode = "",
                _ => {}
            }
            args.clear();
            continue;
        }
        if mode.is_empty() {
            continue;
        }
        args.push(t);
        match (mode, args.as_slice()) {
            ("space", [Obj::Str(lo), Obj::Str(_)]) => {
                width.get_or_insert(lo.len());
                args.clear();
            }
            ("char", [Obj::Str(src), Obj::Str(dst)]) => {
                map.insert(code(src), utf16(dst));
                args.clear();
            }
            ("range", [Obj::Str(lo), Obj::Str(hi), dst]) => {
                let (lo, hi) = (code(lo), code(hi));
                if hi >= lo && hi - lo < 65536 {
                    match dst {
                        Obj::Str(base) => {
                            let mut units: Vec<u16> = base.chunks(2).map(|c| c.iter().fold(0u16, |a, &x| a << 8 | x as u16)).collect();
                            for c in lo..=hi {
                                map.insert(c, String::from_utf16_lossy(&units));
                                if let Some(last) = units.last_mut() {
                                    *last = last.wrapping_add(1);
                                }
                            }
                        }
                        Obj::Arr(list) => {
                            for (c, d) in (lo..=hi).zip(list) {
                                if let Obj::Str(d) = d {
                                    map.insert(c, utf16(d));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                args.clear();
            }
            (_, a) if a.len() >= 3 => args.clear(),
            _ => {}
        }
    }
    (map, width)
}

fn op_mode(op: &str) -> &'static str {
    match op {
        "begincodespacerange" => "space",
        "beginbfchar" => "char",
        _ => "range",
    }
}

fn utf16(b: &[u8]) -> String {
    if b.len() == 1 {
        return (b[0] as char).to_string();
    }
    let units: Vec<u16> = b.chunks(2).map(|c| c.iter().fold(0u16, |a, &x| a << 8 | x as u16)).collect();
    String::from_utf16_lossy(&units)
}

fn win_ansi(c: u8) -> Option<char> {
    Some(match c {
        b'\t' | b'\n' | b'\r' => ' ',
        0x20..=0x7e => c as char,
        0x80 => '€',
        0x85 => '…',
        0x91 => '‘',
        0x92 => '’',
        0x93 => '“',
        0x94 => '”',
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x99 => '™',
        0xa0..=0xff => c as char,
        _ => return None,
    })
}

/// Maps an Adobe glyph name to text (common names only).
fn glyph(name: &str) -> Option<String> {
    let base = name.split('.').next().unwrap_or(name);
    if base.chars().count() == 1 {
        return Some(base.to_string());
    }
    if let Some(hex) = base.strip_prefix("uni").or_else(|| base.strip_prefix('u')) {
        if (4..=6).contains(&hex.len()) {
            if let Some(c) = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
                return Some(c.to_string());
            }
        }
    }
    let s = match base {
        "space" | "nbspace" => " ",
        "exclam" => "!",
        "quotedbl" => "\"",
        "numbersign" => "#",
        "dollar" => "$",
        "percent" => "%",
        "ampersand" => "&",
        "quotesingle" => "'",
        "parenleft" => "(",
        "parenright" => ")",
        "asterisk" => "*",
        "plus" => "+",
        "comma" => ",",
        "hyphen" | "minus" => "-",
        "period" => ".",
        "slash" => "/",
        "zero" => "0",
        "one" => "1",
        "two" => "2",
        "three" => "3",
        "four" => "4",
        "five" => "5",
        "six" => "6",
        "seven" => "7",
        "eight" => "8",
        "nine" => "9",
        "colon" => ":",
        "semicolon" => ";",
        "less" => "<",
        "equal" => "=",
        "greater" => ">",
        "question" => "?",
        "at" => "@",
        "bracketleft" => "[",
        "backslash" => "\\",
        "bracketright" => "]",
        "underscore" => "_",
        "braceleft" => "{",
        "bar" => "|",
        "braceright" => "}",
        "quoteleft" => "‘",
        "quoteright" => "’",
        "quotedblleft" => "“",
        "quotedblright" => "”",
        "endash" => "–",
        "emdash" => "—",
        "bullet" => "•",
        "ellipsis" => "…",
        "fi" => "fi",
        "fl" => "fl",
        "ff" => "ff",
        "Euro" | "euro" => "€",
        "copyright" => "©",
        "registered" => "®",
        "trademark" => "™",
        "degree" => "°",
        "section" => "§",
        "eacute" => "é",
        "egrave" => "è",
        "aacute" => "á",
        "oacute" => "ó",
        "uacute" => "ú",
        "iacute" => "í",
        "ntilde" => "ñ",
        "udieresis" => "ü",
        "odieresis" => "ö",
        "adieresis" => "ä",
        "germandbls" => "ß",
        "ccedilla" => "ç",
        _ => return None,
    };
    Some(s.to_string())
}

// ---------------------------------------------------------------- output

#[derive(Default)]
struct Writer {
    out: String,
    /// Where the previous run of text ended (device space).
    end: Option<(f64, f64)>,
}

impl Writer {
    /// Decides between nothing, a space, a column gap or a line break before text at (x, y).
    fn place(&mut self, x: f64, y: f64, size: f64) {
        let Some((ex, ey)) = self.end else { return };
        if (y - ey).abs() > size * 0.5 {
            self.newline();
        } else if x - ex > size * 1.5 {
            self.gap();
        } else if x - ex > size * 0.15 || x < ex - size * 2.0 {
            self.space();
        }
    }

    fn text(&mut self, s: &str) {
        if self.out.len() < 16 * 1024 * 1024 {
            self.out.push_str(s);
        }
    }

    fn space(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with([' ', '\n']) {
            self.out.push(' ');
        }
    }

    /// Two spaces mark a column boundary, as in a two-column invoice header or a table.
    fn gap(&mut self) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push_str("  ");
        }
    }

    fn newline(&mut self) {
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
    }

    fn finish(self) -> String {
        self.out
            .lines()
            .map(squeeze)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Trims a line and shortens runs of whitespace: one stays a space, two or more become
/// two spaces, so column boundaries survive and stray spacing doesn't.
fn squeeze(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut run = 0;
    for c in line.trim().chars() {
        if c.is_whitespace() {
            run += 1;
            continue;
        }
        if run > 0 {
            out.push_str(if run == 1 { " " } else { "  " });
            run = 0;
        }
        out.push(c);
    }
    out
}

fn metadata(pdf: &Pdf, info: Option<&Dict>) -> Value {
    let mut out = Map::new();
    if let Some(info) = info {
        for key in ["Title", "Author", "Subject", "Keywords", "Creator", "Producer", "CreationDate", "ModDate"] {
            if let Some(Obj::Str(b)) = info.get(key).map(|o| pdf.get(o)) {
                let s = if b.starts_with(&[0xFE, 0xFF]) { utf16(&b[2..]) } else { b.iter().map(|&c| c as char).collect() };
                let s = s.trim_matches(char::from(0)).trim().to_string();
                if !s.is_empty() {
                    out.insert(key.to_lowercase(), Value::String(s));
                }
            }
        }
    }
    Value::Object(out)
}
