# template

The smallest useful Neblex module: it reads JSON, adds a `_field_count` field to each record and returns it. Copy this folder to start your own module, then replace `transform` in `src/lib.rs`.

```sh
../build.sh template
```

## Use it in a Neblex flow

1. Build your module with `../build.sh <name>`; the `.wasm` file must stay small (a few hundred KB at most).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file.
3. Add a step that uses the module's **Run module** operation. Every field you map becomes part of the JSON the module receives.
