# csv-cleaner

Reads the CSV files real systems produce into clean records: semicolon- or tab-separated exports, files that start with a title or a "generated on" line, a totals row at the bottom, European decimal commas, a byte-order mark, quoted fields with line breaks, blank or duplicate headers, and rows with too few or too many fields. It works out the delimiter, header row and number style on its own and reports everything it skipped or couldn't read.

## Input

One record or an array of records, each with the file text:

```json
{
  "csv": "Kundenexport\nErstellt am 09.11.2026\n\nKundennr;Name;PLZ;Betrag\n1001;Müller GmbH;01067;1.234,56\n1002;\"Schmidt; Söhne\";80331;99,90\nSumme;;;1.334,46\n",
  "_options": { "numbers": true, "snake_case_headers": true }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the CSV text | `csv` |
| `delimiter` | `,`, `;`, `tab` or `\|` | detected |
| `header_row` | The line number of the header, when detection picks the wrong one | detected |
| `header` | `false` when the file has no header; columns are named `column_1`, `column_2`... | `true` |
| `numbers` | Turn numeric columns into numbers | `false` |
| `decimal` | `.` or `,` | detected |
| `snake_case_headers` | `Order ID` → `order_id` | `false` |
| `drop_totals` | Skip "Total", "Sum", "Summe", "Gesamt" rows at the end | `true` |
| `keep_source` | Keep the CSV text in the output | `false` |

How detection works:

- **Delimiter:** the one of `,` `;` tab `|` that splits the most lines into the same number of fields.
- **Header:** the first line as wide as most lines. Title lines and notes before it are skipped.
- **Decimal style:** values like `1.234,56` or `99,90` mean a decimal comma; `1,234.56` or `12.50` mean a decimal point.
- **Numbers:** with `numbers`, a column becomes numbers only when every value in it is one. A column with any leading-zero value (postal codes, IDs, SKUs) stays text throughout, and so does a column with any text in it.

## Output

```json
{
  "columns": ["kundennr", "name", "plz", "betrag"],
  "delimiter": ";",
  "decimal": ",",
  "record_count": 2,
  "records": [
    { "_line": 5, "kundennr": 1001, "name": "Müller GmbH", "plz": "01067", "betrag": 1234.56 },
    { "_line": 6, "kundennr": 1002, "name": "Schmidt; Söhne", "plz": "80331", "betrag": 99.9 }
  ],
  "skipped": [
    { "line": 1, "reason": "before the header", "text": "Kundenexport" },
    { "line": 2, "reason": "before the header", "text": "Erstellt am 09.11.2026" },
    { "line": 7, "reason": "totals row", "text": "Summe;;;1.334,46" }
  ],
  "errors": [],
  "_valid": true,
  "ok": true
}
```

- `_line` is the record's line number in the file, for tracing a value back to its source.
- Empty cells become null.
- `errors` lists rows with the wrong number of fields. Missing cells become null and extra cells are dropped, so check `_valid` before loading a file into a system of record.

Add an Unwind step on `records` to get one flow record per row. The text must already be decoded: files in a legacy encoding such as Windows-1252 should be converted to UTF-8 first, which most file connectors do on read.

## Build and test

```sh
../build.sh csv-cleaner
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
