# yaml-to-json

Converts YAML to JSON so a flow can read configuration files, OpenAPI specs, CI and deployment files, Markdown front matter, or YAML exports from other tools. Anchors, aliases and merge keys (`<<: *defaults`) are resolved, and a file with several documents separated by `---` returns each one.

## Input

One record or an array of records, each with the YAML text:

```json
{ "file": "sync.yaml", "yaml": "name: hubspot-to-erp\nenabled: true\n..." }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the YAML | `yaml` |
| `always_list` | Return `documents` (a list) even when the text has one document | `false` |
| `keep_source` | Keep the YAML text in the output | `false` |

## Output

For this input:

```yaml
# Sync settings
name: hubspot-to-erp
enabled: true
schedule: "*/15 * * * *"
batch_size: 500
defaults: &defaults
  retries: 3
  timeout_s: 30
targets:
  - system: erp
    <<: *defaults
    fields: [email, company, phone]
  - system: warehouse
    retries: 5
zip: "02134"
```

```json
{
  "ok": true,
  "file": "sync.yaml",
  "data": {
    "name": "hubspot-to-erp",
    "enabled": true,
    "schedule": "*/15 * * * *",
    "batch_size": 500,
    "defaults": { "retries": 3, "timeout_s": 30 },
    "targets": [
      { "system": "erp", "retries": 3, "timeout_s": 30, "fields": ["email", "company", "phone"] },
      { "system": "warehouse", "retries": 5 }
    ],
    "zip": "02134"
  }
}
```

- A text with several documents returns `documents` (a list) and `document_count` instead of `data`.
- Numbers, booleans and nulls (`~`, `null`) keep their types; quoted values stay text, so `"02134"` keeps its leading zero.
- Keys that aren't text (such as `404:`) become text. Tags (`!Ref`, `!!str`) are dropped and their value kept. `.inf` and `.nan` become `null`.
- Invalid YAML returns `ok: false` with the line and column of the problem.
- Alias chains that would expand into huge output ("billion laughs") are refused.

## Build and test

```sh
../build.sh yaml-to-json
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
