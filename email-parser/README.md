# email-parser

Parses a raw email (RFC 5322 with MIME) into fields a flow can route on: sender, recipients, subject, date, thread IDs, the plain-text body and the attachments. It also separates what the sender just wrote from the quoted history and the signature, which is usually the only part a support desk, order inbox or AI step needs.

## Input

One record or an array of records, each with the raw message (the `.eml` content, or the "raw" or "MIME" format most mail APIs offer):

```json
{ "email": "From: \"Doe, Jane\" <jane.doe@example.com>\r\nTo: support@example.com\r\nSubject: ...\r\n\r\n..." }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the message | `email` |
| `base64` | The message is base64-encoded (as some APIs return it) | `false` |
| `include_html` | Also return the HTML body as `body_html` | `false` |
| `include_attachments` | Include each attachment's content as `content_base64` | `false` |
| `keep_source` | Keep the raw message in the output | `false` |

## Output

```json
{
  "ok": true,
  "from": { "name": "Doe, Jane", "address": "jane.doe@example.com" },
  "to": [{ "name": null, "address": "support@example.com" }],
  "cc": [],
  "reply_to": null,
  "subject": "Re: Order 45-1180 – damaged",
  "date": "2026-11-09T10:15:00-05:00",
  "date_raw": "Mon, 9 Nov 2026 10:15:00 -0500",
  "message_id": "abc123@mail.example.com",
  "in_reply_to": null,
  "references": [],
  "body_text": "Hi, the box arrived crushed – photos attached. ...",
  "reply_text": "Hi, the box arrived crushed – photos attached.\nCan you send a replacement for PO 45-1180?",
  "quoted_text": "On Fri, Nov 6, 2026 at 9:00 AM Support <support@example.com> wrote:\n> Your order shipped today.",
  "signature": "Jane Doe | Acme",
  "attachments": [{ "filename": "box.jpg", "content_type": "image/jpeg", "size": 6 }]
}
```

- Addresses are lowercased; display names are decoded, including non-English names.
- `body_text` is the plain-text part, or text made from the HTML part when there is no plain-text part.
- `reply_text` cuts at the first "On ... wrote:", "-----Original Message-----", an Outlook-style "From: / Sent:" block or quoted `>` lines, and removes the signature (after `-- ` or a "Sent from my ..." line).
- `message_id`, `in_reply_to` and `references` let a flow attach a reply to the right ticket.

Supported encodings: base64, quoted-printable, 7bit and 8bit; UTF-8, ISO-8859-1 and Windows-1252. Other character sets are read as UTF-8. Nested messages (a forwarded email inside an email) are read in place. Reply detection covers the common English formats; replies written in other languages may keep their quoted part.

## Use it in a Neblex flow

1. Download `email-parser.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `email-parser`.
3. Add a step that uses the module's **Run module** operation after a mail step that returns the raw message (.eml, raw or MIME format). Map it to `email`.

## Build and test

```sh
../build.sh email-parser
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
