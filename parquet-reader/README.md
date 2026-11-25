# parquet-reader

Reads an Apache Parquet file into JSON records, with the column names and types, so the rest of the flow can work with exports from data warehouses, data lakes and analytics tools. Use it to load a Parquet export from Snowflake, BigQuery, Databricks, Spark, DuckDB, Polars or pandas into an application, check it, or split it into batches.

It reads files compressed with Snappy, Gzip, Zstandard, LZ4 or Brotli, or not compressed at all, including older Spark and Hive files.

## Input

Any JSON that contains the Parquet file as base64. The module finds the first string that starts with `UEFSM` (base64 for the `PAR1` marker every Parquet file starts with), or a `data:` URL of one, wherever it sits in the input. Set `field` to name it directly.

```json
{ "export_id": "orders-2026-11", "file": { "name": "orders.parquet", "content": "UEFSMRUEFbAB..." }, "_options": { "limit": 100 } }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Dotted path to the base64 file, such as `file.content` | first base64 Parquet string found |
| `columns` | Only read these top-level columns, in this order | all columns |
| `offset` | Skip this many rows first | `0` |
| `limit` | Return at most this many rows | `10000` |
| `decimals` | `"string"` keeps decimal values exact (`"1250.00"`); `"number"` returns JSON numbers | `"string"` |
| `schema` | `false` leaves out `columns`, `row_groups` and `created_by` | `true` |

An array of records gives an array of results.

## Output

The input comes back without the file itself, with the rows and schema added:

```json
{
  "export_id": "orders-2026-11",
  "file": { "name": "orders.parquet" },
  "ok": true,
  "source_field": "file.content",
  "row_count": 4,
  "returned": 4,
  "truncated": false,
  "columns": [
    { "name": "order_id", "type": "int64", "nullable": true },
    { "name": "customer", "type": "string", "nullable": true },
    { "name": "amount", "type": "decimal(12,2)", "nullable": true },
    { "name": "order_date", "type": "date", "nullable": true },
    { "name": "created_at", "type": "timestamp", "nullable": true },
    { "name": "tags", "type": "list", "nullable": true },
    { "name": "ship_to", "type": "struct", "nullable": true }
  ],
  "row_groups": 1,
  "created_by": "parquet-cpp-arrow version 22.0.0",
  "rows": [
    {
      "order_id": 1001,
      "customer": "Acme Corp",
      "amount": "1250.00",
      "order_date": "2026-01-31",
      "created_at": "2026-01-31T14:05:09Z",
      "tags": ["rush", "b2b"],
      "ship_to": { "city": "Springfield", "zip": "01101" }
    }
  ]
}
```

(Shortened: the test file has more columns and rows.)

| Field | Meaning |
|---|---|
| `row_count` | Rows in the whole file |
| `returned` | Rows in `rows` |
| `truncated` | `true` when the file has rows after the last one returned. Read the next page with `offset` |
| `rows` | One object per row, with columns in the file's order |

Values are converted like this:

| Parquet type | JSON |
|---|---|
| Integers, floats, booleans, strings | numbers, booleans, strings |
| Decimal | exact string such as `"12.345"`, or a number with `decimals: "number"` |
| Date | `"2026-01-31"` |
| Timestamp (milliseconds, microseconds or nanoseconds) | ISO 8601: `"2026-01-31T14:05:09.123456Z"`. Columns stored without a time zone have no `Z` |
| Old Spark/Hive INT96 timestamp | ISO 8601 in UTC |
| Time of day | `"08:30:15.000000"` |
| List, struct, map | array, object, object |
| Binary | base64 string |
| Missing value | `null` |

On failure it returns `{"ok": false, "error": "..."}`, for example when the column you named isn't in the file (the error lists the columns that are) or the file was cut off in transfer.

## Large files

Rows are read one row group at a time. With `offset`, whole row groups before it are skipped without being decompressed, so reading a large file page by page stays fast. In local tests the module read 50,000 rows (four columns) in about 0.15 seconds using about 57 MB of memory. For very large files, page through with `offset` and `limit` and pick the columns you need.

## Limits

- It reads Parquet; it doesn't write it.
- Encrypted Parquet files (Parquet modular encryption) are not supported.
- Integers above 2^53 (9,007,199,254,740,992) are returned exactly, but tools that read JSON numbers as JavaScript numbers round them.
- Dates and timestamps outside about 262,000 years either side of year 0 come back as the raw stored number.
- A file whose data pages are damaged (not just cut off) stops the step with a module error (in Neblex: "the module failed: UnreachableCodeReached") instead of returning `ok: false`.
- Zstandard pages are decoded with [ruzstd](https://crates.io/crates/ruzstd), a pure-Rust decoder, through a small stand-in for the `zstd` crate in `../vendor/zstd-shim`.

## Use it in a Neblex flow

1. Download `parquet-reader.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `parquet-reader`.
3. Add a step that uses the module's **Run module** operation after a storage, SFTP or API step that returns the file as base64. Then split `rows` into records with an Unwind step.

## Build and test

```sh
../build.sh parquet-reader
docker run --rm -v "$PWD/..":/repo -w /repo/parquet-reader rust:1-slim cargo test --lib
```

The unit tests read the files in `test/fixtures/`, written by pyarrow (every compression, many data types, several row groups, an empty file and an old Spark-style file), Polars and DuckDB. `test/make_fixtures.py` recreates them. `test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
