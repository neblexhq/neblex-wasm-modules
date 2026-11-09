# html-to-text

Turns HTML into clean text or Markdown: HTML emails, web pages, rich-text fields from a CRM or help desk, and HTML descriptions in product feeds. It keeps the structure that matters (paragraphs, headings, lists, table rows, links) and drops what doesn't (scripts, styles, tracking pixels, comments), so the result is ready for a ticket, a log, a search index or an AI step.

## Input

One record or an array of records, each with the HTML:

```json
{ "html": "<html><head><title>Your order &amp; receipt</title></head><body><h1>Order update</h1><p>Your order <b>SO-1001</b> has shipped.</p>...</body></html>" }
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` | Which field holds the HTML | `html` |
| `format` | `"text"` or `"markdown"` | `"text"` |
| `inline_links` | In text, write each link's address after its text: `pricing (https://...)` | `false` |
| `keep_source` | Keep the HTML in the output | `false` |

## Output

```json
{
  "text": "Order update\n\nYour order SO-1001 has shipped.\n\n- Widget x 2\n- Bolt x 5\n\nTrack it here.\n\nItem | Qty\nWidget | 2",
  "title": "Your order & receipt",
  "links": [{ "text": "here", "url": "https://example.com/t/1" }],
  "ok": true
}
```

- Headings and paragraphs are separated by a blank line; list items start with `- ` or `1. ` and nest with two spaces; table cells are joined with ` | `, one row per line.
- With `"format": "markdown"` the result is `markdown` instead of `text`, with `#` headings, `**bold**`, `*italic*`, `[text](url)` links and fenced `pre` blocks.
- `links` lists every link with a real address (not `#anchors` or `javascript:`), in order.
- Named entities for common punctuation and all numeric entities (`&#8217;`, `&#x263A;`) are decoded. Image alt text is kept in place of the image.

Text-like content that isn't HTML passes through, and a stray `<` in text ("a < b") is kept as text.

## Build and test

```sh
../build.sh html-to-text
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

`test/sample.json` is a ready input for trying the module in a flow or with `../test/call.py`.
