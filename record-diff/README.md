# record-diff

Compares an incoming record with the version you already have and returns only what changed. Sync flows can then skip records that didn't change, update just the changed fields, keep an audit trail of old and new values, or route a change to a particular field (a new owner, a price) down its own path. Skipping no-op writes also saves API quota on the target system.

## Input

One record or an array of records, each holding the stored and the incoming version:

```json
{
  "old": { "id": 7, "email": "Jane@Example.com", "phone": "", "address": { "city": "Boston", "zip": "02134" }, "updated_at": "2026-11-01" },
  "new": { "id": 7, "email": "jane@example.com", "phone": null, "address": { "city": "Cambridge", "zip": "02134" }, "updated_at": "2026-11-09", "tier": "gold" },
  "_options": { "ignore": ["updated_at"], "case_insensitive": true }
}
```

If `old` is missing or null, the record counts as new and every field is a change.

| `_options` key | Meaning | Default |
|---|---|---|
| `old`, `new` | Field names holding the stored and incoming versions | `old`, `new` |
| `ignore` | Fields or dotted paths to leave out, such as timestamps the source always changes | |
| `only` | Compare only these fields or paths | all |
| `case_insensitive` | `Jane@Example.com` equals `jane@example.com` | `false` |
| `trim` | Ignore spaces at the start and end of text | `true` |
| `empty_equal` | Treat null, a missing field, `""` and `[]` as the same | `true` |
| `numeric_strings` | `"1250.00"` equals `1250` | `true` |
| `tolerance` | Numbers within this difference count as equal (for example `0.005` for rounding) | `0` |
| `report_removed` | Also report fields present in `old` but absent from `new` | `false` |

Only fields present in `new` are compared, unless `report_removed` is set, because many APIs send partial records. Nested objects are compared field by field with dotted paths; arrays are compared as a whole.

## Output

```json
{
  "is_new": false,
  "changed": true,
  "changed_fields": ["address.city", "tier"],
  "changes": {
    "address.city": { "old": "Boston", "new": "Cambridge" },
    "tier": { "old": null, "new": "gold" }
  },
  "patch": { "address": { "city": "Cambridge" }, "tier": "gold" },
  "ok": true
}
```

- `changed` is the field to filter or branch on.
- `patch` has only the new values of changed fields, nested like the record, ready for an update call.
- `changes` is the before-and-after record for an audit log.

## Build and test

```sh
../build.sh record-diff
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
