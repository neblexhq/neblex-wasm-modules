# gs1-barcode-parser

Splits a scanned GS1 barcode into its fields: product code (GTIN), batch or lot, expiry and other dates, serial number, weights, shipping container code (SSCC) and more. It reads what a scanner sends for GS1-128, GS1 DataMatrix and GS1 QR codes, and the human-readable text printed under them. Check digits are verified.

## Input

One record or an array of records, each with one scan:

```json
{ "barcode": "(01)09506000134352(17)261231(10)AB-123(21)SN0001" }
```

Both forms work:

- Human-readable: `(01)09506000134352(17)261231(10)AB-123`
- Raw scanner output: `01095060001343521726123110AB-123`, with the FNC1 separator sent as the GS character (ASCII 29) after each variable-length field that isn't last. A leading symbology identifier such as `]C1`, `]d2` or `]Q3` is removed. `<GS>` and `{GS}` are also read as the separator, for systems that write it out.

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the scan | `barcode` |

## Output

```json
{
  "gs1": { "gtin": "09506000134352", "expiry": "2026-12-31", "batch": "AB-123", "serial": "SN0001" },
  "elements": [
    { "ai": "01", "name": "gtin", "value": "09506000134352", "raw": "09506000134352" },
    { "ai": "17", "name": "expiry", "value": "2026-12-31", "raw": "261231" }
  ],
  "errors": [],
  "_valid": true,
  "ok": true
}
```

`gs1` holds the fields by name for later steps. `elements` keeps the scan order and the raw values. `errors` lists wrong check digits, invalid dates, wrong lengths and unknown Application Identifiers, and `_valid` is false when there are any.

## Supported Application Identifiers

| AI | Name | Notes |
|---|---|---|
| 00 | `sscc` | 18 digits, check digit verified |
| 01, 02 | `gtin`, `content_gtin` | 14 digits, check digit verified |
| 10, 21, 22 | `batch`, `serial`, `consumer_product_variant` | up to 20 characters |
| 11, 12, 13, 15, 16, 17 | `production_date`, `due_date`, `packaging_date`, `best_before`, `sell_by`, `expiry` | YYMMDD to ISO date; day 00 means the last day of the month |
| 20 | `variant` | 2 digits |
| 30, 37 | `variable_count`, `count` | up to 8 digits |
| 240, 241, 250 | `additional_product_id`, `customer_part_number`, `secondary_serial` | up to 30 characters |
| 400, 401 | `customer_po`, `consignment_number` | up to 30 characters |
| 402 | `shipment_id` | 17 digits, check digit verified |
| 410-415 | `ship_to_gln`, `bill_to_gln`, `purchased_from_gln`, `ship_for_gln`, `location_gln`, `invoicing_party_gln` | 13 digits, check digit verified |
| 420, 422 | `ship_to_postal_code`, `country_of_origin` | |
| 310n-315n, 320n, 330n, 340n | `net_weight_kg`, `length_m`, `width_m`, `height_m`, `net_volume_l`, `net_weight_lb`, `gross_weight_kg`, `gross_weight_lb` | 6 digits; n is the number of decimal places |
| 392n, 393n | `price`, `price_with_currency` | n decimal places; 393n starts with the ISO 4217 numeric currency |
| 8020 | `payment_slip_reference` | up to 25 characters |
| 90-99 | `internal_90` ... `internal_99` | company-defined |

Other AIs are reported in `errors`. Two-digit years 00-49 are read as 20xx and 50-99 as 19xx: the module has no clock, so it can't apply GS1's rule of choosing the century nearest the current year.

## Build and test

```sh
../build.sh gs1-barcode-parser
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` has three scans (human-readable, raw with separators, and a logistics label) for trying the module in a flow or with `../test/call.py`.
