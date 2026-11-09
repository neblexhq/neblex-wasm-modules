# address-standardizer

Standardizes postal addresses in the style of USPS Publication 28 before they're matched, deduplicated or loaded: street suffixes and directions abbreviated (Street → ST, North → N), unit designators standardized (Apartment → APT, Suite → STE), punctuation removed, states and provinces turned into codes, ZIP and Canadian postal codes formatted, and country names turned into ISO codes. Standardized addresses match each other far more often, which is what duplicate detection and address matching need.

It formats addresses; it does not check that an address exists or is deliverable. For that, use a postal verification service.

## Input

One record or an array of records with the address in separate fields:

```json
{ "id": 1, "line1": "123 north main street apt 4b", "city": "springfield", "state": "Illinois", "zip": "2134" }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `fields` | Your field names, as `{ "line1": "street", "city": "town", ... }` for `line1`, `line2`, `city`, `state`, `zip`, `country` | the names above |
| `case` | `"upper"` (USPS style) or `"title"` (`500 W Georgia St Ste 1000`) | `"upper"` |
| `default_country` | Country to assume when the record has none | `"US"` |

## Output

The original fields are kept, with these added:

```json
{
  "address": { "line1": "123 N MAIN ST APT 4B", "line2": null, "city": "SPRINGFIELD", "state": "IL", "zip": "02134", "country": "US" },
  "changes": [
    { "field": "line1", "from": "123 north main street apt 4b", "to": "123 N MAIN ST APT 4B" },
    { "field": "state", "from": "Illinois", "to": "IL" },
    { "field": "zip", "from": "2134", "to": "02134" }
  ],
  "_valid": true,
  "errors": []
}
```

More examples from the tests: `P.O. Box 4411` → `PO BOX 4411`; `789 Elm St. N.W.` → `789 ELM ST NW`; `456 Oak Avenue, Suite #200` → `456 OAK AVE STE 200`; `Building 7, Floor 3` → `BLDG 7 FL 3`; `v6b4n8` (Canada) → `V6B 4N8`; ZIP `787011234` → `78701-1234`.

- A direction after the house number is abbreviated unless it is the street name itself (`100 North Street` stays `100 NORTH ST`).
- US and Canadian addresses are checked for a recognized state or province and a well-formed ZIP or postal code; `errors` explains what failed and `_valid` is false. Other countries get the same clean-up without those checks.
- `changes` lists every field that changed, for review and audit.

Coverage: the common USPS street suffixes, directions and unit designators; the 50 states, DC and US territories; the Canadian provinces and territories; and country names for the US, Canada, Mexico, the UK, Germany, France, the Netherlands and Australia (others are kept as written and flagged).

## Build and test

```sh
../build.sh address-standardizer
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` has three addresses (US street, PO box, Canada) for trying the module in a flow or with `../test/call.py`.
