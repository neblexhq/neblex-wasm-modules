# text-extractor

Pulls structured values out of free text, such as an email body, a support ticket, a chat message or text extracted from a PDF, without an AI model. It finds emails, URLs, phone numbers, dates, amounts, IBANs, parcel tracking numbers and labeled references like PO and invoice numbers, and validates check digits where the format has them. It runs in milliseconds at no per-record cost, so you can run it on everything and send only what it can't resolve to an AI step or a person.

## Input

One record or an array of records, each with the text:

```json
{ "text": "PO# 45-1180 arrived damaged. Invoice No: INV-2026-0091 for $1,250.00 ..." }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the text | `text` |
| `types` | Which value types to look for (see below) | all |
| `date_order` | `mdy` or `dmy`, to resolve dates like 04/03/2026 | unset: ambiguous dates are flagged |
| `references` | Your own labeled references (see below); replaces the default invoice, PO and order number labels | |
| `context` | Characters of surrounding text to return with each match | `30` |

## Value types

| Type | Finds | Normalized value |
|---|---|---|
| `email` | Addresses with a real-looking domain | lowercased |
| `url` | `http://`, `https://` and `www.` links, without trailing punctuation | `www.` links get `https://` |
| `phone` | 10 to 15 digits with common separators | E.164 (`+14155550132`) when it starts with `+` or fits the North American plan; other numbers are skipped |
| `date` | `2026-03-05`, `March 5, 2026`, `5 Mar 2026`, `25/03/2026` | ISO date. Numeric dates where both orders are valid (04/03/2026) return null with `"ambiguous": true` unless `date_order` is set |
| `amount` | A number with a currency symbol or code before or after it: `$1,250.00`, `€99,90`, `1.234,56 EUR` | number, plus `currency` |
| `iban` | IBANs, with or without spaces | compact form, plus `country`; only kept when the MOD 97 check digits are right |
| `tracking` | UPS (1Z...) and USPS (20 or 22 digits) | compact form, plus `carrier`; only kept when the check digit is right |
| `reference` | A value that follows a label: `PO# 45-1180`, `Invoice No: INV-2026-0091`, `Order number 7781` | the value, plus `name` (`po_number`, `invoice_number`, `order_number`) |

### Your own references

```json
"_options": {
  "references": {
    "case_id": { "labels": ["case", "case id"] },
    "ticket": { "labels": ["ticket"], "digits_only": true, "min": 4, "max": 10 }
  }
}
```

Labels are matched case-insensitively and may be followed by `#`, `:`, `No` or `Number`. A value must contain at least one digit.

## Output

```json
{
  "found": {
    "reference": ["45-1180", "INV-2026-0091"],
    "amount": [1250.0],
    "date": ["2026-03-05"],
    "phone": ["+14155550132"],
    "email": ["jane.doe@example.com"]
  },
  "entities": [
    { "type": "reference", "name": "po_number", "text": "45-1180", "value": "45-1180", "start": 13, "end": 20, "context": "Hi team, PO# 45-1180 arrived damaged. Invoice No:" }
  ],
  "ok": true
}
```

- `found` has the unique values per type, which is what most flows need.
- `entities` lists every match in order, with character positions and the text around it, for review screens or for an AI step that should only check what was found.

## Limits

The module works by patterns and check digits. It handles predictable formats well, and it will miss values written in unusual ways (an amount with no currency, a phone number in words) and can't tell which of two dates is the due date. Use the context, or pass the text and the module's findings to an AI step when that matters.

## Build and test

```sh
../build.sh text-extractor
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
