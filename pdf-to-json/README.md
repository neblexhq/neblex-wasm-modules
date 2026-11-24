# pdf-to-json

Extracts the text and metadata from a PDF so the rest of the flow can work with it as JSON: read an invoice total, route a document by its title, or pass the text to an AI step.

Neblex also has a built-in ReadDocument step that reads PDF, Word, PowerPoint and Excel files; it is the first choice for most documents. Use this module as a starting point when you need to change how PDF text is extracted.

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

This is a small, text-only reader. It handles uncompressed content streams and the Flate, ASCII85 and ASCIIHex filters, including ASCII85 over Flate as ReportLab writes it. Wide gaps between text on one line, such as the two columns of an invoice header or the cells of a table, come out as two spaces, so `key-value-extractor` can tell the columns apart. It opens encrypted PDFs that need no password to open (RC4 and AES, as banks and government forms often send them); a PDF that needs a password returns an error saying so. It does not read images or run OCR, so a scanned PDF returns `has_text: false`. Use that field to send scans to an OCR or AI step instead.

## Use it in a Neblex flow

1. Download `pdf-to-json.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `pdf-to-json`.
3. Add a step that uses the module's **Run module** operation after a mail, storage or API step that returns a PDF as base64. Map the record as is (the PDF is found wherever it sits).

## Build and test

```sh
../build.sh pdf-to-json
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

The unit tests build small PDFs in code and also read the files in `test/fixtures/`: encrypted copies of one invoice (RC4 and AES, made with qpdf) and an object-stream PDF. `test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
