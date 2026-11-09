# uom-converter

Converts quantities between units of measure, including the pack sizes your business uses: a case of 12, a box of 6, a pallet of 40 cases. Orders, ERPs, warehouses and suppliers rarely agree on units, so this converts order lines to the unit the target system expects, and flags any it can't convert instead of guessing.

## Input

A record with a quantity and units:

```json
{ "quantity": "2.5", "from": "KG", "to": "lb", "_options": { "places": 2 } }
```

or a record holding a list of lines:

```json
{
  "order": "SO-1001",
  "lines": [ { "sku": "W-9", "qty": 3, "uom": "CS" }, { "sku": "B-2", "qty": 30, "uom": "EA" }, { "sku": "P-1", "qty": 1, "uom": "PLT" } ],
  "_options": {
    "lines": "lines", "quantity_field": "qty", "from_field": "uom", "to": "EA", "into": "qty_ea",
    "packs": { "CS": { "EA": 12 }, "PLT": { "CS": 40 } }, "places": 0
  }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `quantity_field`, `from_field`, `to_field` | Field names for the quantity and units | `quantity`, `from`, `to` |
| `to` | One target unit for every record or line, instead of a field | |
| `into` | Where to write the result (and `<into>_unit`) | `converted` |
| `packs` | Your pack sizes: `{ "CS": { "EA": 12 } }` means one CS holds 12 EA. They chain (PLT → CS → EA) and work in both directions. | |
| `lines` | The field holding a list of lines to convert | |
| `places` | Round to this many decimals (0 gives whole numbers) | no rounding |

## Output

```json
{
  "lines": [
    { "sku": "W-9", "qty": 3, "uom": "CS", "qty_ea": 36, "qty_ea_unit": "EA", "factor": 12.0, "_valid": true },
    { "sku": "B-2", "qty": 30, "uom": "EA", "qty_ea": 30, "qty_ea_unit": "EA", "factor": 1.0, "_valid": true },
    { "sku": "P-1", "qty": 1, "uom": "PLT", "qty_ea": 480, "qty_ea_unit": "EA", "factor": 480.0, "_valid": true }
  ],
  "_valid": true,
  "ok": true
}
```

A unit that can't be converted (kilograms to meters, or a pack code with no size) gives `null`, `_valid: false` and an `error` naming the missing pack size, and the record's `_valid` is false.

## Built-in units

| Kind | Units (codes and common names) |
|---|---|
| Mass | MG, G, KG, T (metric tonne), OZ, LB |
| Length | MM, CM, M, KM, IN, FT, YD, MI |
| Volume | ML, L, M3, FLOZ, PT, QT, GAL (US measures) |
| Count | EA (each, pc, pcs, unit), PR (pair), DZ (dozen), GRO (gross) |

Names such as "kilograms", "lbs", "pieces" and "dozen" are recognized. Ambiguous abbreviations ("gr", "ct") are not, so they need a pack size or a clear code. Imperial (UK) gallons and pints, and conversions between mass and volume, need your own pack sizes.

## Build and test

```sh
../build.sh uom-converter
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input (an order with cases, eaches and a pallet) for trying the module in a flow or with `../test/call.py`.
