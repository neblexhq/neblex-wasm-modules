//! The individual identifier checks. Each returns a JSON object with at least
//! `type` and `valid`; valid results carry `normalized`, invalid ones `error`.

use serde_json::{json, Value};

/// Picks a check from a field name, e.g. `billing_iban` -> iban, `vat_id` -> vat.
pub fn kind_for_field(field: &str) -> Option<&'static str> {
    let f = field.to_ascii_lowercase();
    let has = |w: &str| f.split(|c: char| !c.is_ascii_alphanumeric()).any(|p| p == w) || f == w;
    if f.contains("iban") {
        Some("iban")
    } else if has("vat") || f.contains("vat_number") || f.contains("vatnumber") || f.contains("vat_id") {
        Some("vat")
    } else if f.contains("gtin") || f.contains("barcode") || ["ean", "ean13", "upc"].iter().any(|w| has(w)) {
        Some("gtin")
    } else if has("aba") || f.contains("routing") {
        Some("aba")
    } else if has("lei") {
        Some("lei")
    } else {
        None
    }
}

pub fn check(kind: &str, raw: &str) -> Value {
    match kind {
        "iban" => iban(raw),
        "vat" => vat(raw),
        "gtin" | "ean" | "upc" => gtin(raw),
        "aba" | "routing" => aba(raw),
        "lei" => lei(raw),
        other => invalid(other, "unknown check type (use iban, vat, gtin, aba or lei)"),
    }
}

pub fn invalid(kind: &str, error: &str) -> Value {
    json!({ "type": kind, "valid": false, "error": error })
}

fn compact(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '.').collect::<String>().to_ascii_uppercase()
}

/// ISO 7064 MOD 97-10 over an alphanumeric string (letters become 10..35).
fn mod97(s: &str) -> Option<u32> {
    let mut rem: u32 = 0;
    for c in s.chars() {
        let v = c.to_digit(36)?;
        rem = if v < 10 { (rem * 10 + v) % 97 } else { (rem * 100 + v) % 97 };
    }
    Some(rem)
}

// IBAN lengths per country, from the SWIFT IBAN registry.
const IBAN_LENGTHS: &[(&str, usize)] = &[
    ("AD", 24), ("AE", 23), ("AL", 28), ("AT", 20), ("AZ", 28), ("BA", 20), ("BE", 16), ("BG", 22),
    ("BH", 22), ("BI", 27), ("BR", 29), ("BY", 28), ("CH", 21), ("CR", 22), ("CY", 28), ("CZ", 24),
    ("DE", 22), ("DJ", 27), ("DK", 18), ("DO", 28), ("EE", 20), ("EG", 29), ("ES", 24), ("FI", 18),
    ("FK", 18), ("FO", 18), ("FR", 27), ("GB", 22), ("GE", 22), ("GI", 23), ("GL", 18), ("GR", 27),
    ("GT", 28), ("HN", 28), ("HR", 21), ("HU", 28), ("IE", 22), ("IL", 23), ("IQ", 23), ("IS", 26),
    ("IT", 27), ("JO", 30), ("KW", 30), ("KZ", 20), ("LB", 28), ("LC", 32), ("LI", 21), ("LT", 20),
    ("LU", 20), ("LV", 21), ("LY", 25), ("MC", 27), ("MD", 24), ("ME", 22), ("MK", 19), ("MN", 20),
    ("MR", 27), ("MT", 31), ("MU", 30), ("NI", 28), ("NL", 18), ("NO", 15), ("OM", 23), ("PK", 24),
    ("PL", 28), ("PS", 29), ("PT", 25), ("QA", 29), ("RO", 24), ("RS", 22), ("RU", 33), ("SA", 24),
    ("SC", 31), ("SD", 18), ("SE", 24), ("SI", 19), ("SK", 24), ("SM", 27), ("SO", 23), ("ST", 25),
    ("SV", 28), ("TL", 23), ("TN", 24), ("TR", 26), ("UA", 29), ("VA", 22), ("VG", 24), ("XK", 20),
    ("YE", 30),
];

fn iban(raw: &str) -> Value {
    let s = compact(raw);
    let s = s.strip_prefix("IBAN").unwrap_or(&s).to_string();
    if s.len() < 5 || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return invalid("iban", "IBAN must be letters and digits only");
    }
    let country = &s[..2];
    let Some(&(_, expected)) = IBAN_LENGTHS.iter().find(|(c, _)| *c == country) else {
        return invalid("iban", &format!("{country} is not an IBAN country"));
    };
    if s.len() != expected {
        return invalid("iban", &format!("{country} IBANs have {expected} characters, got {}", s.len()));
    }
    let rearranged = format!("{}{}", &s[4..], &s[..4]);
    if mod97(&rearranged) != Some(1) {
        return invalid("iban", "check digits do not match");
    }
    let formatted = s.as_bytes().chunks(4).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join(" ");
    json!({ "type": "iban", "valid": true, "normalized": s, "formatted": formatted, "country": country })
}

/// Character classes for VAT patterns: d = digit, a = letter, x = letter or digit.
fn matches(s: &str, pattern: &str) -> bool {
    s.len() == pattern.len()
        && s.chars().zip(pattern.chars()).all(|(c, p)| match p {
            'd' => c.is_ascii_digit(),
            'a' => c.is_ascii_uppercase(),
            'x' => c.is_ascii_alphanumeric(),
            lit => c == lit,
        })
}

fn digits(n: usize) -> String {
    "d".repeat(n)
}

/// Accepted number formats after the country prefix. Format only: registration
/// status needs an online lookup (VIES for EU numbers, HMRC for UK numbers).
fn vat_patterns(country: &str) -> Option<Vec<String>> {
    let p = |v: &[&str]| Some(v.iter().map(|s| s.to_string()).collect());
    let d = |ns: &[usize]| Some(ns.iter().map(|n| digits(*n)).collect());
    match country {
        "AT" => p(&["Udddddddd"]),
        "BE" => p(&["0ddddddddd", "1ddddddddd"]),
        "BG" => d(&[9, 10]),
        "CY" => p(&["dddddddda"]),
        "CZ" => d(&[8, 9, 10]),
        "DE" | "EE" | "PT" => d(&[9]),
        "DK" | "FI" | "HU" | "LU" | "MT" | "SI" => d(&[8]),
        "EL" => d(&[9]),
        "ES" => p(&["xdddddddx"]),
        "FR" => p(&["xxddddddddd"]),
        "HR" | "IT" | "LV" => d(&[11]),
        "IE" => p(&["ddddddda", "dddddddaa", "dxddddda"]),
        "LT" => d(&[9, 12]),
        "NL" => p(&["dddddddddBdd"]),
        "PL" | "SK" => d(&[10]),
        "RO" => d(&[2, 3, 4, 5, 6, 7, 8, 9, 10]),
        "SE" => p(&["dddddddddd01"]),
        "GB" | "XI" => p(&["ddddddddd", "dddddddddddd", "GDddd", "HAddd"]),
        _ => None,
    }
}

fn vat(raw: &str) -> Value {
    let s = compact(raw);
    if s.len() < 4 || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return invalid("vat", "VAT number must start with a two-letter country prefix, e.g. DE123456789");
    }
    // Greece uses EL in VAT numbers; GR is a common mistake.
    let (country, number) = match &s[..2] {
        "GR" => ("EL", &s[2..]),
        c => (c, &s[2..]),
    };
    let Some(patterns) = vat_patterns(country) else {
        return invalid("vat", &format!("{country} is not an EU, UK or Northern Ireland VAT prefix"));
    };
    if !patterns.iter().any(|p| matches(number, p)) {
        return invalid("vat", &format!("{country} VAT numbers do not have this format"));
    }
    json!({ "type": "vat", "valid": true, "normalized": format!("{country}{number}"), "country": country, "check": "format" })
}

/// GS1 mod-10 check digit for GTIN-8, UPC-A (12), EAN-13 and GTIN-14.
fn gtin(raw: &str) -> Value {
    let s = compact(raw);
    if !s.chars().all(|c| c.is_ascii_digit()) {
        return invalid("gtin", "GTIN must be digits only");
    }
    let kind = match s.len() {
        8 => "GTIN-8",
        12 => "UPC-A",
        13 => "EAN-13",
        14 => "GTIN-14",
        n => return invalid("gtin", &format!("GTIN must have 8, 12, 13 or 14 digits, got {n}")),
    };
    let d: Vec<u32> = s.chars().map(|c| c.to_digit(10).unwrap()).collect();
    let (body, check) = d.split_at(d.len() - 1);
    let sum: u32 = body.iter().rev().enumerate().map(|(i, v)| if i % 2 == 0 { v * 3 } else { *v }).sum();
    if (10 - sum % 10) % 10 != check[0] {
        return invalid("gtin", "check digit does not match");
    }
    json!({ "type": "gtin", "valid": true, "normalized": s, "gtin14": format!("{:0>14}", s), "format": kind })
}

/// ABA routing number: 9 digits, valid Federal Reserve prefix, 3-7-1 checksum.
fn aba(raw: &str) -> Value {
    let s = compact(raw);
    if s.len() != 9 || !s.chars().all(|c| c.is_ascii_digit()) {
        return invalid("aba", "routing number must be 9 digits");
    }
    let prefix: u32 = s[..2].parse().unwrap();
    if !matches!(prefix, 0..=12 | 21..=32 | 61..=72 | 80) {
        return invalid("aba", "first two digits are not a valid routing prefix");
    }
    let d: Vec<u32> = s.chars().map(|c| c.to_digit(10).unwrap()).collect();
    let sum: u32 = d.iter().zip([3, 7, 1].iter().cycle()).map(|(a, w)| a * w).sum();
    if sum % 10 != 0 {
        return invalid("aba", "check digit does not match");
    }
    json!({ "type": "aba", "valid": true, "normalized": s })
}

/// Legal Entity Identifier (ISO 17442): 18 alphanumerics + 2 check digits, MOD 97-10.
fn lei(raw: &str) -> Value {
    let s = compact(raw);
    if s.len() != 20 || !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return invalid("lei", "LEI must be 20 letters and digits");
    }
    if !s[18..].chars().all(|c| c.is_ascii_digit()) {
        return invalid("lei", "last two characters must be digits");
    }
    if mod97(&s) != Some(1) {
        return invalid("lei", "check digits do not match");
    }
    json!({ "type": "lei", "valid": true, "normalized": s })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(kind: &str, v: &str) -> bool {
        check(kind, v)["valid"] == true
    }

    #[test]
    fn ibans() {
        assert!(ok("iban", "DE89 3704 0044 0532 0130 00"));
        assert!(ok("iban", "GB82WEST12345698765432"));
        assert!(ok("iban", "fr14 2004 1010 0505 0001 3m02 606"));
        assert!(!ok("iban", "DE89370400440532013001"));
        assert!(!ok("iban", "DE8937040044053201300"));
        assert!(!ok("iban", "ZZ89370400440532013000"));
        assert_eq!(check("iban", "de89370400440532013000")["formatted"], "DE89 3704 0044 0532 0130 00");
    }

    #[test]
    fn gtins() {
        assert!(ok("gtin", "4006381333931"));
        assert!(ok("gtin", "036000291452"));
        assert!(ok("gtin", "96385074"));
        assert!(!ok("gtin", "4006381333932"));
        assert!(!ok("gtin", "12345"));
    }

    #[test]
    fn routing_numbers() {
        assert!(ok("aba", "011000015"));
        assert!(ok("aba", "021000021"));
        assert!(!ok("aba", "021000022"));
        assert!(!ok("aba", "991000015"));
    }

    #[test]
    fn leis() {
        let mut body = "5493001KJTIIGC8Y1R".to_string();
        let rem = mod97(&format!("{body}00")).unwrap();
        body.push_str(&format!("{:02}", 98 - rem));
        assert!(ok("lei", &body));
        assert!(!ok("lei", &format!("{}{}", &body[..18], "00")));
    }

    #[test]
    fn vat_formats() {
        assert!(ok("vat", "DE123456789"));
        assert!(ok("vat", "ATU12345678"));
        assert!(ok("vat", "NL123456789B01"));
        assert!(ok("vat", "GR123456789"));
        assert_eq!(check("vat", "GR123456789")["normalized"], "EL123456789");
        assert!(ok("vat", "GB123456789"));
        assert!(!ok("vat", "DE12345678"));
        assert!(!ok("vat", "US123456789"));
    }

    #[test]
    fn field_names() {
        assert_eq!(kind_for_field("billing_iban"), Some("iban"));
        assert_eq!(kind_for_field("VAT_ID"), Some("vat"));
        assert_eq!(kind_for_field("product_barcode"), Some("gtin"));
        assert_eq!(kind_for_field("routing_number"), Some("aba"));
        assert_eq!(kind_for_field("lei"), Some("lei"));
        assert_eq!(kind_for_field("release_date"), None);
        assert_eq!(kind_for_field("private_note"), None);
        assert_eq!(kind_for_field("clean_email"), None);
        assert_eq!(kind_for_field("upc"), Some("gtin"));
    }
}
