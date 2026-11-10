# csv-writer

Writes records as CSV text with the layout the receiving system expects: any delimiter, decimal commas for European spreadsheets, chosen columns and headers, nested fields, a byte-order mark so Excel reads accents correctly, and Windows or Unix line endings. Text that a spreadsheet would run as a formula is neutralized by default.

## Input

One record holding the rows to write:

```json
{
  "records": [
    { "sku": "A-1", "name": "Widget, blue", "price": 9.5, "customer": { "name": "Müller GmbH" } },
    { "sku": "B-2", "name": "=cmd|' /C calc'!A0", "price": 12, "customer": { "name": "Globex" } }
  ],
  "_options": {
    "delimiter": ";", "decimal": ",", "bom": true,
    "columns": [ { "field": "sku", "header": "Artikel" }, { "field": "name", "header": "Bezeichnung" }, { "field": "customer.name", "header": "Kunde" }, { "field": "price", "header": "Preis" } ]
  }
}
```

To write a file from a list of flow records, collect them first (for example with an Aggregate step) so the module gets one record holding the list.

| `_options` key | Meaning | Default |
|---|---|---|
| `records_field` | Which field holds the rows | `records` |
| `into` | Where to write the CSV text | `csv` |
| `columns` | Field names, or `{ "field": "customer.name", "header": "Kunde" }` objects, in the order to write them; dotted paths read nested fields | every top-level field, alphabetically |
| `delimiter` | `,`, `;`, `tab`, `\|` or any single character | `,` |
| `decimal` | `.` or `,` for numbers (a decimal comma needs another delimiter) | `.` |
| `quote` | `minimal` (only when needed) or `all` | `minimal` |
| `header` | Write a header row | `true` |
| `bom` | Start with a UTF-8 byte-order mark (helps Excel open accented text) | `false` |
| `line_ending` | `"\r\n"` or `"\n"` | `"\r\n"` |
| `null_value` | Text for missing and null values | `""` |
| `escape_formulas` | Prefix text starting with `=`, `+`, `-` or `@` with `'` so spreadsheets don't run it | `true` |
| `keep_source` | Keep the rows in the output | `false` |

## Output

```json
{
  "csv": "﻿Artikel;Bezeichnung;Kunde;Preis\r\nA-1;Widget, blue;Müller GmbH;9,5\r\nB-2;'=cmd|' /C calc'!A0;Globex;12\r\n",
  "row_count": 2,
  "columns": ["Artikel", "Bezeichnung", "Kunde", "Preis"],
  "ok": true
}
```

- Values with the delimiter, quotes, line breaks or edge spaces are quoted, and quotes are doubled (RFC 4180).
- Numbers keep full precision; text that looks like a number (`"-42"`) isn't treated as a formula. Nested objects and lists are written as JSON text.
- Formula escaping follows the OWASP advice for CSV injection. Turn it off only for trusted data going to a system that isn't a spreadsheet.

## Use it in a Neblex flow

1. Download `csv-writer.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `csv-writer`.
3. Add a step that uses the module's **Run module** operation after a step that returns the records to export. Map the list to `records`, and set `columns` to fix the column order.

## Build and test

```sh
../build.sh csv-writer
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` writes a European-style file with renamed columns, a nested field and a formula that gets neutralized.
