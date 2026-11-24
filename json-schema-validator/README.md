# json-schema-validator

Checks each record against a JSON Schema and lists every problem with the field it's in. Use it as a data contract at the edge of a flow: check API payloads, webhook bodies or rows from a file before they reach your ERP, CRM or warehouse, and send the records that fail to a review queue with the reasons attached.

Supports JSON Schema drafts 4, 6, 7, 2019-09 and 2020-12 (the default), including `$ref` and `$defs` within the schema, `if`/`then`/`else`, `pattern` and formats such as `email`, `date`, `date-time`, `uri`, `uuid` and `ipv4`.

## Input

One record or an array of records. By default the whole record (without `_options`) is checked:

```json
{
  "order_id": "PO-10042",
  "email": "buyer.example.com",
  "ship_date": "2026-12-01",
  "currency": "CAD",
  "lines": [{ "sku": "AB-100", "qty": 3 }, { "sku": "CD-7", "qty": 0 }],
  "_options": {
    "schema": {
      "type": "object",
      "required": ["order_id", "email", "lines"],
      "properties": {
        "order_id": { "type": "string", "pattern": "^PO-[0-9]{4,}$" },
        "email": { "type": "string", "format": "email" },
        "ship_date": { "type": "string", "format": "date" },
        "currency": { "enum": ["USD", "EUR", "GBP"] },
        "lines": {
          "type": "array", "minItems": 1,
          "items": {
            "type": "object", "required": ["sku", "qty"],
            "properties": { "sku": { "type": "string", "minLength": 3 }, "qty": { "type": "integer", "minimum": 1 } }
          }
        }
      }
    }
  }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `schema` | The JSON Schema, as an object or as JSON text (for example read from a file or a table) | required |
| `field` | Check only this field (dotted path such as `order.shipping`) instead of the whole record | whole record |
| `formats` | Check `format` (email, date, uri...). Set `false` to treat formats as labels only | `true` |
| `draft` | `4`, `6`, `7`, `2019-09` or `2020-12`; overrides `$schema` | from `$schema`, else 2020-12 |
| `max_errors` | Most errors to list per record (`error_count` still counts them all) | `50` |

`$ref` must point inside the schema (`#/$defs/...`). The module has no network or file access, so a `$ref` to a URL is reported as a schema error rather than fetched.

## Output

The record comes back with these fields added (output of the input above):

```json
{
  "valid": false,
  "error_count": 3,
  "errors": [
    {
      "field": "currency",
      "keyword": "enum",
      "message": "\"CAD\" is not one of \"USD\", \"EUR\" or \"GBP\"",
      "path": "/currency",
      "schema_path": "/properties/currency/enum"
    },
    {
      "field": "email",
      "keyword": "format",
      "message": "\"buyer.example.com\" is not a \"email\"",
      "path": "/email",
      "schema_path": "/properties/email/format"
    },
    {
      "field": "lines[1].qty",
      "keyword": "minimum",
      "message": "0 is less than the minimum of 1",
      "path": "/lines/1/qty",
      "schema_path": "/properties/lines/items/properties/qty/minimum"
    }
  ],
  "invalid_fields": [
    "currency",
    "email",
    "lines[1].qty"
  ],
  "ok": true
}
```

- `valid` is the field to branch on.
- `field` uses dotted paths with list positions (`lines[1].qty`); `path` is the same location as a JSON Pointer. For a missing required field, `path` points at the object that lacks it and `field` names the missing field itself.
- `invalid_fields` lists each field once, ready for a review screen or a message back to the sender.

A schema that isn't valid returns `"ok": false` with the reason.

## Use it in a Neblex flow

1. Download `json-schema-validator.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `json-schema-validator`.
3. Add a step that uses the module's **Run module** operation. Map the record, and put your schema in `_options.schema`.
4. Branch on `valid`: send valid records on, and invalid ones with `invalid_fields` to a review queue or back to the sender.

## Build and test

```sh
../build.sh json-schema-validator
docker run --rm -v "$PWD/..":/repo -w /repo/json-schema-validator rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`. Built on the [jsonschema](https://github.com/Stranger6667/jsonschema) crate (MIT).
