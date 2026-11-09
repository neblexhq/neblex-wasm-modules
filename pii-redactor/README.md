# pii-redactor

Removes personal data from free text, such as support emails, ticket descriptions, chat messages or call notes, before the text goes to an AI model, a log, an analytics tool or a third-party system. It finds email addresses, phone numbers, payment card numbers, US Social Security numbers, IBANs, IP addresses and any names or terms you list, and replaces them in place.

## Input

One record or an array of records:

```json
{
  "text": "Hi, I'm Jane Doe (jane.doe@example.com, +1 415-555-0132). Card 4111 1111 1111 1111 was charged twice.",
  "_options": { "terms": ["Jane Doe"] }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` or `fields` | The text field, or a list of fields, to redact in place | `text` |
| `types` | Which value types to redact (see below); `[]` with `terms` redacts only your terms | all |
| `mode` | `placeholder` (`[EMAIL_1]`), `mask` (`j***@example.com`, `****1111`) or `remove` (`[REDACTED]`) | `placeholder` |
| `terms` | Names or words to redact: a list (shown as `[NAME_n]`), or `{ "customer": ["Acme Corp"], "project": ["Falcon"] }` for your own labels | |
| `return_map` | Also return `redaction_map`, from each placeholder to the original value | `false` |

## Value types

| Type | Finds |
|---|---|
| `email` | Email addresses |
| `phone` | 10 to 15 digits with separators or a leading `+`. A bare run of digits is left alone, so order and invoice numbers aren't hidden. |
| `card` | 13 to 19 digits that pass the Luhn check, with or without spaces or dashes |
| `ssn` | US Social Security numbers written `123-45-6789`, excluding invalid ranges (000, 666, 9xx, zero group or serial) |
| `iban` | IBANs whose check digits are right, with or without spaces |
| `ip` | IPv4 addresses |

## Output

```json
{
  "text": "Hi, I'm [NAME_1] ([EMAIL_1], [PHONE_1]). Card [CARD_1] was charged twice.",
  "redaction_count": 4,
  "redactions": [
    { "type": "name", "field": "text", "placeholder": "[NAME_1]", "start": 8, "end": 16 },
    { "type": "email", "field": "text", "placeholder": "[EMAIL_1]", "start": 18, "end": 38 }
  ],
  "ok": true
}
```

The same value gets the same placeholder everywhere in the record, across all fields, so an AI model can still tell that two mentions are the same person or card. With `return_map`, a later step can put the original values back into the model's answer. Keep the map in the flow (for example in a Set Value step) and never send it to the model.

## Limits

The module finds data by pattern and check digit. It won't find a person's name unless you list it in `terms`, addresses written in prose, dates of birth, or numbers in unusual formats. Treat it as one layer of protection, not a guarantee that text is free of personal data.

## Build and test

```sh
../build.sh pii-redactor
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
