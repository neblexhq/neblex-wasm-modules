//! Neblex sandboxed module: PDF (base64 inside the step input JSON) -> JSON text.
//! Contract (from the Worker runtime): export `memory`, `alloc(i32)->i32`,
//! `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len; no imports.
//! Deliberately small: a text-only PDF reader; opens encrypted PDFs that need no password;
//! no images or OCR.

mod crypt;
mod pdf;

use base64::Engine;
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
    let out = serde_json::to_vec(&run(input)).unwrap_or_else(|_| b"{\"ok\":false,\"error\":\"serialize\"}".to_vec());
    let out = out.leak();
    ((out.as_ptr() as i64) << 32) | (out.len() as i64)
}

fn run(input: &[u8]) -> Value {
    let value: Value = match serde_json::from_slice(input) {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    };
    let Some((field, b64)) = find_pdf(&value, String::new()) else {
        return json!({ "ok": false, "error": "no base64 PDF found in input (expected a string starting with JVBERi or a data:application/pdf URL)" });
    };
    let cleaned: String = b64.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = match base64::engine::general_purpose::STANDARD.decode(cleaned) {
        Ok(b) => b,
        Err(e) => return json!({ "ok": false, "error": format!("invalid base64 in {field}: {e}") }),
    };
    let doc = match pdf::extract(&bytes) {
        Ok(d) => d,
        Err(e) => return json!({ "ok": false, "source_field": field, "error": e }),
    };
    let text = doc.pages.join("\n\n");
    let pages: Vec<Value> = doc.pages.iter().enumerate().map(|(i, t)| json!({ "page": i + 1, "text": t })).collect();
    json!({
        "ok": true,
        "source_field": field,
        "page_count": pages.len(),
        "metadata": doc.metadata,
        "has_text": !text.trim().is_empty(),
        "text": text,
        "pages": pages,
    })
}

/// Depth-first search for the first string that is a base64 PDF.
fn find_pdf(v: &Value, path: String) -> Option<(String, String)> {
    match v {
        Value::String(s) => {
            let s = s.trim_start();
            if s.starts_with("JVBERi") {
                return Some((path, s.to_string()));
            }
            s.strip_prefix("data:application/pdf;base64,").map(|rest| (path, rest.to_string()))
        }
        Value::Object(m) => m.iter().find_map(|(k, x)| {
            find_pdf(x, if path.is_empty() { k.clone() } else { format!("{path}.{k}") })
        }),
        Value::Array(a) => a.iter().enumerate().find_map(|(i, x)| find_pdf(x, format!("{path}[{i}]"))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;
    use miniz_oxide::deflate::compress_to_vec_zlib;

    /// Writes numbered objects (1 is the catalog) with an xref table. A `Some(data)` entry is
    /// written as a stream with that dictionary content; `info`, if given, is the /Info object.
    fn build(objs: Vec<(Vec<u8>, Option<Vec<u8>>)>, info: Option<&[u8]>) -> Vec<u8> {
        let mut all = objs;
        if let Some(i) = info {
            all.push((i.to_vec(), None));
        }
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, (dict, stream)) in all.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).bytes());
            match stream {
                Some(data) => {
                    out.extend(format!("<< /Length {} ", data.len()).bytes());
                    out.extend(dict);
                    out.extend(b" >>\nstream\n");
                    out.extend(data);
                    out.extend(b"\nendstream");
                }
                None => out.extend(dict),
            }
            out.extend(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", all.len() + 1).bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").bytes());
        }
        let info_ref = if info.is_some() { format!(" /Info {} 0 R", all.len()) } else { String::new() };
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R{info_ref} >>\nstartxref\n{xref}\n%%EOF\n", all.len() + 1).bytes());
        out
    }

    /// One page per content stream, all using Helvetica as /F1. `filter` goes in each stream dictionary.
    fn doc(contents: &[Vec<u8>], filter: &str, info: Option<&[u8]>) -> Vec<u8> {
        let kids: Vec<String> = (0..contents.len()).map(|i| format!("{} 0 R", 5 + 2 * i)).collect();
        let mut objs = vec![
            (b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), None),
            (format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), contents.len()).into_bytes(), None),
            (b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(), None),
        ];
        for (i, c) in contents.iter().enumerate() {
            objs.push((filter.as_bytes().to_vec(), Some(c.clone())));
            let page = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {} 0 R /Resources << /Font << /F1 3 0 R >> >> >>", 4 + 2 * i);
            objs.push((page.into_bytes(), None));
        }
        build(objs, info)
    }

    fn call(input: Value) -> Value {
        run(&serde_json::to_vec(&input).unwrap())
    }

    fn read(pdf: &[u8]) -> Value {
        call(json!({ "file": STANDARD.encode(pdf) }))
    }

    fn ascii85(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for chunk in data.chunks(4) {
            let mut b = [0u8; 4];
            b[..chunk.len()].copy_from_slice(chunk);
            let mut n = u32::from_be_bytes(b);
            let mut digits = [0u8; 5];
            for d in digits.iter_mut().rev() {
                *d = (n % 85) as u8 + b'!';
                n /= 85;
            }
            out.extend(&digits[..chunk.len() + 1]);
        }
        out.extend(b"~>");
        out
    }

    const INVOICE: &[u8] = b"BT /F1 14 Tf 72 720 Td (Invoice INV-1001) Tj 0 -20 Td (Bill to: Globex) Tj 0 -20 Td (Total: 1,250.00 USD) Tj ET";
    const INVOICE_TEXT: &str = "Invoice INV-1001\nBill to: Globex\nTotal: 1,250.00 USD";

    #[test]
    fn reads_text_metadata_and_where_the_pdf_was() {
        let pdf = doc(&[INVOICE.to_vec()], "", Some(b"<< /Title (Invoice INV-1001) /Author (Acme Corp) >>"));
        let out = call(json!({ "order_id": 7, "attachment": { "content": STANDARD.encode(pdf) } }));
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["source_field"], json!("attachment.content"));
        assert_eq!(out["page_count"], json!(1));
        assert_eq!(out["has_text"], json!(true));
        assert_eq!(out["text"], json!(INVOICE_TEXT));
        assert_eq!(out["metadata"], json!({ "title": "Invoice INV-1001", "author": "Acme Corp" }));
    }

    #[test]
    fn flate_pages_from_a_data_url() {
        let p2 = b"BT /F1 12 Tf 1 0 0 1 72 700 Tm (Line one) Tj 1 0 0 1 72 680 Tm (Line two) Tj ET";
        let pdf = doc(&[compress_to_vec_zlib(INVOICE, 6), compress_to_vec_zlib(p2, 6)], "/Filter /FlateDecode", None);
        let out = call(json!({ "file": format!("data:application/pdf;base64,{}", STANDARD.encode(pdf)) }));
        assert_eq!(out["page_count"], json!(2));
        assert_eq!(out["pages"][1], json!({ "page": 2, "text": "Line one\nLine two" }));
        assert_eq!(out["text"], json!(format!("{INVOICE_TEXT}\n\nLine one\nLine two")));
    }

    #[test]
    fn ascii85_over_flate_as_reportlab_writes_it() {
        let pdf = doc(&[ascii85(&compress_to_vec_zlib(INVOICE, 6))], "/Filter [/ASCII85Decode /FlateDecode]", None);
        assert_eq!(read(&pdf)["text"], json!(INVOICE_TEXT));
    }

    #[test]
    fn wide_gap_on_a_line_becomes_two_spaces() {
        let pdf = doc(&[b"BT /F1 10 Tf 72 700 Td (Invoice No: 7) Tj 250 0 Td (Due: 1 Dec 2026) Tj ET".to_vec()], "", None);
        assert_eq!(read(&pdf)["text"], json!("Invoice No: 7  Due: 1 Dec 2026"));
    }

    #[test]
    fn content_split_mid_operator_across_streams() {
        // Word splits a page's /Contents into several streams, even inside one TJ operator.
        let pdf = build(
            vec![
                (b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), None),
                (b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(), None),
                (b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [4 0 R 5 0 R] /Resources << /Font << /F1 6 0 R >> >> >>".to_vec(), None),
                (Vec::new(), Some(b"BT /F1 12 Tf 72 700 Td [(would be prepared to ad)".to_vec())),
                (Vec::new(), Some(b"4(just)] TJ ET".to_vec())),
                (b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(), None),
            ],
            None,
        );
        assert_eq!(read(&pdf)["text"], json!("would be prepared to adjust"));
    }

    #[test]
    fn simple_font_reads_one_byte_codes_whatever_its_cmap_says() {
        // Acrobat PDFMaker gives WinAnsi TrueType fonts a ToUnicode CMap with a two-byte codespace.
        let cmap = b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap 1 begincodespacerange <0000> <FFFF> endcodespacerange 1 beginbfchar <41> <0058> endbfchar endcmap end end";
        let pdf = build(
            vec![
                (b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(), None),
                (b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(), None),
                (b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_vec(), None),
                (Vec::new(), Some(b"BT /F1 12 Tf 72 700 Td (National ABC) Tj ET".to_vec())),
                (b"<< /Type /Font /Subtype /TrueType /BaseFont /Calibri /Encoding /WinAnsiEncoding /ToUnicode 6 0 R >>".to_vec(), None),
                (Vec::new(), Some(cmap.to_vec())),
            ],
            None,
        );
        // The CMap maps A to X; every other code falls back to WinAnsi.
        assert_eq!(read(&pdf)["text"], json!("National XBC"));
    }

    #[test]
    fn metadata_in_pdfdocencoding_and_utf16() {
        let info = b"<< /Title (Q3 \x84 final) /Author <FEFF00C9006D0069006C0065> >>";
        let out = read(&doc(&[INVOICE.to_vec()], "", Some(info)));
        assert_eq!(out["metadata"], json!({ "title": "Q3 \u{2014} final", "author": "\u{c9}mile" }));
    }

    #[test]
    fn object_streams() {
        let out = read(include_bytes!("../test/fixtures/object-streams.pdf"));
        assert_eq!(out["text"], json!("Invoice INV-1001\nTotal: 1,250.00 USD"));
    }

    #[test]
    fn encrypted_pdfs_that_open_without_a_password() {
        // Made with qpdf, an independent implementation, from the same one-page invoice.
        let files: [(&str, &[u8]); 4] = [
            ("rc4-40", include_bytes!("../test/fixtures/encrypted-rc4-40.pdf")),
            ("rc4-128", include_bytes!("../test/fixtures/encrypted-rc4-128.pdf")),
            ("aes-128", include_bytes!("../test/fixtures/encrypted-aes-128.pdf")),
            ("aes-256", include_bytes!("../test/fixtures/encrypted-aes-256.pdf")),
        ];
        for (name, pdf) in files {
            let out = read(pdf);
            assert_eq!(out["text"], json!("Invoice INV-1001\nTotal: 1,250.00 USD"), "{name}");
            assert_eq!(out["metadata"]["title"], json!("Invoice INV-1001"), "{name}");
        }
    }

    #[test]
    fn encrypted_pdf_that_needs_a_password_says_so() {
        let out = read(include_bytes!("../test/fixtures/encrypted-needs-password.pdf"));
        assert_eq!(out["ok"], json!(false));
        assert!(out["error"].as_str().unwrap().contains("needs a password"), "{out}");
    }

    #[test]
    fn page_without_text_reports_has_text_false() {
        let out = read(&doc(&[b"q 612 0 0 792 0 0 cm Q".to_vec()], "", None));
        assert_eq!(out["ok"], json!(true));
        assert_eq!(out["has_text"], json!(false));
    }

    #[test]
    fn errors_are_returned_not_raised() {
        assert!(run(b"not json")["error"].as_str().unwrap().starts_with("input is not JSON"));
        assert!(call(json!({ "nothing": "here" }))["error"].as_str().unwrap().starts_with("no base64 PDF found"));
        assert!(call(json!({ "file": "JVBERi!!" }))["error"].as_str().unwrap().starts_with("invalid base64 in file"));
        assert_eq!(read(b"%PDF-1.4\ngarbage")["error"], json!("could not read any PDF objects"));
    }
}
