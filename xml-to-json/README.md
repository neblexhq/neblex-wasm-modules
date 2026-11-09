# xml-to-json

Converts XML documents into JSON a flow can work with: ERP messages such as Infor ION BODs, SOAP responses, older APIs and exports, and XML bank or supplier files. It can return the whole document, or one record per repeating element (one per order line, say) with header fields copied onto each record.

## Input

One record or an array of records, each with the XML text and optional `_options`:

```json
{
  "xml": "<SyncSalesOrder>...</SyncSalesOrder>",
  "_options": {
    "records": "DataArea/SalesOrder/SalesOrderLine",
    "include": { "order_id": "DataArea/SalesOrder/SalesOrderHeader/DocumentID/ID", "customer": "//Name" },
    "numbers": true
  }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the XML | `xml` |
| `records` | Path to the repeating element: names separated by `/` from the root (the root's own name may lead), or `//Name` for that element at any depth. Without it, the whole document is returned as `data`. | |
| `include` | Values to copy onto every record, as `{ "output_name": "path" }`. The first match is used. | |
| `numbers` | Turn numeric text into numbers. Values with leading zeros, such as `007` or a GTIN, stay text. | `false` |
| `always_array` | Element names that are always arrays, even when one appears, so later steps see the same shape every time | |
| `keep_namespaces` | Keep prefixes such as `ns2:` in names; by default names are local (`SalesOrder`) | `false` |
| `keep_source` | Keep the XML text in the output | `false` |

## Output

How elements map to JSON:

- An element with only text becomes that text: `<Name>Acme</Name>` → `"Name": "Acme"`. An empty element becomes null.
- An element with attributes or children becomes an object. Attributes are prefixed with `@`, and text next to them goes in `#text`: `<Quantity unitCode="EA">5</Quantity>` → `{ "@unitCode": "EA", "#text": 5 }`.
- Repeated elements become arrays.

With `records`, the output has `records` and `record_count`:

```json
{
  "ok": true,
  "record_count": 2,
  "records": [
    { "LineNumber": 1, "Item": { "ItemID": { "ID": "WIDGET-9" } }, "Quantity": { "@unitCode": "EA", "#text": 5 },
      "order_id": { "@accountingEntity": "100", "#text": "SO-1001" }, "customer": "Acme & Sons" }
  ]
}
```

Add an Unwind step on `records` to get one flow record per line.

XML that isn't well formed returns `"ok": false` with the parser's reason and position. Documents with a DTD are refused, so entity-expansion attacks can't run.

## Build and test

```sh
../build.sh xml-to-json
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
