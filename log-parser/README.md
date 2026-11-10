# log-parser

Turns log text into records: web server access logs (Apache and nginx), syslog from servers and network devices (RFC 3164 and RFC 5424), key=value application logs (logfmt) and JSON lines. Each line's format is detected on its own, so mixed files work. Use it to route errors to a ticket, alert on failed logins, or load logs into a database without a separate log pipeline.

## Input

One record or an array of records, each with log text (one entry per line):

```json
{ "text": "203.0.113.9 - jane [09/Nov/2026:10:15:00 -0500] \"GET /api/orders?id=7 HTTP/1.1\" 200 512 ...\n<34>1 2026-11-09T15:15:02.003Z web01 sshd 4321 AUTH - Failed password ...", "_options": { "year": 2026 } }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the log text | `text` |
| `format` | Only accept one format: `access`, `syslog`, `logfmt` or `json` | all |
| `year` | The year for RFC 3164 syslog lines, which don't include one; with it, their time becomes ISO | none: the time is kept as written |
| `keep_source` | Keep the log text in the output | `false` |

## Output

```json
{
  "record_count": 5,
  "formats": { "access": 1, "syslog": 2, "logfmt": 1, "json": 1 },
  "records": [
    { "_format": "access", "_line": 1, "ip": "203.0.113.9", "user": "jane", "time": "2026-11-09T10:15:00-05:00", "method": "GET",
      "path": "/api/orders", "query": "id=7", "protocol": "HTTP/1.1", "status": 200, "bytes": 512, "referrer": "https://example.com/", "user_agent": "curl/8.4.0" },
    { "_format": "syslog", "_line": 2, "facility": 4, "severity": "critical", "time": "2026-11-09T15:15:02.003Z", "host": "web01",
      "app": "sshd", "pid": "4321", "msgid": "AUTH", "message": "Failed password for root from 192.0.2.7" },
    { "_format": "logfmt", "_line": 4, "level": "error", "msg": "order save failed", "order_id": 7, "took": 12.5, "retry": true }
  ],
  "errors": [],
  "ok": true
}
```

- Access logs: common and combined formats. `-` values become null; the time becomes ISO with its offset.
- Syslog: the `<priority>` is split into `facility` (number) and `severity` (name). RFC 5424 structured data is skipped; the message after it is kept.
- logfmt: quoted values may contain spaces; numbers and `true`/`false` become typed values. A line needs at least two `key=value` pairs, so prose with one `=` isn't mistaken for logfmt.
- JSON lines are kept as they are, with `_format` and `_line` added.
- `errors` lists lines that match no format, with their line number and first 200 characters.

## Use it in a Neblex flow

1. Download `log-parser.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `log-parser`.
3. Add a step that uses the module's **Run module** operation after a storage, SFTP or webhook step that returns log lines. Map them to `text`.

## Build and test

```sh
../build.sh log-parser
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` has one line of each format for trying the module in a flow or with `../test/call.py`.
