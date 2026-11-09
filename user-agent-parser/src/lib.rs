//! Neblex sandboxed module: read browser, operating system, device type and engine from a
//! user-agent string, and tell bots and HTTP libraries apart from people.
//! Contract: a core WebAssembly module with no imports that exports `memory`,
//! `alloc(i32)->i32` and `invoke(ptr,len)->i64` returning (out_ptr << 32) | out_len.
//! Input and output are JSON. See README.md for the fields.

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
        Ok(Value::Array(items)) => Value::Array(items.into_iter().map(parse_record).collect()),
        Ok(v) => parse_record(v),
        Err(e) => json!({ "ok": false, "error": format!("input is not JSON: {e}") }),
    }
}

/// The version after `token` (e.g. "Chrome/" -> "120.0.6099.71"), up to a space, ";" or ")".
fn version_after<'a>(ua: &'a str, token: &str) -> Option<&'a str> {
    let i = ua.find(token)? + token.len();
    let rest = &ua[i..];
    let end = rest.find([' ', ';', ')', '(']).unwrap_or(rest.len());
    let v = &rest[..end];
    (!v.is_empty()).then_some(v)
}

fn major(v: Option<&str>) -> Option<String> {
    v.map(|v| v.split('.').next().unwrap_or(v).to_owned())
}

pub struct Parsed {
    pub kind: &'static str,
    pub browser: Option<(String, Option<String>)>,
    pub os: Option<(String, Option<String>)>,
    pub engine: Option<&'static str>,
    pub model: Option<String>,
    pub vendor: Option<&'static str>,
}

pub fn parse(ua: &str) -> Parsed {
    let low = ua.to_lowercase();
    let mut p = Parsed { kind: "desktop", browser: None, os: None, engine: None, model: None, vendor: None };

    // Bots and crawlers: the first token naming one.
    if ["bot", "crawler", "spider", "slurp", "facebookexternalhit", "headlesschrome", "lighthouse", "preview"].iter().any(|b| low.contains(b)) {
        p.kind = "bot";
        let token = ua
            .split(|c: char| c == ' ' || c == ';' || c == '(' || c == ')' || c == ',')
            .find(|t| {
                let l = t.to_lowercase();
                ["bot", "crawler", "spider", "slurp", "facebookexternalhit", "headlesschrome", "lighthouse", "preview"].iter().any(|b| l.contains(b))
            })
            .unwrap_or(ua);
        let (name, ver) = token.split_once('/').map(|(n, v)| (n, Some(v.to_owned()))).unwrap_or((token, None));
        p.browser = Some((name.trim_start_matches('+').to_owned(), ver));
        return p;
    }
    // HTTP libraries and command-line tools.
    for lib in ["curl/", "Wget/", "python-requests/", "python-urllib/", "PostmanRuntime/", "okhttp/", "Go-http-client/", "axios/", "node-fetch/", "Java/", "Apache-HttpClient/", "insomnia/", "HTTPie/", "Dart/", "aiohttp/", "Ruby", "PowerShell/"] {
        if ua.starts_with(lib) || (lib == "PowerShell/" && ua.contains(lib)) {
            p.kind = "library";
            let name = lib.trim_end_matches('/');
            p.browser = Some((name.to_owned(), version_after(ua, &format!("{name}/")).map(str::to_owned)));
            return p;
        }
    }

    // Operating system.
    if let Some(v) = version_after(ua, "Windows NT ") {
        let name = match v {
            "10.0" => "10",
            "6.3" => "8.1",
            "6.2" => "8",
            "6.1" => "7",
            "6.0" => "Vista",
            "5.1" | "5.2" => "XP",
            other => other,
        };
        p.os = Some(("Windows".into(), Some(name.into())));
    } else if ua.contains("iPad") {
        p.os = Some(("iPadOS".into(), version_after(ua, "CPU OS ").map(|v| v.replace('_', "."))));
        p.kind = "tablet";
        p.vendor = Some("Apple");
        p.model = Some("iPad".into());
    } else if ua.contains("iPhone") || ua.contains("iPod") {
        p.os = Some(("iOS".into(), version_after(ua, "iPhone OS ").map(|v| v.replace('_', "."))));
        p.kind = "mobile";
        p.vendor = Some("Apple");
        p.model = Some(if ua.contains("iPod") { "iPod" } else { "iPhone" }.into());
    } else if let Some(v) = version_after(ua, "Android ").or_else(|| ua.contains("Android").then_some("")) {
        p.os = Some(("Android".into(), (!v.is_empty()).then(|| v.to_owned())));
        p.kind = if ua.contains("Mobile") { "mobile" } else { "tablet" };
        // Model: the part after "Android x; " up to ")" or " Build/".
        if let Some(i) = ua.find("Android") {
            let rest = &ua[i..];
            if let Some(semi) = rest.find("; ") {
                let after = &rest[semi + 2..];
                let end = after.find(" Build/").into_iter().chain(after.find(')')).min().unwrap_or(after.len());
                let model = after[..end].trim();
                if !model.is_empty() && model != "K" && !model.starts_with("wv") {
                    p.model = Some(model.to_owned());
                }
            }
        }
    } else if ua.contains("CrOS") {
        p.os = Some(("ChromeOS".into(), None));
    } else if let Some(v) = version_after(ua, "Mac OS X ") {
        p.os = Some(("macOS".into(), Some(v.replace('_', "."))));
        p.vendor = Some("Apple");
    } else if ua.contains("Macintosh") {
        p.os = Some(("macOS".into(), None));
        p.vendor = Some("Apple");
    } else if ua.contains("Linux") || ua.contains("X11") {
        p.os = Some(("Linux".into(), None));
    }

    // Browser and engine, most specific first.
    let b = |name: &str, token: &str| Some((name.to_owned(), version_after(ua, token).map(str::to_owned)));
    let (browser, engine) = if ua.contains("Edg/") || ua.contains("EdgA/") || ua.contains("EdgiOS/") {
        let t = if ua.contains("EdgA/") { "EdgA/" } else if ua.contains("EdgiOS/") { "EdgiOS/" } else { "Edg/" };
        (b("Edge", t), "Blink")
    } else if ua.contains("OPR/") {
        (b("Opera", "OPR/"), "Blink")
    } else if ua.contains("SamsungBrowser/") {
        (b("Samsung Internet", "SamsungBrowser/"), "Blink")
    } else if ua.contains("FxiOS/") {
        (b("Firefox", "FxiOS/"), "WebKit")
    } else if ua.contains("Firefox/") {
        (b("Firefox", "Firefox/"), "Gecko")
    } else if ua.contains("CriOS/") {
        (b("Chrome", "CriOS/"), "WebKit")
    } else if ua.contains("Chromium/") {
        (b("Chromium", "Chromium/"), "Blink")
    } else if ua.contains("Chrome/") {
        (b("Chrome", "Chrome/"), "Blink")
    } else if ua.contains("Safari/") && ua.contains("Version/") {
        (b("Safari", "Version/"), "WebKit")
    } else if ua.contains("MSIE ") {
        (Some(("Internet Explorer".to_owned(), version_after(ua, "MSIE ").map(str::to_owned))), "Trident")
    } else if ua.contains("Trident/7") {
        (Some(("Internet Explorer".to_owned(), Some("11.0".to_owned()))), "Trident")
    } else if ua.contains("AppleWebKit/") {
        (None, "WebKit")
    } else {
        (None, "")
    };
    p.browser = browser;
    p.engine = (!engine.is_empty()).then_some(engine);
    if p.os.is_none() && p.browser.is_none() {
        p.kind = "unknown";
    }
    p
}

/// Reads the user agent from `_options.field` (default `user_agent`) and adds `browser`, `os`,
/// `device`, `engine` and `is_bot`.
fn parse_record(record: Value) -> Value {
    let Value::Object(mut m) = record else {
        return json!({ "ok": false, "error": "each input item must be a JSON object" });
    };
    let opt = m.remove("_options").unwrap_or(Value::Null);
    let field = opt.get("field").and_then(Value::as_str).unwrap_or("user_agent").to_owned();
    let Some(ua) = m.get(&field).and_then(Value::as_str).map(str::to_owned) else {
        m.insert("ok".into(), json!(false));
        m.insert("error".into(), json!(format!("field '{field}' is missing or not text")));
        return Value::Object(m);
    };
    let p = parse(ua.trim());
    let pair = |x: &Option<(String, Option<String>)>| match x {
        Some((n, v)) => json!({ "name": n, "version": v, "major": major(v.as_deref()) }),
        None => Value::Null,
    };
    m.insert("browser".into(), pair(&p.browser));
    m.insert("os".into(), pair(&p.os));
    m.insert("device".into(), json!({ "type": p.kind, "vendor": p.vendor, "model": p.model }));
    m.insert("engine".into(), json!(p.engine));
    m.insert("is_bot".into(), json!(p.kind == "bot"));
    m.insert("ok".into(), json!(true));
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn go(ua: &str) -> Value {
        run(serde_json::to_vec(&json!({ "user_agent": ua })).unwrap().as_slice())
    }

    #[test]
    fn desktop_browsers() {
        let c = go("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.6099.71 Safari/537.36");
        assert_eq!(c["browser"], json!({ "name": "Chrome", "version": "120.0.6099.71", "major": "120" }));
        assert_eq!(c["os"], json!({ "name": "Windows", "version": "10", "major": "10" }));
        assert_eq!(c["device"]["type"], json!("desktop"));
        assert_eq!(c["engine"], json!("Blink"));
        let e = go("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.2210.61");
        assert_eq!(e["browser"]["name"], json!("Edge"));
        let s = go("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Safari/605.1.15");
        assert_eq!(s["browser"], json!({ "name": "Safari", "version": "17.1", "major": "17" }));
        assert_eq!(s["os"]["version"], json!("10.15.7"));
        assert_eq!(s["device"]["vendor"], json!("Apple"));
        let f = go("Mozilla/5.0 (X11; Ubuntu; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0");
        assert_eq!(f["browser"]["name"], json!("Firefox"));
        assert_eq!(f["os"]["name"], json!("Linux"));
        assert_eq!(f["engine"], json!("Gecko"));
        let ie = go("Mozilla/5.0 (Windows NT 6.1; Trident/7.0; rv:11.0) like Gecko");
        assert_eq!(ie["browser"]["name"], json!("Internet Explorer"));
        assert_eq!(ie["os"]["version"], json!("7"));
    }

    #[test]
    fn mobile_and_tablets() {
        let i = go("Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Mobile/15E148 Safari/604.1");
        assert_eq!(i["os"], json!({ "name": "iOS", "version": "17.1", "major": "17" }));
        assert_eq!(i["device"], json!({ "type": "mobile", "vendor": "Apple", "model": "iPhone" }));
        let pad = go("Mozilla/5.0 (iPad; CPU OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/120.0.6099.119 Mobile/15E148 Safari/604.1");
        assert_eq!(pad["device"]["type"], json!("tablet"));
        assert_eq!(pad["browser"]["name"], json!("Chrome"));
        assert_eq!(pad["os"]["name"], json!("iPadOS"));
        let a = go("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.6099.144 Mobile Safari/537.36");
        assert_eq!(a["device"], json!({ "type": "mobile", "vendor": null, "model": "Pixel 8" }));
        let sm = go("Mozilla/5.0 (Linux; Android 13; SM-X710 Build/TP1A.220624.014) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/23.0 Chrome/115.0.0.0 Safari/537.36");
        assert_eq!(sm["device"]["type"], json!("tablet"));
        assert_eq!(sm["device"]["model"], json!("SM-X710"));
        assert_eq!(sm["browser"]["name"], json!("Samsung Internet"));
    }

    #[test]
    fn bots_libraries_and_unknown() {
        let g = go("Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)");
        assert_eq!(g["is_bot"], json!(true));
        assert_eq!(g["browser"], json!({ "name": "Googlebot", "version": "2.1", "major": "2" }));
        assert_eq!(go("curl/8.4.0")["device"]["type"], json!("library"));
        assert_eq!(go("python-requests/2.31.0")["browser"]["name"], json!("python-requests"));
        assert_eq!(go("something else entirely")["device"]["type"], json!("unknown"));
        assert_eq!(run(br#"{"ua": 1}"#)["ok"], json!(false));
    }
}
