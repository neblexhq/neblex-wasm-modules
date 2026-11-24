# Contributing

Thanks for helping. Fixes, improvements and new modules are all welcome.

## Before you write code

For a new module or a change in behavior, [open an issue](https://github.com/neblexhq/neblex-wasm-modules/issues/new/choose) first and describe the problem, with an example input and the output you want. That way we can agree on the idea before you spend time on it. Small fixes (a typo, a clear bug with a test) can go straight to a pull request.

A module fits this repository when it:

- transforms the JSON it receives and nothing else. The sandbox has no network, file system, clock or host calls, so anything that needs to fetch, store or look something up belongs in a flow step instead.
- solves a problem more than one team is likely to have, such as a file format, an identifier, a common business rule.
- stays as small as the job allows. Prefer `serde_json` and the standard library; add a crate when it does the job clearly better.

## What a pull request needs

- **Code** in the module's `src/lib.rs`, in the style of the existing modules: the same `alloc` and `invoke` exports, settings in `_options`, errors returned as JSON (`{"ok": false, "error": "..."}`) instead of panics.
- **Tests:** unit tests in `src/lib.rs` (`cargo test --lib`), including edge cases and at least one input that must not match or must be rejected.
- **A sample:** `test/sample.json`, a complete input that works.
- **A README** in the module folder: what it does, the input and `_options`, an example output taken from a real run, and how to use it in a Neblex flow.
- **The build:** add a new module to the list in `build.sh` and to the table in the root `README.md`.

Check it locally before opening the pull request:

```sh
cd <module> && cargo test --lib && cd ..
./build.sh <module>
python -I test/contract.py dist
```

CI runs the same tests, builds every module and checks it against the module contract on every pull request.

## Review

We review pull requests on a best-effort basis. Changes to `vendor/`, `build.sh` and `.github/` always need a maintainer's review, because they affect how every module is built and released. A merged module is released with the others, with checksums and a build attestation, and maintained like the rest.

## License

By contributing, you agree that your contribution is licensed under the [MIT License](LICENSE), like the rest of this repository.
