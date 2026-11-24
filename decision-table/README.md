# decision-table

Evaluates a decision table on each record: discount and pricing rules, approval thresholds, routing, shipping options or risk flags, kept as data instead of code. The people who own the rules can read and change the table, and every result names the rule that produced it.

Conditions use the ZEN expression language (`> 100`, `[1000..9999]`, `"DE", "FR"`, `endsWith($, "@example.com")`). Tables in the JDM format of the open-source [ZEN engine](https://github.com/gorules/zen) work as they are, so you can also design them in a JDM editor and paste the JSON.

## Input

One record or an array of records, with the table in `_options.table`:

```json
{
  "customer": { "tier": "gold" },
  "order": { "total": 12500, "country": "US" },
  "_options": {
    "table": {
      "hit_policy": "first",
      "inputs": [
        { "field": "customer.tier", "name": "Tier" },
        { "field": "order.total", "name": "Order total" },
        { "field": "order.country", "name": "Country" }
      ],
      "outputs": [{ "field": "discount" }, { "field": "approval" }],
      "rules": [
        { "_description": "Gold, large order", "Tier": "\"gold\"", "Order total": ">= 10000", "discount": "0.15", "approval": "\"manager\"" },
        { "_description": "Gold", "Tier": "\"gold\"", "discount": "0.10", "approval": "\"none\"" },
        { "_description": "EU mid-size", "Order total": "[1000..9999]", "Country": "\"DE\", \"FR\", \"NL\"", "discount": "0.05", "approval": "\"none\"" },
        { "_description": "Everyone else", "discount": "0", "approval": "\"none\"" }
      ]
    }
  }
}
```

### The table

| Key | Meaning |
|---|---|
| `hit_policy` (or `hitPolicy`) | `first`: the first matching rule wins. `collect`: every matching rule adds a result |
| `inputs` | The columns to test. `field` is the record field (a dotted path, or an expression such as `lower(email)`); `name` and `id` are other keys a rule can use for the column. A column without a `field` holds whole conditions (`total > 1000 and tier == "gold"`) |
| `outputs` | The result fields, set by each rule. Dotted fields such as `price.unit` build nested results |
| `rules` | One object per row, keyed by a column's `name`, `field` or `id`. `_description` and `_id` are returned with the match |

**Input cells** test the column's value, written `$` in functions:

| Cell | Matches |
|---|---|
| `"gold"` | equal to the text gold |
| `"DE", "FR", "NL"` | any of these |
| `> 100`, `<= 30` | comparisons |
| `[1000..9999]`, `(0..1)` | ranges, inclusive with `[ ]`, exclusive with `( )` |
| `startsWith($, "PO-")`, `len($) > 10` | any condition using `$` |
| empty, `-` or left out | anything |

A condition that can't apply to the value, such as a range check on a missing field, doesn't match. A condition that doesn't parse returns an error naming the rule and column.

**Output cells** are expressions over the record: `0.15`, `"manager"` (text needs quotes), `list_price * 0.8`, `"Bulk " + string(qty)`. Numbers and booleans can also be written as plain JSON values.

### Options

| `_options` key | Meaning | Default |
|---|---|---|
| `table` | The table as an object or JSON text; or a whole JDM decision graph, in which case the first decision table node is used | required |
| `node` | With a JDM graph, the name or id of the decision table node to use | first one |
| `into` | Field for the result | `decision` |
| `now` | The current time for date conditions such as `date(due) < date("now")`: an ISO date or time, or Unix milliseconds. The module has no clock, so pass the run's time when your rules use dates | 1970-01-01 |

## Output

The record comes back with these fields added (output of the input above):

```json
{
  "matched": true,
  "decision": {
    "approval": "manager",
    "discount": 0.15
  },
  "matched_rule": {
    "description": "Gold, large order",
    "index": 1
  },
  "ok": true
}
```

- `matched` is false and `decision` is null when no rule matches; end the table with a catch-all rule to always get a result.
- With `collect`, `decision` is a list of results and `matched_rules` lists every matching rule.

## Use it in a Neblex flow

1. Download `decision-table.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `decision-table`.
3. Add a step that uses the module's **Run module** operation. Map the record, and put the table in `_options.table`. To let others edit the rules without opening the flow, keep the table JSON in a Data Table row or a file and map it in.

## Build and test

```sh
../build.sh decision-table
docker run --rm -v "$PWD/..":/repo -w /repo/decision-table rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`. Built on the [zen-expression](https://crates.io/crates/zen-expression) crate (MIT); the table evaluation is this module's own.
