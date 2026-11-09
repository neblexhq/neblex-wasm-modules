# user-agent-parser

Reads the browser, operating system, device type and rendering engine from a user-agent string, as found in web server logs, form submissions, analytics exports and marketing events. Bots and HTTP libraries are flagged separately, so they can be filtered out before counting visitors or scoring leads.

## Input

One record or an array of records, each with a user-agent string:

```json
{ "user_agent": "Mozilla/5.0 (iPhone; CPU iPhone OS 17_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.1 Mobile/15E148 Safari/604.1" }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the user agent | `user_agent` |

## Output

The record comes back with these fields added:

```json
{
  "browser": { "name": "Safari", "version": "17.1", "major": "17" },
  "os": { "name": "iOS", "version": "17.1", "major": "17" },
  "device": { "type": "mobile", "vendor": "Apple", "model": "iPhone" },
  "engine": "WebKit",
  "is_bot": false,
  "ok": true
}
```

| Field | Values |
|---|---|
| `device.type` | `desktop`, `mobile`, `tablet`, `bot`, `library` (curl, python-requests, Postman and other HTTP clients) or `unknown` |
| `browser.name` | Chrome, Edge, Firefox, Safari, Opera, Samsung Internet, Chromium, Internet Explorer; for bots and libraries, their own name (Googlebot, curl ...) |
| `os.name` | Windows, macOS, iOS, iPadOS, Android, ChromeOS, Linux |
| `engine` | Blink, WebKit, Gecko or Trident |
| `device.model` | iPhone, iPad, or the Android model when the user agent includes it (Pixel 8, SM-X710) |

Windows 10 and 11 send the same version ("10"), and recent browsers reduce the detail in their user agents, so treat versions as approximate. For exact device data, use client hints.

## Build and test

```sh
../build.sh user-agent-parser
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` has a desktop browser, a phone and a bot for trying the module in a flow or with `../test/call.py`.
