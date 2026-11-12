# Calls the module exactly like the Worker contract: alloc -> write JSON -> invoke -> unpack i64.
import base64, glob, json, os, sys, zlib, wasmtime

def build_pdf(page_streams, compress=False, info=True, a85=False):
    objs = ["<< /Type /Catalog /Pages 2 0 R >>", None,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    kids = []
    for content in page_streams:
        data = content.encode()
        if compress:
            data = zlib.compress(data)
        if a85:  # as ReportLab writes it: ASCII85 over Flate, no "<~" prefix
            data = base64.a85encode(data, adobe=True)[2:]
        filt = " /Filter [/ASCII85Decode /FlateDecode]" if a85 else " /Filter /FlateDecode" if compress else ""
        objs.append((f"<< /Length {len(data)}" + filt + " >>", data))
        objs.append(f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {len(objs)} 0 R /Resources << /Font << /F1 3 0 R >> >> >>")
        kids.append(f"{len(objs)} 0 R")
    objs[1] = f"<< /Type /Pages /Kids [{' '.join(kids)}] /Count {len(kids)} >>"
    objs.append("<< /Title (Invoice INV-1001) /Author (Acme Corp) >>")
    out, offs = b"%PDF-1.4\n", []
    for i, o in enumerate(objs, 1):
        offs.append(len(out))
        if isinstance(o, tuple):
            out += f"{i} 0 obj\n{o[0]}\nstream\n".encode() + o[1] + b"\nendstream\nendobj\n"
        else:
            out += f"{i} 0 obj\n{o}\nendobj\n".encode()
    x = len(out)
    out += f"xref\n0 {len(objs)+1}\n0000000000 65535 f \n".encode() + "".join(f"{o:010d} 00000 n \n" for o in offs).encode()
    out += f"trailer\n<< /Size {len(objs)+1} /Root 1 0 R" .encode() + (f" /Info {len(objs)} 0 R".encode() if info else b"") + f" >>\nstartxref\n{x}\n%%EOF\n".encode()
    return out

engine = wasmtime.Engine()
module = wasmtime.Module.from_file(engine, sys.argv[1])
print("size:", os.path.getsize(sys.argv[1]), "imports:", [f"{i.module}.{i.name}" for i in module.imports], "exports:", [e.name for e in module.exports])

def call(payload):
    store = wasmtime.Store(engine)
    ex = wasmtime.Instance(store, module, []).exports(store)
    data = json.dumps(payload).encode()
    p = ex["alloc"](store, len(data)); ex["memory"].write(store, data, p)
    r = ex["invoke"](store, p, len(data)) & 0xFFFFFFFFFFFFFFFF
    op, ol = r >> 32, r & 0xFFFFFFFF
    return json.loads(ex["memory"].read(store, op, op + ol))

b64 = lambda b: base64.b64encode(b).decode()
p1 = "BT /F1 14 Tf 72 720 Td (Invoice INV-1001) Tj 0 -20 Td (Bill to: Globex) Tj 0 -20 Td [(Total:) -300 (1,250.00) -300 (USD)] TJ ET"
p2 = "BT /F1 12 Tf 1 0 0 1 72 700 Tm (Line one) Tj 1 0 0 1 72 680 Tm (Line two) Tj T* (Line three) Tj ET"

print("--- uncompressed, 1 page");   print(json.dumps(call({"order_id": 7, "attachment": {"content": b64(build_pdf([p1]))}}), indent=1))
print("--- flate, 2 pages");          r = call({"file": "data:application/pdf;base64," + b64(build_pdf([p1, p2], compress=True))}); print(r["page_count"], r["metadata"]); print(r["text"])
print("--- ascii85 + flate");    r = call({"file": b64(build_pdf([p1], compress=True, a85=True))}); print(r["has_text"]); print(r["text"]); assert r["has_text"] and "INV-1001" in r["text"]
print("--- no pdf");                  print(call({"nothing": "here"}))
print("--- garbage");                 print(call({"file": "JVBERi0xLjQKZ2FyYmFnZQ=="}))
print("--- not json input handled by module? (raw text)")

for f in sorted(glob.glob("/pdfs/*.pdf"))[:6]:
    data = open(f, "rb").read()
    if len(data) > 8_000_000: continue
    r = call({"file": b64(data)})
    print(f"=== {os.path.basename(f)} ({len(data)//1024} KB): ok={r.get('ok')} pages={r.get('page_count')} has_text={r.get('has_text')} err={r.get('error')}")
    if r.get("ok"): print(r["text"][:400].replace("\n", " | "))
