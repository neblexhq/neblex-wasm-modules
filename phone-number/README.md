# phone-number

Reads a phone number in any common format and tells you what it is: whether it is valid under its country's numbering plan, which country it belongs to, its type (mobile, landline, toll-free, premium rate, VoIP...), its extension, and the number in E.164, international, national and `tel:` formats. The rules come from Google's libphonenumber metadata (through the [phonenumber](https://crates.io/crates/phonenumber) crate).

Neblex's built-in **Normalize** step already turns phone numbers into E.164. Use this module when you also need the number type (for example, to send SMS only to mobiles), the country of a number, or the extension, which Normalize drops.

## Input

One record or an array of records, each with a phone number:

```json
{ "contact": "Dana Reyes", "phone": "(415) 555-2671 x305", "_options": { "region": "US" } }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the phone number | `phone` |
| `into` | Name of the field to add | `phone_info` |
| `region` | Two-letter country code for numbers written without `+` or an international prefix (`US`, `GB`, `DE`...) | none |

A number that starts with `+` or an international prefix (`00`, `011`) keeps its own country whatever `region` says. Letters are read as keypad digits (`1-800-FLOWERS`).

## Output

The record comes back with `phone_info` added:

```json
{
  "phone_info": {
    "valid": true,
    "e164": "+14155552671",
    "international": "+1 415-555-2671 ext. 305",
    "national": "(415) 555-2671 ext. 305",
    "rfc3966": "tel:+1-415-555-2671;ext=305",
    "country_code": 1,
    "region": "US",
    "type": "fixed_line_or_mobile",
    "extension": "305"
  },
  "ok": true
}
```

| Field | Values |
|---|---|
| `valid` | `true` when the number fits the country's numbering plan (length and number ranges) |
| `reason` | Why it is not valid: `empty`, `no country: add a + and country code, or set _options.region`, `not a valid number under the country's numbering plan`, or a parsing error |
| `region` | The country the number belongs to. Countries that share a code are told apart (`+1 416...` is `CA`, `+44 7911...` is `GG`, Guernsey) |
| `type` | `mobile`, `fixed_line`, `fixed_line_or_mobile`, `toll_free`, `premium_rate`, `shared_cost`, `voip`, `personal`, `pager`, `uan`, `voicemail` or `unknown` |
| `extension` | The extension (`x305`, `ext 12`), or `null` |

The original field is left as it was. An invalid number still gets `ok: true`, with `valid: false` and a `reason`, so you can route it to a review queue.

## Limits

- **US and Canadian numbers are `fixed_line_or_mobile`.** Mobiles and landlines share number ranges there, so the type can't be read from the number. Finding that out needs a carrier lookup service.
- Valid means the number could exist under the numbering plan, not that it is in service.
- Each call loads the numbering data first. In Neblex, the module runs once per record and took about 0.3 s per number in our test, so 1,000 contacts take a few minutes. Outside Neblex, an array of records in one call shares the load: 1,000 numbers take about 0.2 s in total.

## Use it in a Neblex flow

1. Download `phone-number.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `phone-number`.
3. Add a step that uses the module's **Run module** operation after a CRM, form or file step that returns contacts. Map the number to `phone` and set `_options.region` to the country most of your numbers are from.

## Build and test

```sh
../build.sh phone-number
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` has a US number with an extension, a UK mobile, a German landline and an invalid number.
