# language-detector

Detects the language and writing system of a text, such as a support ticket, an email or a product review, with a confidence score. Use it to route messages to the right team or queue, or to pick a reply template, without sending the text to an AI model. It knows 69 languages, using the [whatlang](https://crates.io/crates/whatlang) trigram models.

## Input

One record or an array of records, each with a text:

```json
{ "ticket": 1042, "text": "Hola, no puedo iniciar sesión.", "_options": { "languages": ["en", "es", "fr", "de"] } }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the text | `text` |
| `into` | Name of the field to add | `language` |
| `languages` | Only choose between these languages (ISO 639-1 such as `es`, ISO 639-3 such as `spa`, or English names) | all 69 |
| `min_confidence` | Return `null` instead of a guess below this score (0 to 1) | `0` |

URLs, email addresses and @handles are ignored, since they carry no language.

## Output

The record comes back with `language` and `script` added:

```json
{
  "language": {
    "code": "spa",
    "iso639_1": "es",
    "name": "Spanish",
    "script": "Latin",
    "confidence": 1.0,
    "reliable": true,
    "in_list": true
  },
  "script": "Latin",
  "ok": true
}
```

| Field | Meaning |
|---|---|
| `language.code` / `iso639_1` | ISO 639-3 and ISO 639-1 codes (`cmn`/`zh` for Chinese, `nob`/`nb` for Norwegian) |
| `language.reliable` | Route on this. `false` means the text is too short or too mixed to be sure |
| `language.in_list` | Only with `languages`: `false` when the text is clearly in a language outside your list. The module then returns that language instead of forcing a wrong match |
| `script` | Writing system (Latin, Cyrillic, Arabic, Hangul...), also when no language could be picked |

When nothing can be detected (no letters, or an empty text), `language` is `null` and `ok` is still `true`.

## Short texts

Full sentences are detected reliably. Short texts such as subject lines are often marked `reliable: false`, and very short ones can be wrong: without a list, "Password reset not working" comes back as Afrikaans with low confidence. With `languages` set to the ones your team handles, short texts land correctly:

| Text | Without a list | With `["en","es","fr","de","pt","ru"]` |
|---|---|---|
| Where is my order? It has been two weeks. | en, not reliable | en |
| Hola, no puedo iniciar sesión. | es, not reliable | es |
| Password reset not working | af, not reliable | en |
| Где мой заказ? Уже прошло две недели. | bg, not reliable | ru, reliable |

One-word texts ("Danke!") are guesses either way. Send texts that are not `reliable` to an AI step or a person.

## Use it in a Neblex flow

1. Download `language-detector.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `language-detector`.
3. Add a step that uses the module's **Run module** operation after a step that returns tickets or emails, and map the message text to `text`. Then branch on `language.iso639_1` where `language.reliable` is true.

## Build and test

```sh
../build.sh language-detector
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` has a French ticket, a short Spanish one with a language list and a Turkish one outside the list.
