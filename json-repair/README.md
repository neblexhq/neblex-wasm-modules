# json-repair

Turns almost-JSON into valid JSON. AI models asked for JSON often wrap it in prose or a code fence, use single quotes or Python's `True` and `None`, leave trailing commas, or stop mid-object when they hit a token limit. A strict JSON step rejects all of that. This module repairs it and lists every fix, so you can decide whether to trust the result.

## Input

One record or an array of records, each with the text to repair:

```json
{ "text": "Sure! Here is the order:\n```json\n{'customer': 'Acme', total: 1250., \"lines\": [1, 2,],}\n```" }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the text | `text` |
| `into` | Where to write the parsed value | `json` |

## What it repairs

| Problem | Example |
|---|---|
| Text before or after the JSON, code fences | `Sure! ```json {...} ``` Hope this helps` |
| Single quotes, unquoted keys | `{'a': 'x', b: 1}` |
| Trailing and missing commas | `[1, 2,]`, `{"a": 1 "b": 2}` |
| Comments | `// note`, `/* note */` |
| Non-JSON literals | `True`, `False`, `None`, `NaN`, `undefined` |
| Number forms | `1250.`, `.5`, `+3` |
| Line breaks inside strings | a string that spans two lines |
| Text cut off mid-way | `{"items": [{"sku": "A1"}, {"sku": "B2", "name": "Bo` is closed and kept |

Valid JSON passes through untouched, with `repaired: false`.

## Output

```json
{
  "json": { "customer": "Acme", "total": 1250, "lines": [1, 2] },
  "repaired": true,
  "fixes": ["removed code fence", "replaced single quotes", "quoted unquoted keys", "normalized numbers", "removed trailing commas"],
  "ok": true
}
```

When no JSON object or array can be found, the record comes back with `"ok": false` and the reason.

A fix that mentions the text being cut off means the source stopped early: the value is valid, but fields after the cut are missing. Route those records to a retry or a review step rather than loading them as complete.

## Build and test

```sh
../build.sh json-repair
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
