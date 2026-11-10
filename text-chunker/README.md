# text-chunker

Splits long text, such as policies, manuals, knowledge-base articles, email threads or text from `pdf-to-json`, into overlapping chunks ready to embed and store in a vector database for retrieval (RAG). Chunks follow the document's structure: they break at Markdown headings, paragraphs, list items and sentences, never mid-word unless a single word is longer than a chunk, and each chunk carries the heading of its section.

## Input

One record or an array of records:

```json
{
  "doc_id": "returns-policy",
  "text": "# Returns\n\nItems must be returned within 30 days...\n\n## Refunds\n\n...",
  "_options": { "max_chars": 1500, "overlap_chars": 150, "id_field": "doc_id" }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the text | `text` |
| `max_chars` | Largest chunk, in characters, including the overlap (minimum 100) | `1500` |
| `overlap_chars` | How much of the previous chunk to repeat at the start of the next one, in whole sentences (at most half of `max_chars`) | `150` |
| `id_field` | A field whose value becomes `source_id`, with `chunk_id` set to `<id>-<index>` | |
| `keep_source` | Keep the full text in the output | `false` |

About 4 characters make one token for English text, so the 1,500-character default is roughly 375 tokens, a common size for retrieval.

## Output

```json
{
  "chunk_count": 4,
  "chunks": [
    { "index": 0, "chunk_id": "returns-policy-0", "source_id": "returns-policy", "heading": "Returns",
      "text": "# Returns\n\nReturn rule 0: ...", "chars": 420, "approx_tokens": 105, "start": 0, "end": 420, "overlap_chars": 0 },
    { "index": 1, "chunk_id": "returns-policy-1", "source_id": "returns-policy", "heading": "Returns",
      "text": "Return rule 4: ...", "chars": 163, "approx_tokens": 41, "start": 339, "end": 502, "overlap_chars": 82 }
  ],
  "ok": true
}
```

- `start` and `end` are character positions in the source text, for citations and highlighting.
- `overlap_chars` is how much of the chunk repeats the previous one. The first chunk of a section never overlaps into the previous section, so a chunk never mixes two topics.
- `chunk_id` is stable for the same text and settings, so re-running a document updates its chunks instead of duplicating them when you upsert by `chunk_id`.

In a flow: Unwind on `chunks`, embed each chunk's `text`, then insert or upsert it into your vector table with `chunk_id`, `source_id`, `heading` and `text`.

## Use it in a Neblex flow

1. Download `text-chunker.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `text-chunker`.
3. Add a step that uses the module's **Run module** operation after a document, PDF or HTML step that returns long text. Map it to `text`, and send the chunks on to an embeddings step.

## Build and test

```sh
../build.sh text-chunker
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
