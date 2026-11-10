# identifier-validator

Validates and normalizes business identifiers on each record before they reach your ERP, payment run or product catalog. It catches typos and malformed values at the start of a flow, where they are cheap to fix.

| Type | What is checked | Extra output |
|---|---|---|
| `iban` | Country, length for that country, ISO 7064 MOD 97-10 check digits | `formatted` (groups of four), `country` |
| `vat` | Country prefix and number format for the 27 EU countries, the UK (GB) and Northern Ireland (XI). `GR` is corrected to `EL`. | `country`, `check: "format"` |
| `gtin` | GS1 check digit for GTIN-8, UPC-A, EAN-13 and GTIN-14 | `format`, `gtin14` (zero-padded) |
| `aba` | US routing number: 9 digits, valid prefix, 3-7-1 checksum | |
| `lei` | Legal Entity Identifier (ISO 17442): 20 characters, MOD 97-10 check digits | |

Every check runs offline. A valid result means the identifier is well formed, not that the account, VAT registration or company exists. For VAT registration status, look the number up in the EU VIES service or with HMRC.

## Input

One record or an array of records. The module picks fields by name:

- names containing `iban` → `iban`
- `vat`, `vat_id`, `vat_number` → `vat`
- names containing `gtin` or `barcode`, or `ean`, `upc` → `gtin`
- `aba` or names containing `routing` → `aba`
- `lei` → `lei`

To name the fields yourself, add a `_checks` object. It replaces the name matching and is removed from the output.

```json
{ "sku": "A-1", "code": "4006381333931", "_checks": { "code": "gtin" } }
```

Empty and null fields are skipped.

## Output

The record comes back unchanged, with two fields added:

```json
{
  "supplier": "Initech",
  "iban": "GB82WEST12345698765431",
  "routing_number": "021000021",
  "_validation": {
    "iban": { "type": "iban", "valid": false, "error": "check digits do not match" },
    "routing_number": { "type": "aba", "valid": true, "normalized": "021000021" }
  },
  "_valid": false
}
```

`_valid` is false when any checked field fails. In the flow, filter on it or route failed records to a review step.

## Use it in a Neblex flow

1. Download `identifier-validator.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `identifier-validator`.
3. Add a step that uses the module's **Run module** operation after a step that returns records with IBANs, VAT IDs, GTINs, routing numbers or LEIs. Map the record as is (fields are picked by name), or name them in `_checks`.

## Build and test

```sh
../build.sh identifier-validator
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` has three records (one valid, one with a bad IBAN, one with a bad barcode check digit) for trying the module in a flow or with `../test/call.py`.
