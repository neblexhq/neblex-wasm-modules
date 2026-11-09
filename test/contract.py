# Checks every built module against the Neblex contract: a core module with no imports that
# exports memory, alloc and invoke, and returns valid JSON (no "ok": false) for its sample input.
# Usage: python test/contract.py <dir with .wasm files>
import json, os, sys, wasmtime

dist = sys.argv[1]
failed = 0
for name in sorted(d for d in os.listdir(".") if os.path.isfile(os.path.join(d, "Cargo.toml"))):
    path = os.path.join(dist, f"{name}.wasm")
    problems = []
    if not os.path.exists(path):
        problems.append("not built")
    else:
        engine = wasmtime.Engine()
        module = wasmtime.Module.from_file(engine, path)
        imports = [f"{i.module}.{i.name}" for i in module.imports]
        exports = {e.name for e in module.exports}
        if imports:
            problems.append(f"has imports {imports}")
        missing = {"memory", "alloc", "invoke"} - exports
        if missing:
            problems.append(f"missing exports {sorted(missing)}")
        sample = os.path.join(name, "test", "sample.json")
        if not problems and os.path.exists(sample):
            store = wasmtime.Store(engine)
            ex = wasmtime.Instance(store, module, []).exports(store)
            data = open(sample, "rb").read()
            p = ex["alloc"](store, len(data))
            ex["memory"].write(store, data, p)
            r = ex["invoke"](store, p, len(data)) & 0xFFFFFFFFFFFFFFFF
            out = json.loads(ex["memory"].read(store, r >> 32, (r >> 32) + (r & 0xFFFFFFFF)))
            items = out if isinstance(out, list) else [out]
            if any(isinstance(i, dict) and i.get("ok") is False for i in items):
                problems.append(f"sample returned ok: false: {out}")
    size = os.path.getsize(path) if os.path.exists(path) else 0
    print(f"{'FAIL' if problems else 'ok  '} {name} ({size:,} bytes) {'; '.join(problems)}")
    failed += bool(problems)
sys.exit(1 if failed else 0)
