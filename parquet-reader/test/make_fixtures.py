"""Writes the Parquet test files in test/fixtures with the tools that usually produce them.

Run from the parquet-reader folder:
  docker run --rm -v "$PWD/test":/t:ro -v "$PWD/test/fixtures":/out python:3.12-slim sh -c \
    'pip -q install pyarrow==22.0.0 polars==1.35.2 duckdb==1.4.2 && python -I /t/make_fixtures.py /out'
"""
import datetime as dt
import decimal
import sys
from pathlib import Path

import duckdb
import polars as pl
import pyarrow as pa
import pyarrow.parquet as pq

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

orders = pa.table({
    "order_id": pa.array([1001, 1002, 1003, 1004], pa.int64()),
    "customer": pa.array(["Acme Corp", "Globex", None, "Initech"], pa.string()),
    "amount": pa.array([decimal.Decimal("1250.00"), decimal.Decimal("99.95"), decimal.Decimal("0.10"), None], pa.decimal128(12, 2)),
    "qty": pa.array([3, 1, None, 12], pa.int32()),
    "unit_price": pa.array([416.6667, 99.95, 0.1, 5.5], pa.float64()),
    "paid": pa.array([True, False, None, True], pa.bool_()),
    "order_date": pa.array([dt.date(2026, 1, 31), dt.date(2026, 2, 28), dt.date(1999, 12, 31), None], pa.date32()),
    "created_at": pa.array([dt.datetime(2026, 1, 31, 14, 5, 9), dt.datetime(2026, 2, 28, 0, 0, 0), None, dt.datetime(2026, 3, 1, 23, 59, 59)], pa.timestamp("ms", tz="UTC")),
    "tags": pa.array([["rush", "b2b"], [], None, ["b2b"]], pa.list_(pa.string())),
    "ship_to": pa.array([{"city": "Springfield", "zip": "01101"}, {"city": "Zürich", "zip": "8001"}, None, {"city": "Austin", "zip": None}],
                        pa.struct([("city", pa.string()), ("zip", pa.string())])),
})

for codec in ["snappy", "gzip", "zstd", "lz4", "brotli", "none"]:
    pq.write_table(orders, out / f"orders-{codec}.parquet", compression=codec)

# Several row groups so offset/limit can skip whole groups: 25 rows, 10 per group.
many = pa.table({"n": pa.array(range(25), pa.int64()), "label": pa.array([f"row {i}" for i in range(25)])})
pq.write_table(many, out / "rowgroups.parquet", row_group_size=10, compression="snappy")

# Polars writes zstd by default; DuckDB writes snappy by default.
pl.DataFrame({"sku": ["A-1", "B-2"], "price": [9.99, 15.0], "ts": [dt.datetime(2026, 5, 1, 8, 30), dt.datetime(2026, 5, 2, 9, 45)]}).write_parquet(out / "polars-default.parquet")
duckdb.sql("COPY (SELECT 1 AS id, 'Café' AS name, DATE '2026-04-01' AS d, 12.345::DECIMAL(10,3) AS price, NULL::VARCHAR AS note "
           "UNION ALL SELECT 2, 'Tea', DATE '2026-04-02', 1.5, 'x') TO '" + str(out / "duckdb-default.parquet") + "' (FORMAT parquet)")

# Timestamp units and zones, time of day, maps, binary, nested lists.
types = pa.table({
    "ts_ns_utc": pa.array([dt.datetime(2026, 1, 31, 14, 5, 9, 123456)], pa.timestamp("ns", tz="UTC")),
    "ts_us_local": pa.array([dt.datetime(2026, 1, 31, 14, 5, 9)], pa.timestamp("us")),
    "ts_ms_utc": pa.array([dt.datetime(1969, 12, 31, 23, 59, 59, 500000)], pa.timestamp("ms", tz="UTC")),
    "t": pa.array([dt.time(8, 30, 15)], pa.time64("us")),
    "attrs": pa.array([[("color", "red"), ("size", "L")]], pa.map_(pa.string(), pa.string())),
    "blob": pa.array([b"\x00\x01hi"], pa.binary()),
    "matrix": pa.array([[[1, 2], [3]]], pa.list_(pa.list_(pa.int32()))),
    "big": pa.array([9007199254740993], pa.int64()),
    "u8": pa.array([200], pa.uint8()),
})
pq.write_table(types, out / "types.parquet")
# Spark and Hive files written before 2020 use INT96 timestamps and two-level lists.
pq.write_table(pa.table({"ts": pa.array([dt.datetime(2026, 1, 31, 14, 5, 9)], pa.timestamp("ms")),
                         "nums": pa.array([[1, 2, 3]], pa.list_(pa.int32()))}),
               out / "legacy-spark.parquet", use_deprecated_int96_timestamps=True, use_compliant_nested_type=False,
               version="1.0")

# Empty file (schema, no rows).
pq.write_table(orders.slice(0, 0), out / "empty.parquet")
for f in sorted(out.glob("*.parquet")):
    print(f.name, f.stat().st_size)
