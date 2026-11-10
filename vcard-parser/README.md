# vcard-parser

Turns vCard (`.vcf`) contact cards into contact records: cards attached to emails, exports from phones, address books and CRMs. Versions 2.1, 3.0 and 4.0 are supported, including the quoted-printable encoding older phones use for non-English names.

## Input

One record or an array of records, each with the vCard text (one or many cards):

```json
{ "vcf": "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Doe;Jane;;Dr.;\r\nFN:Dr. Jane Doe\r\n..." }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the vCard text | `vcf` |
| `keep_source` | Keep the vCard text in the output | `false` |

## Output

```json
{
  "contact_count": 2,
  "contacts": [
    {
      "version": "3.0",
      "full_name": "Dr. Jane Doe",
      "name": { "family": "Doe", "given": "Jane", "additional": null, "prefix": "Dr.", "suffix": null },
      "organization": "Acme Corp", "department": "Procurement", "title": "Buyer",
      "emails": [{ "value": "jane.doe@acme.com", "types": ["work"] }],
      "phones": [{ "value": "+1 415 555 0123", "types": ["work", "voice"] }],
      "addresses": [{ "types": ["work"], "po_box": null, "extended": "Suite 200", "street": "123 Main St", "city": "Springfield", "region": "IL", "postal_code": "62701", "country": "USA" }],
      "urls": []
    },
    { "version": "2.1", "full_name": "Jörg Müller", "name": { "family": "Müller", "given": "Jörg", ... }, "phones": [{ "value": "030 1234567", "types": ["work", "voice"] }], ... }
  ],
  "ok": true
}
```

- `types` come from `TYPE=` parameters (3.0, 4.0) or bare parameters such as `;WORK;CELL` (2.1), lowercased; `internet` is left out.
- `birthday` is returned as an ISO date when the card has one; `note` with escapes decoded.
- A card without `FN` gets `full_name` from its name parts.
- Phone numbers are returned as written. To normalize them, use `phone_e164` from the Rhai modules or `text-extractor`.

Photos and other binary properties are skipped.

## Use it in a Neblex flow

1. Download `vcard-parser.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `vcard-parser`.
3. Add a step that uses the module's **Run module** operation after a mail, CRM or storage step that returns a .vcf contact card. Map it to `vcf`.

## Build and test

```sh
../build.sh vcard-parser
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input (a 3.0 card and a 2.1 card with an encoded name) for trying the module in a flow or with `../test/call.py`.
