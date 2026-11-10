# fixed-width-writer

Writes records as a fixed-width (positional) text file: bank and payment uploads, legacy ERP imports, mainframe feeds and other formats where each field sits at a set column position. It uses the same `_layout` as [fixed-width-parser](../fixed-width-parser), so one layout describes a format in both directions.

## Input

One record or an array of records, each with the rows to write and a `_layout`:

```json
{
  "records": [
    { "id": 1, "name": "ACME CORP", "amount": 12.5 },
    { "id": 2, "name": "GLOBEX INC", "amount": 99 }
  ],
  "_layout": {
    "columns": [
      { "name": "id", "start": 1, "length": 5, "type": "integer" },
      { "name": "name", "start": 6, "length": 14 },
      { "name": "amount", "start": 20, "length": 7, "type": "decimal", "scale": 2 }
    ]
  }
}
```

gives

```
00001ACME CORP     0001250
00002GLOBEX INC    0009900
```

To get `records` from a list of flow records, collect them first (for example with an Aggregate step) so the module gets one record holding the array.

| `_layout` key | Meaning | Default |
|---|---|---|
| `columns` or `record_types` | As in fixed-width-parser. With `record_types`, each row picks its type by its `_record_type` field (the parser sets it), and the type's `when.equals` text is written at `when.start`. | |
| `records_field` | Which field holds the rows | `records` |
| `into` | Where to write the file text | `file` |
| `line_ending` | `"\n"` or `"\r\n"` | `"\n"` |
| `final_newline` | End the file with a line ending | `true` |
| `line_length` | Pad every line to this width | the end of the last column |
| `truncate` | Cut text that is too long instead of reporting an error | `false` |

### Column options

| Key | Meaning | Default |
|---|---|---|
| `type` | `text`, `integer`, `decimal`, `zoned` (COBOL signed overpunch) or `date` | `text` |
| `scale` | Implied decimal places for `decimal` and `zoned`: 12.5 with scale 2 is written `1250` | `0` |
| `format` | Date layout: `yyyymmdd`, `yymmdd`, `mmddyyyy`, `mmddyy` or `ddmmyyyy`. Input dates are ISO (`2026-11-06`). | `yyyymmdd` |
| `align` | `left` or `right` | right for numbers, left for text |
| `pad` | Fill character | `0` for numbers, space for text |
| `value` | A constant written on every line (filler, record codes) | |

Negative decimals keep their sign in front of the zero padding (`-0042`). Missing values become blanks for text and dates, and zeros for numbers; set `"pad": "0"` on a date to write `00000000`.

## Output

```json
{ "file": "00001ACME CORP     0001250\n00002GLOBEX INC    0009900\n", "line_count": 2, "errors": [], "_valid": true, "ok": true }
```

`errors` lists every value that didn't fit or didn't parse, with its record index and column; that field is left blank, and `_valid` is false. Don't send a file with errors to a bank or ERP. Route it to review instead.

## Use it in a Neblex flow

1. Download `fixed-width-writer.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `fixed-width-writer`.
3. Add a step that uses the module's **Run module** operation after a step that returns the records to write. Map the list to `records` and add the same `_layout` the parser uses.

## Build and test

```sh
../build.sh fixed-width-writer
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` writes the same file that fixed-width-parser's sample reads.
