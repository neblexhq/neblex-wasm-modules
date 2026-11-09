# pdf-to-json

Extracts the text and metadata from a PDF so the rest of the flow can work with it as JSON: read an invoice total, route a document by its title, or pass the text to an AI step.

## Input

Any JSON that contains the PDF as base64. The module finds the first string that starts with `JVBERi` (base64 for `%PDF`) or `data:application/pdf;base64,`, wherever it sits in the input.

```json
{ "order_id": 7, "attachment": { "content": "JVBERi0xLjQK..." } }
```

## Output

```json
{
  "ok": true,
  "source_field": "attachment.content",
  "page_count": 1,
  "metadata": { "title": "Invoice INV-1001", "author": "Acme Corp" },
  "has_text": true,
  "text": "Invoice INV-1001\nBill to: Globex\nTotal: 1,250.00 USD",
  "pages": [{ "page": 1, "text": "Invoice INV-1001\nBill to: Globex\nTotal: 1,250.00 USD" }]
}
```

On failure it returns `{"ok": false, "error": "..."}`.

## Limits

This is a small, text-only reader. It handles uncompressed and Flate-compressed content streams. It does not decrypt password-protected PDFs, read images or run OCR, so a scanned PDF returns `has_text: false`. Use that field to send scans to an OCR or AI step instead.

## Build and test

```sh
../build.sh pdf-to-json
docker run --rm -v "$(cd .. && pwd)":/w -w /w python:3.12-slim sh -c \
  'pip install -q wasmtime && python pdf-to-json/test/run.py dist/pdf-to-json.wasm'
```
