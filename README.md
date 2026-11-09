# Neblex WebAssembly modules

[![CI](https://github.com/neblexhq/neblex-wasm-modules/actions/workflows/ci.yml/badge.svg)](https://github.com/neblexhq/neblex-wasm-modules/actions/workflows/ci.yml)

Sandboxed WebAssembly (WASM) modules for [Neblex Integration Fabric](https://www.neblex.com/integration-fabric). A module adds custom logic to an integration flow, such as parsing a file format or validating data, without running unreviewed code on your servers. Each module gets JSON in, returns JSON out, and cannot reach the network, the file system or anything else outside its own memory.

This repository has a starter template and thirteen ready-to-use modules, all written in Rust.

### Turn unstructured data into records

| Module | What it does | Size |
|---|---|---|
| [email-parser](email-parser) | Parses a raw email into sender, recipients, subject, date, thread IDs, body and attachments, and separates the new reply from quoted history and the signature | about 145 KB |
| [text-extractor](text-extractor) | Finds emails, URLs, phone numbers, dates, amounts, IBANs, parcel tracking numbers and PO or invoice numbers in free text, with check digits verified | about 165 KB |
| [pdf-to-json](pdf-to-json) | Extracts the text and metadata from a text-based PDF passed as base64 | about 175 KB |
| [json-repair](json-repair) | Turns almost-JSON from AI models (code fences, single quotes, trailing commas, cut-off output) into valid JSON and lists every fix | about 125 KB |
| [text-chunker](text-chunker) | Splits long text into overlapping chunks along headings, paragraphs and sentences, with stable IDs, ready to embed for retrieval (RAG) | about 100 KB |

### Read and write file and message formats

| Module | What it does | Size |
|---|---|---|
| [csv-cleaner](csv-cleaner) | Reads messy CSV exports: detects the delimiter, header row and decimal style, skips title and totals rows, and reports rows it couldn't read | about 135 KB |
| [xml-to-json](xml-to-json) | Converts XML (ERP messages, SOAP, exports) to JSON, optionally one record per repeating element with header fields copied on | about 160 KB |
| [fixed-width-parser](fixed-width-parser) | Turns fixed-width text files (mainframe extracts, bank and legacy ERP files) into records, including header/detail/trailer lines, implied decimals and COBOL signed numbers | about 115 KB |
| [fixed-width-writer](fixed-width-writer) | Writes records back out as a fixed-width file, using the same layout as the parser | about 165 KB |
| [gs1-barcode-parser](gs1-barcode-parser) | Splits GS1-128, DataMatrix and QR barcode scans into GTIN, batch, expiry, serial, weights and SSCC, with check digits verified | about 100 KB |

### Protect and compare

| Module | What it does | Size |
|---|---|---|
| [pii-redactor](pii-redactor) | Replaces emails, phone numbers, card numbers, SSNs, IBANs, IP addresses and names you list with placeholders before text reaches an AI model or a log | about 140 KB |
| [record-diff](record-diff) | Compares an incoming record with the stored version and returns only the changed fields, a patch and old/new values | about 120 KB |

### Validate

| Module | What it does | Size |
|---|---|---|
| [identifier-validator](identifier-validator) | Validates and normalizes IBANs, EU and UK VAT number formats, GTIN/EAN/UPC barcodes, US ABA routing numbers and LEIs on each record | about 105 KB |

### Start your own

| Module | What it does | Size |
|---|---|---|
| [template](template) | Minimal module to copy when writing your own | about 60 KB |

Each module works by rules, patterns and check digits, so results are repeatable and cost nothing per record. They handle predictable formats; route what they flag or can't read to an AI step or a person.

## Download

Built `.wasm` files for every module are attached to the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest). Download the one you need and upload it to Neblex; no build tools are required. To change a module, build it from source as described below.

Release files are built by GitHub Actions from the tagged source, after the tests and a contract check pass, so you can confirm what you're running:

- **Checksums:** each release includes `SHA256SUMS`. Check a download with `sha256sum -c SHA256SUMS --ignore-missing` (on macOS, `shasum -a 256 -c SHA256SUMS --ignore-missing`).
- **Build provenance:** each file has a signed attestation linking it to the commit and workflow that built it. Verify it with the GitHub CLI: `gh attestation verify email-parser.wasm --repo neblexhq/neblex-wasm-modules`.
- **Reproducible builds:** release files are built on x86-64 Linux with build paths mapped to fixed names. `./build.sh` uses the same settings (in an x86-64 container, emulated on other machines such as Apple silicon Macs), so building a tagged version with the Rust version named in its release notes gives files byte-identical to the release. Compare them with `SHA256SUMS`.

## Why WebAssembly for custom logic

Integration flows often need one step that no built-in step covers: a niche file format, a checksum, an industry rule. Running that as ordinary code means trusting it with whatever the runtime can reach. A WASM module is different:

- It has no imports, so it has no way to open a socket, read a file or call the host. It can only transform the JSON it receives.
- The runtime limits its memory, compute and run time.
- The same `.wasm` file runs in the Neblex cloud and on the on-premises runtime, so records from internal systems can be processed inside your network.

## The module contract

A Neblex module is a core WebAssembly module (not a component) that:

- has **no imports** (build for `wasm32-unknown-unknown`, not WASI);
- exports `memory`;
- exports `alloc(len: i32) -> i32`, which returns a pointer to `len` free bytes;
- exports `invoke(ptr: i32, len: i32) -> i64`, which reads the input JSON at `ptr`, writes the output JSON to memory and returns `(out_ptr << 32) | out_len`.

The input is the step's JSON (one record or an array of records). The output must be valid JSON. The modules here pass each record through and add their results as new fields, so later steps in the flow can filter or route on them.

Keep modules small. Neblex accepted modules up to about 680 KB in our tests and rejected larger ones. The modules here are 60 to 180 KB; avoid crates that pull in compression, crypto or Unicode tables you don't need. An early pdf-to-json built on a full PDF library was 1.4 MB.

## Build

You need Docker. No local Rust install is required.

```sh
./build.sh                         # all modules
./build.sh identifier-validator    # one module
```

The `.wasm` files land in `dist/`.

## Test locally

`test/call.py` calls a module exactly the way the runtime does: `alloc`, write the input, `invoke`, read the output.

```sh
docker run --rm -v "$PWD":/w -w /w python:3.12-slim sh -c \
  'pip install -q wasmtime && python test/call.py dist/identifier-validator.wasm "{\"iban\": \"DE89 3704 0044 0532 0130 00\"}"'
```

## Use a module in Neblex

1. In Neblex, open **Custom Integrations** and choose **Add Sandboxed Module**.
2. Give it a name (we use the folder name, such as `email-parser`) and upload the `.wasm` file from `dist/`. Neblex validates the module before saving it.
3. In a flow, add the module like any integration and choose its **Run module** operation. It needs no connection. Map the fields it should receive, including any `_options` or `_layout`, in the step's input; each module's `test/sample.json` shows a complete input.

## Write your own

Copy `template/`, rename the package in `Cargo.toml`, and replace the `transform` function in `src/lib.rs`. Use `serde_json` for input and output, and return an error as JSON (for example `{"ok": false, "error": "..."}`) instead of panicking.

## Links

- [Neblex Integration Fabric](https://www.neblex.com/integration-fabric)
- [Capabilities](https://www.neblex.com/integration-fabric/capabilities)
- [Reusable Rhai functions](https://github.com/neblexhq/neblex-rhai-modules): for lighter custom logic, a Rhai script in a Code step is quicker to write than a module
- [Neblex MCP server](https://github.com/neblexhq/neblex-mcp)

## License

MIT. See [LICENSE](LICENSE).
