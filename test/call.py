# Calls a module the way the Neblex runtime does: alloc -> write JSON -> invoke -> unpack i64.
# Usage: python test/call.py dist/<module>.wasm '<json>'   (or pipe JSON on stdin)
import json, sys, wasmtime

engine = wasmtime.Engine()
module = wasmtime.Module.from_file(engine, sys.argv[1])
assert not list(module.imports), f"module must have no imports: {[f'{i.module}.{i.name}' for i in module.imports]}"
store = wasmtime.Store(engine)
ex = wasmtime.Instance(store, module, []).exports(store)
data = (sys.argv[2] if len(sys.argv) > 2 else sys.stdin.read()).encode()
p = ex["alloc"](store, len(data))
ex["memory"].write(store, data, p)
r = ex["invoke"](store, p, len(data)) & 0xFFFFFFFFFFFFFFFF
out = ex["memory"].read(store, r >> 32, (r >> 32) + (r & 0xFFFFFFFF))
print(json.dumps(json.loads(out), indent=2))
