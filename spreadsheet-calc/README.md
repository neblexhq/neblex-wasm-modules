# spreadsheet-calc

Runs a spreadsheet as a flow step. Upload the workbook your team already uses for pricing, commissions, freight or quotes; the module puts each record's values into its input cells, recalculates every formula and returns the cells you name. The spreadsheet stays the source of truth for the math, and nobody has to rewrite it as code.

It reads .xlsx files (Excel 2007 and later) and supports the common spreadsheet functions, including lookups (VLOOKUP, XLOOKUP, INDEX/MATCH, LOOKUP), logic (IF, IFS, IFERROR, AND, OR), math and rounding, text (TEXT, CONCAT, LEFT...), dates (DATE, EDATE, TODAY...) and statistics. You can also define small sheets directly in JSON. Calculation uses [IronCalc](https://github.com/ironcalc/ironcalc), an open-source spreadsheet engine.

## Input

One record or an array of records:

```json
{
  "customer": { "tier": "gold" },
  "qty": 600,
  "list_price": 12,
  "ship_to": { "state": "TX" },
  "_options": {
    "workbook": "UEsDBBQAAAAIAA...",
    "now": "2026-11-24",
    "inputs": { "Quote!B3": "customer.tier", "Quantity": "qty", "Quote!B5": "list_price", "Quote!B6": "ship_to.state" },
    "outputs": {
      "net_unit_price": "Quote!B10",
      "tax": "Quote!B13",
      "total": "Total",
      "needs_approval": "Quote!B15",
      "valid_until": { "cell": "Quote!B16", "formatted": true }
    }
  }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `workbook` | The .xlsx file as base64 (a data URL also works) | one of `workbook` or `sheets` |
| `sheets` | Sheets defined in JSON instead: `{ "Pay": { "B1": 0, "B3": "=ROUND(B1*B2,2)" } }`. Values are typed as a person would type them, so `=` starts a formula | |
| `inputs` | `{ cell: record field }`. Sets each cell from a dotted field of the record. See below for lists and fixed values | |
| `outputs` | `{ result field: cell }`, or `{ "cell": ..., "formatted": true }` for the value as the sheet displays it (`2026-12-24`, `$1,250.00`) | required |
| `into` | Field for the results | `result` |
| `now` | The time TODAY() and NOW() return: an ISO date or time, or Unix milliseconds. The module has no clock, so pass the run's time when the workbook uses them | 1970-01-01 |
| `timezone` | Time zone for date functions, such as `America/Chicago` | `UTC` |

**Cells** are written `B4` (first sheet), `Quote!B4`, `'Price List'!B4`, a range such as `Lines!A2:D21`, or a name defined in the workbook (`Total`).

**Input values** keep their type: numbers stay numbers, `true`/`false` become booleans, `2026-11-24` becomes a spreadsheet date, and other text stays text. Text from a record is never run as a formula. A field missing from the record leaves the workbook's own value in place and is listed in `missing_inputs`. Other input forms:

| Input | Writes |
|---|---|
| `"Lines!A2": "skus"` with a list of values | the values down the column |
| `"Lines!A2": "rows"` with a list of lists | one row per inner list |
| `"Lines!A2": { "field": "lines", "columns": ["sku", "qty", "price"] }` | a list of objects, one row each, columns in that order |
| `"Quote!B7": { "value": 0.07 }` | a fixed value |

## Output

The record comes back with these fields added (output of the input above, using [test/fixtures/quote-calculator.xlsx](test/fixtures/quote-calculator.xlsx)):

```json
{
  "result": {
    "net_unit_price": 10.26,
    "tax": 507.87,
    "total": 6663.87,
    "needs_approval": false,
    "valid_until": "2026-12-24"
  },
  "cell_errors": {},
  "ok": true
}
```

- Numbers come back rounded to 12 significant digits, as a spreadsheet shows them; dates come back as spreadsheet date numbers unless you ask for `formatted`.
- A range comes back as a list of rows, without trailing empty rows.
- A cell showing an error (`#DIV/0!`, `#N/A`, or `#NAME?` for a function the engine doesn't support) comes back as null, with the error in `cell_errors`. Check `cell_errors` before trusting a result.

## Things to know

- Each record is calculated on a fresh copy of the workbook, so one record's inputs never leak into the next. The file is read once per batch.
- External links, macros, pivot tables and data connections aren't run. RAND() repeats the same numbers on every run.
- Test a workbook with a few known records before relying on it: compare the module's results with the same inputs typed into Excel.

## Use it in a Neblex flow

1. Download `spreadsheet-calc.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `spreadsheet-calc`.
3. Get the workbook into the flow as base64, for example from a file step, a file share or a fixed value in the step, and map it to `_options.workbook`.
4. Add a step that uses the module's **Run module** operation, with the record fields, `inputs` and `outputs` mapped.

## Build and test

```sh
../build.sh spreadsheet-calc
docker run --rm -v "$PWD/..":/repo -w /repo/spreadsheet-calc rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input (with `test/fixtures/quote-calculator.xlsx` inside it) for trying the module in a flow or with `../test/call.py`.
