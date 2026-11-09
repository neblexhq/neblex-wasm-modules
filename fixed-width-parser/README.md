# fixed-width-parser

Turns fixed-width (positional) text files into records: mainframe extracts, bank and payment files, legacy ERP exports and other formats where each field sits at a set column position instead of between commas. It handles files that mix header, detail and trailer lines, implied decimals and COBOL signed numbers.

## Input

One record or an array of records. Each record holds the file text and a `_layout` that describes it:

```json
{
  "file": "00001ACME CORP     0001250\n00002GLOBEX INC    0009900\n",
  "_layout": {
    "columns": [
      { "name": "id", "start": 1, "length": 5, "type": "integer" },
      { "name": "name", "start": 6, "length": 14 },
      { "name": "amount", "start": 20, "length": 7, "type": "decimal", "scale": 2 }
    ]
  }
}
```

`start` is the 1-based character position, as most file specs print it. In a flow, add the `_layout` with a Set Fields step before the module.

| `_layout` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the file text | `file` |
| `columns` | The columns, for a file with one kind of line | |
| `record_types` | For files that mix line types (see below); use instead of `columns` | |
| `skip_lines` | Lines to skip at the top, such as a column ruler | `0` |
| `keep_source` | Keep the file text in the output | `false` |

### Column types

| `type` | Reads | Example |
|---|---|---|
| `text` (default) | Text, trimmed. Set `"trim": false` to keep the padding. | `ACME CORP     ` → `"ACME CORP"` |
| `integer` | Whole number, leading zeros allowed | `00042` → `42` |
| `decimal` | Number with a point, or an implied decimal set by `scale`. A leading or trailing sign is accepted. | `0001250`, scale 2 → `12.5`; `1250-`, scale 2 → `-12.5` |
| `zoned` | COBOL zoned decimal (signed overpunch): the last character carries the sign | `000012{`, scale 2 → `1.2`; `000001J`, scale 2 → `-0.11` |
| `date` | `YYYYMMDD` or `YYMMDD` to an ISO date. All zeros becomes null. Two-digit years 00-69 are 20xx. | `20261106` → `"2026-11-06"` |

Blank numeric and date fields become null.

### Files with header, detail and trailer lines

List the line types in `record_types`. Each line is matched against `when` (the text found at a position) in order. A type without `when` matches any line, so put it last as a catch-all.

```json
"_layout": {
  "record_types": [
    { "name": "header", "when": { "start": 1, "equals": "H" }, "columns": [
      { "name": "file_date", "start": 2, "length": 8, "type": "date" } ] },
    { "name": "detail", "when": { "start": 1, "equals": "D" }, "columns": [
      { "name": "seq", "start": 2, "length": 4, "type": "integer" },
      { "name": "amount", "start": 20, "length": 7, "type": "zoned", "scale": 2 } ] },
    { "name": "trailer", "when": { "start": 1, "equals": "T" }, "columns": [
      { "name": "detail_count", "start": 2, "length": 2, "type": "integer" } ] }
  ]
}
```

## Output

The record comes back without `_layout` (and without the file text unless `keep_source` is set), with these fields added:

```json
{
  "records": [
    { "_line": 1, "_record_type": "header", "file_date": "2026-11-06" },
    { "_line": 2, "_record_type": "detail", "seq": 1, "amount": 1.2 }
  ],
  "line_count": 2,
  "errors": [],
  "_valid": true
}
```

- `records` has one entry per non-blank line, with `_line` (the line number in the file) and `_record_type` when `record_types` is used. Add an Unwind step on `records` to get one flow record per line, then a Filter on `_record_type` to route headers, details and trailers.
- `errors` lists each problem with its line and column: a value that doesn't parse, a line that ends early, or a line no record type matches. The value is null and the rest of the line is still read.
- `_valid` is false when there is any error. Route those files to a review step instead of loading part of a file.

A bad `_layout` (missing `start`, unknown type) returns `"ok": false` with the reason instead.

The module reads text. A file still in EBCDIC needs converting to UTF-8 or ASCII first, which most transfer tools do on download.

## Build and test

```sh
../build.sh fixed-width-parser
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
