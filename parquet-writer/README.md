# parquet-writer

Writes records to an Apache Parquet file, so data from any flow can be loaded into a data warehouse, a data lake or an analytics tool. Snowflake, BigQuery, Databricks, Spark, DuckDB, Polars and pandas all read Parquet directly, and it is usually much smaller than the same data as CSV or JSON.

Column types are inferred from the values or set per column: text, whole numbers, decimals with exact precision, dates, timestamps with or without a time zone, true/false and JSON. Files can be compressed with Snappy, Gzip, Zstandard, LZ4 or Brotli.

The output pairs with [parquet-reader](../parquet-reader), which reads it back.

## Input

One record holding the rows to write:

```json
{
  "export_id": "orders-2026-11",
  "records": [
    { "order_id": 1001, "customer": "Acme Corp", "amount": "1250.00", "qty": 3, "paid": true, "order_date": "2026-01-31", "created_at": "2026-01-31T14:05:09Z", "ship_to": { "city": "Springfield" } },
    { "order_id": 1002, "customer": "Globex", "amount": "99.95", "qty": 1, "paid": false, "order_date": "2026-02-28", "created_at": "2026-02-28T00:00:00Z", "ship_to": { "city": "Zürich" } },
    { "order_id": 1003, "customer": null, "amount": "0.10", "qty": null, "paid": null, "order_date": "1999-12-31", "created_at": null, "ship_to": null }
  ],
  "_options": {
    "compression": "zstd",
    "columns": [
      "order_id", "customer",
      { "field": "amount", "type": "decimal(12,2)" },
      "qty", "paid", "order_date", "created_at",
      { "field": "ship_to.city", "name": "ship_city" }
    ]
  }
}
```

To write a file from a list of flow records, collect them first (for example with an Aggregate step) so the module gets one record holding the list.

| `_options` key | Meaning | Default |
|---|---|---|
| `records_field` | Which field holds the rows | `records` |
| `into` | Where to put the Parquet file (base64) | `file` |
| `columns` | Field names, or `{ "field": "ship_to.city", "name": "ship_city", "type": "string" }` objects, in the order to write them; dotted paths read nested fields | every top-level field, in order of first appearance |
| `compression` | `snappy`, `gzip`, `zstd`, `lz4`, `brotli` or `none` | `snappy` |
| `row_group_size` | Rows per row group | `10000` |
| `infer_dates` | Treat text that is all ISO dates or timestamps as `date` or `timestamp` | `true` |
| `keep_source` | Keep the rows in the output | `false` |

### Column types

| `type` | Parquet type | Accepts |
|---|---|---|
| `string` | UTF-8 string | anything; other values are written as JSON text |
| `int64`, `int32` | 64- or 32-bit integer | whole numbers, or text like `"42"` |
| `double` | 64-bit float | numbers, or text like `"9.99"` |
| `decimal(p,s)` | decimal with precision `p` (1 to 38) and scale `s` | numbers or text like `"1,250.00"`; extra decimal places round half away from zero |
| `boolean` | boolean | `true`/`false`, `"yes"`/`"no"`, `"true"`/`"false"`, `1`/`0` |
| `date` | date | `"2026-01-31"` (a timestamp keeps its date) |
| `timestamp` | timestamp in UTC, microseconds | ISO 8601 with `Z` or an offset: `"2026-01-31T16:05:09+02:00"` is stored as 14:05:09 UTC |
| `timestamp_local` | timestamp without a time zone, microseconds | ISO 8601 without a zone, with `T` or a space |
| `json` | string marked as JSON | objects and lists |

Without a type, each column gets one from its values: all `true`/`false` → `boolean`, all whole numbers → `int64`, numbers → `double`, objects or lists → `json`, ISO dates → `date`, ISO timestamps → `timestamp` (or `timestamp_local` when they have no zone), anything else or a mix → `string`. Amounts are usually text in JSON, so give money columns a `decimal(p,s)` type to keep them exact. `null` and missing fields are written as nulls.

## Output

The input comes back with the file instead of the rows (output for `test/sample.json`):

```json
{
  "export_id": "orders-2026-11",
  "file": "UEFSMRUEFTAVQkwVBhUAEgAAKLUv/QA4wQAA6QMAAAAAAADqAwAAAAAAAOsDAAAAAAAAFQAV...",
  "row_count": 3,
  "row_groups": 1,
  "bytes": 2020,
  "compression": "zstd",
  "columns": [
    { "name": "order_id", "type": "int64" },
    { "name": "customer", "type": "string" },
    { "name": "amount", "type": "decimal(12,2)" },
    { "name": "qty", "type": "int64" },
    { "name": "paid", "type": "boolean" },
    { "name": "order_date", "type": "date" },
    { "name": "created_at", "type": "timestamp" },
    { "name": "ship_city", "type": "string" }
  ],
  "ok": true
}
```

When a value doesn't fit its column type, nothing is written and the error names the row and column, for example `{"ok": false, "error": "row 2, column 'qty': expected a whole number, got \"three\""}`.

## Compatibility

Files are checked in tests with pyarrow 22, Polars 1.35 and DuckDB 1.4 for every compression, and read back with parquet-reader. Float and double columns are marked with the type-defined column order that pyarrow also writes, because some readers don't yet know the newer IEEE 754 total order.

## Limits

- Nested objects and lists are written as JSON text columns, not as Parquet structs or lists. Use dotted paths in `columns` to write nested fields as their own columns.
- The whole file is built in memory. In local tests 100,000 rows (four columns) took about 0.4 seconds and gave a 1.4 MB Zstandard file. For much larger exports, write several files.
- Zstandard compresses with [ruzstd](https://crates.io/crates/ruzstd) (pure Rust), at levels comparable to zstd 1 and 3, through `../vendor/zstd-shim`. Files are a little larger than the C library makes at higher levels.
- No Parquet encryption.

## Use it in a Neblex flow

1. Download `parquet-writer.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `parquet-writer`.
3. Collect the records into one list (for example with an Aggregate step), then add a step that uses the module's **Run module** operation and map the list to `records`. Send `file` on to a storage, SFTP or API step that takes a base64 file.

## Build and test

```sh
../build.sh parquet-writer
docker run --rm -v "$PWD/..":/repo -w /repo/parquet-writer rust:1-slim cargo test --lib
```

The unit tests write files with every option and read them back with parquet-reader. `test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
