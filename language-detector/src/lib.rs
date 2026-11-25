//! Neblex sandboxed module: detect the language and writing system of a text (69 languages),
//! with a confidence score, so tickets, emails and reviews can be routed without an AI call.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the fields.

use serde_json::{json, Map, Value};
use whatlang::{Detector, Lang};

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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(detect_record).collect()),
        Ok(v) => detect_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// ISO 639-1 two-letter code for a whatlang ISO 639-3 code ("eng" -> "en"); Esperanto is "eo".
pub fn iso1(code3: &str) -> Option<&'static str> {
    Some(match code3 {
        "epo" => "eo", "eng" => "en", "rus" => "ru", "cmn" => "zh", "spa" => "es", "por" => "pt",
        "ita" => "it", "ben" => "bn", "fra" => "fr", "deu" => "de", "ukr" => "uk", "kat" => "ka",
        "ara" => "ar", "hin" => "hi", "jpn" => "ja", "heb" => "he", "yid" => "yi", "pol" => "pl",
        "amh" => "am", "jav" => "jv", "kor" => "ko", "nob" => "nb", "dan" => "da", "swe" => "sv",
        "fin" => "fi", "tur" => "tr", "nld" => "nl", "hun" => "hu", "ces" => "cs", "ell" => "el",
        "bul" => "bg", "bel" => "be", "mar" => "mr", "kan" => "kn", "ron" => "ro", "slv" => "sl",
        "hrv" => "hr", "srp" => "sr", "mkd" => "mk", "lit" => "lt", "lav" => "lv", "est" => "et",
        "tam" => "ta", "vie" => "vi", "urd" => "ur", "tha" => "th", "guj" => "gu", "uzb" => "uz",
        "pan" => "pa", "aze" => "az", "ind" => "id", "tel" => "te", "pes" => "fa", "mal" => "ml",
        "ori" => "or", "mya" => "my", "nep" => "ne", "sin" => "si", "khm" => "km", "tuk" => "tk",
        "aka" => "ak", "zul" => "zu", "sna" => "sn", "afr" => "af", "lat" => "la", "slk" => "sk",
        "cat" => "ca", "tgl" => "tl", "hye" => "hy",
        _ => return None,
    })
}

/// A language code from `_options.languages` ("en", "eng" or "English") as a whatlang language.
pub fn parse_lang(s: &str) -> Option<Lang> {
    let s = s.trim().to_lowercase();
    Lang::from_code(s.as_str()).or_else(|| {
        Lang::all().iter().copied().find(|l| iso1(l.code()) == Some(s.as_str()) || l.eng_name().to_lowercase() == s)
    })
}

/// Strips URLs, email addresses and @handles, which carry no language and skew short texts.
fn clean(text: &str) -> String {
    text.split_whitespace()
        .filter(|w| !(w.contains("://") || w.starts_with("www.") || (w.contains('@') && w.contains('.')) || w.starts_with('@')))
        .collect::<Vec<_>>()
        .join(" ")
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// Reads `_options.field` (default `text`) and adds `language` {code, iso639_1, name, script,
/// confidence, reliable, in_list}; `languages` limits detection to a list, `min_confidence`
/// (default 0) sets `language` to null when the score is lower.
fn detect_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("text").to_owned();
    let into = opt.get("into").and_then(Value::as_str).unwrap_or("language").to_owned();
    let min_conf = opt.get("min_confidence").and_then(Value::as_f64).unwrap_or(0.0);

    let mut allow = Vec::new();
    if let Some(list) = opt.get("languages").and_then(Value::as_array) {
        for v in list {
            match v.as_str().and_then(parse_lang) {
                Some(l) => allow.push(l),
                None => return fail(m, format!("unknown language in _options.languages: {v}")),
            }
        }
    }
    let Some(text) = m.get(&field).and_then(Value::as_str).map(clean) else {
        return fail(m, format!("field '{field}' is missing or not text"));
    };

    let has_list = !allow.is_empty();
    let open = Detector::new();
    let detector = if has_list { Detector::with_allowlist(allow.clone()) } else { Detector::new() };
    // A list only ranks the listed languages against each other, so a text in another language
    // can still score high. When detection without the list is reliable and lands outside the
    // list, report that language with in_list false.
    let info = match (has_list, open.detect(&text)) {
        (true, Some(o)) if o.is_reliable() && !allow.contains(&o.lang()) => Some((o, false)),
        _ => detector.detect(&text).map(|i| (i, true)),
    };
    let result = info.filter(|(i, _)| i.confidence() >= min_conf).map(|(i, in_list)| {
        let code = i.lang().code();
        let mut o = Map::new();
        o.insert("code".into(), json!(code));
        o.insert("iso639_1".into(), json!(iso1(code)));
        o.insert("name".into(), json!(i.lang().eng_name()));
        o.insert("script".into(), json!(i.script().name()));
        o.insert("confidence".into(), json!(round2(i.confidence())));
        o.insert("reliable".into(), json!(i.is_reliable()));
        if has_list {
            o.insert("in_list".into(), json!(in_list));
        }
        Value::Object(o)
    });
    // Script alone still helps for very short texts ("Спасибо", "ありがとう").
    let script = detector.detect_script(&text).map(|s| s.name().to_owned());
    m.insert(into, result.unwrap_or(Value::Null));
    m.insert("script".into(), json!(script));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

fn fail(mut m: Map<String, Value>, error: String) -> Value {
    m.insert("ok".into(), json!(false));
    m.insert("error".into(), json!(error));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(v: Value) -> Value {
        run(serde_json::to_vec(&v).unwrap().as_slice())
    }
    fn lang(text: &str) -> Value {
        go(json!({ "text": text }))["language"].clone()
    }

    #[test]
    fn common_languages() {
        let cases = [
            ("My order arrived damaged and I would like a replacement as soon as possible.", "en"),
            ("Mi pedido llegó dañado y me gustaría un reemplazo lo antes posible, por favor.", "es"),
            ("Meine Bestellung ist beschädigt angekommen und ich hätte gern so schnell wie möglich Ersatz.", "de"),
            ("Bonjour, ma commande est arrivée endommagée hier et je voudrais un remplacement dès que possible, merci de votre aide.", "fr"),
            ("Il mio ordine è arrivato danneggiato e vorrei una sostituzione il prima possibile.", "it"),
            ("Meu pedido chegou danificado e gostaria de uma substituição o mais rápido possível.", "pt"),
            ("Mijn bestelling is beschadigd aangekomen en ik wil zo snel mogelijk een vervanging.", "nl"),
            ("Мой заказ пришёл повреждённым, и я хотел бы получить замену как можно скорее.", "ru"),
            ("ご注文の商品が破損して届きました。できるだけ早く交換をお願いします。", "ja"),
            ("我的订单到货时已经损坏，希望尽快更换。", "zh"),
            ("Zamówienie dotarło uszkodzone i chciałbym jak najszybciej otrzymać wymianę.", "pl"),
            ("Siparişim hasarlı geldi ve en kısa sürede değiştirilmesini istiyorum.", "tr"),
        ];
        for (text, want) in cases {
            let l = lang(text);
            assert_eq!(l["iso639_1"], json!(want), "{text} -> {l}");
            assert_eq!(l["reliable"], json!(true), "{text} -> {l}");
        }
        let en = lang("My order arrived damaged and I would like a replacement as soon as possible.");
        assert_eq!(en["code"], json!("eng"));
        assert_eq!(en["name"], json!("English"));
        assert_eq!(en["script"], json!("Latin"));
    }

    #[test]
    fn allowlist_and_threshold() {
        // Spanish and Portuguese are close; an allowlist of the languages a team supports
        // keeps short texts in the right bucket.
        let r = go(json!({ "text": "Obrigado pela ajuda", "_options": { "languages": ["pt", "en", "es"] } }));
        assert_eq!(r["language"]["iso639_1"], json!("pt"));
        // Short texts: without a list they are flagged unreliable, with one they land right.
        let short = ["Where is my order? It has been two weeks.", "Hola, no puedo iniciar sesión.", "Password reset not working", "Merci beaucoup pour votre aide", "Где мой заказ? Уже прошло две недели."];
        let want = ["en", "es", "en", "fr", "ru"];
        for (t, w) in short.iter().zip(want) {
            let r = go(json!({ "text": t, "_options": { "languages": ["en", "es", "fr", "de", "pt", "ru"] } }));
            assert_eq!(r["language"]["iso639_1"], json!(w), "{t}");
        }
        assert_eq!(lang("Password reset not working")["reliable"], json!(false));
        // Must not match: Turkish is not forced into a listed language.
        let r = go(json!({ "text": "Siparişim hasarlı geldi ve en kısa sürede değiştirilmesini istiyorum.", "_options": { "languages": ["en", "de"] } }));
        assert_eq!(r["language"]["iso639_1"], json!("tr"));
        assert_eq!(r["language"]["in_list"], json!(false));
        let r = go(json!({ "text": "Meine Bestellung ist beschädigt angekommen.", "_options": { "languages": ["en", "de"] } }));
        assert_eq!(r["language"]["iso639_1"], json!("de"));
        assert_eq!(r["language"]["in_list"], json!(true));
        assert!(go(json!({ "text": "Meine Bestellung ist beschädigt angekommen." }))["language"].get("in_list").is_none());
        let r = go(json!({ "text": "Thanks", "_options": { "min_confidence": 0.9 } }));
        assert_eq!(r["language"], Value::Null);
        assert_eq!(r["ok"], json!(true));
        let r = go(json!({ "text": "Hola", "_options": { "languages": ["klingon"] } }));
        assert_eq!(r["ok"], json!(false));
        assert_eq!(parse_lang("English"), Some(Lang::Eng));
        assert_eq!(parse_lang("deu"), Some(Lang::Deu));
        assert_eq!(parse_lang("zh"), Some(Lang::Cmn));
    }

    #[test]
    fn options_and_edge_cases() {
        let r = go(json!({ "body": "Bonjour, je n'arrive pas à me connecter à mon compte depuis hier soir.", "_options": { "field": "body", "into": "lang" } }));
        assert_eq!(r["lang"]["iso639_1"], json!("fr"));
        // URLs and addresses are ignored.
        let r = lang("Bitte sehen Sie sich https://example.com/orders/4411 an, mein Paket ist nicht angekommen. support@example.com");
        assert_eq!(r["iso639_1"], json!("de"));
        // Must not match: no letters at all gives no language, and still ok.
        let r = go(json!({ "text": "12345 !!! 678" }));
        assert_eq!(r["language"], Value::Null);
        assert_eq!(r["script"], Value::Null);
        assert_eq!(r["ok"], json!(true));
        assert_eq!(go(json!({ "text": "" }))["language"], Value::Null);
        assert_eq!(go(json!({ "text": 5 }))["ok"], json!(false));
        assert_eq!(go(json!("text"))["ok"], json!(false));
        assert_eq!(run(b"not json")["ok"], json!(false));
        // Arrays come back in order.
        let r = go(json!([{ "text": "Мой заказ пришёл повреждённым, и я хотел бы получить замену." }, { "text": "Where is my order? It has been two weeks." }]));
        assert_eq!(r[0]["language"]["iso639_1"], json!("ru"));
        assert_eq!(r[1]["language"]["iso639_1"], json!("en"));
        assert_eq!(r[0]["script"], json!("Cyrillic"));
    }

    #[test]
    fn every_language_has_iso1() {
        for l in Lang::all() {
            assert!(iso1(l.code()).is_some(), "{}", l.code());
        }
    }
}
