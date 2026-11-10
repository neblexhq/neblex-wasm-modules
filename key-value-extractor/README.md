# key-value-extractor

Pulls labeled values and simple tables out of document text: invoices, packing slips, order confirmations, remittance advice and statements, after `pdf-to-json` or `email-parser` has turned them into text. You list the fields you want and the labels they appear under; the module finds them, reads them as text, numbers, amounts or dates, and reports where each one came from. Routine documents need no AI step, and anything it can't find is listed for review.

## Input

One record or an array of records, each with the text and the fields to find:

```json
{
  "text": "Invoice No: INV-2026-0091            Invoice Date: Nov 3, 2026\nPO Number: 45-1180 ...",
  "_options": {
    "fields": {
      "invoice_number": ["invoice no", "invoice number"],
      "invoice_date": { "labels": ["invoice date"], "type": "date" },
      "due_date": { "labels": ["due date"], "type": "date" },
      "po_number": ["po number", "purchase order"],
      "bill_to": ["bill to"],
      "total": { "labels": ["total due", "amount due", "total"], "type": "amount" }
    },
    "date_order": "dmy",
    "tables": true
  }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the text | `text` |
| `fields` | Output name → a list of labels, or `{ "labels": [...], "type": "text" \| "number" \| "amount" \| "date" }` | |
| `date_order` | `"mdy"` or `"dmy"` for numeric dates such as 03/12/2026 | unset: only unambiguous numeric dates are read |
| `tables` | Also return aligned tables | `false` |
| `keep_source` | Keep the text in the output | `false` |

How values are found:

- Labels are matched case-insensitively on word boundaries, so `total` doesn't match inside "Subtotal" and `po` doesn't match inside "report". Longer labels are tried first.
- The value is what follows the label on the same line, after separators such as `:`, `#`, `No.` or `Number`, up to a gap of two or more spaces (the next column in a two-column layout). When nothing follows the label, the next non-empty line is used ("Bill To:" then the name below it).

## Output

```json
{
  "fields": {
    "invoice_number": "INV-2026-0091",
    "invoice_date": "2026-11-03",
    "due_date": "2026-12-03",
    "po_number": "45-1180",
    "bill_to": "Globex Corporation",
    "total": { "amount": 1250.0, "currency": "USD" }
  },
  "found": { "total": { "label": "total due", "line": 12, "raw": "USD 1,250.00" } },
  "missing": [],
  "tables": [
    { "line": 7, "columns": ["Item", "Description", "Qty", "Unit Price", "Amount"],
      "rows": [{ "Item": "WID-9", "Description": "Widget, blue", "Qty": "2", "Unit Price": "500.00", "Amount": "1,000.00" }] }
  ],
  "ok": true
}
```

- `missing` lists fields that weren't found or couldn't be read as their type. Route documents with missing fields to an AI step or a person.
- `found` shows the label, line number and raw text behind each value, for review screens and audit.
- Tables are a header line followed by at least two rows with the same columns, separated by two or more spaces or tabs. "Label: value" lines are not treated as tables. Table cells stay text.

Layouts with values above their labels, or tables without clear column gaps, need an AI step.

## Use it in a Neblex flow

1. Download `key-value-extractor.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `key-value-extractor`.
3. Add a step that uses the module's **Run module** operation after a PDF, OCR or email step that returns document text. Map it to `text`.

## Build and test

```sh
../build.sh key-value-extractor
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input (a text invoice) for trying the module in a flow or with `../test/call.py`.
