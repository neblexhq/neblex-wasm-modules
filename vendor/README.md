# Patched third-party crates

A Neblex module must be a WebAssembly file with no imports: it can't call the host for the time, random numbers or anything else. A few crates used by the larger modules (decision-table, json-schema-validator, spreadsheet-calc) reach for JavaScript functions when compiled for `wasm32-unknown-unknown`, because they assume they'll run in a browser. This folder holds copies of those crates with the smallest changes needed to remove those calls. Each module points at them with `[patch.crates-io]` in its `Cargo.toml`.

Every copy is the crate as published on crates.io at the version in its folder name, with its original license files. Changed places are marked with a "Patched" comment.

| Crate | License | Change | Why |
|---|---|---|---|
| getrandom 0.3.4 and 0.4.3 | MIT or Apache-2.0 | On `wasm32-unknown-unknown`, use the module's own random source (`__getrandom_v03_custom`) instead of the browser's Web Crypto API; the `wasm_js` feature does nothing | Hash tables and RAND() need random bytes. Each module supplies a fixed-seed generator, so results repeat exactly |
| chrono 0.4.45 | MIT or Apache-2.0 | The `wasmbind` feature (JavaScript `Date`) is disabled | The modules take the current time from the `now` option instead of asking the host |
| iana-time-zone 0.1.65 | MIT or Apache-2.0 | On `wasm32-unknown-unknown` the local time zone is UTC instead of the browser's | No host to ask; modules take a `timezone` option where it matters |
| zen-expression 2.1.8 | MIT | `now` is set by the module (`set_utc_now_millis`) instead of read from the clock | Same as chrono: the time comes from the `now` option |
| ironcalc_base 0.9.1 | MIT or Apache-2.0 | The browser-only code paths (JavaScript time zones, random numbers and clock) are replaced by the crate's own non-browser paths, using `regex-lite` as the browser build does; the build script uses the crate version instead of asking git; the upstream test suite is left out | The engine then needs nothing from the host, and the build doesn't depend on this repository's git state |

To check a patch, compare a folder with the published crate, for example:

```sh
curl -sL https://crates.io/api/v1/crates/chrono/0.4.45/download | tar xz
diff -r chrono-0.4.45 vendor/chrono-0.4.45
```

The diff also lists `Cargo.toml.orig` and `.cargo_vcs_info.json`, packaging files left out of these copies.

These modules are 1.5 to 3 MB because they carry a full engine (a spreadsheet calculator, an expression language, a JSON Schema validator). The other modules in this repository need none of this and stay small.
