# Builds Sparkplug B test payloads with Google's protobuf library from the official
# sparkplug_b.proto (Eclipse Tahu), so the decoder is tested against an independent encoder.
# Usage (python:3.12-slim): pip install protobuf grpcio-tools, put sparkplug_b.proto next to this
# file (https://github.com/eclipse-tahu/tahu/blob/master/sparkplug_b/sparkplug_b.proto), then
#   python make_fixtures.py fixtures/payloads.json
import base64, json, os, struct, subprocess, sys

here = os.path.dirname(os.path.abspath(__file__))
subprocess.run([sys.executable, "-m", "grpc_tools.protoc", f"-I{here}", f"--python_out={here}", "sparkplug_b.proto"], check=True)
sys.path.insert(0, here)
import sparkplug_b_pb2 as pb

P = pb.Payload
T = {n: i for i, n in enumerate(["Unknown","Int8","Int16","Int32","Int64","UInt8","UInt16","UInt32","UInt64","Float","Double","Boolean","String","DateTime","Text","UUID","DataSet","Bytes","File","Template","PropertySet","PropertySetList","Int8Array","Int16Array","Int32Array","Int64Array","UInt8Array","UInt16Array","UInt32Array","UInt64Array","FloatArray","DoubleArray","BooleanArray","StringArray","DateTimeArray"])}

def java_bools(vals):  # Tahu Java / spec: count, then bits most significant first
    out = bytearray((len(vals) + 7) // 8)
    for i, v in enumerate(vals):
        if v: out[i // 8] |= 0x80 >> (i % 8)
    return struct.pack("<I", len(vals)) + bytes(out)

def python_bools(vals):  # Tahu Python array_packer: bits least significant first
    out = bytearray((len(vals) + 7) // 8)
    for i, v in enumerate(vals):
        out[i // 8] |= int(v) << (i % 8)
    return struct.pack("<I", len(vals)) + bytes(out)

def add(p, name, alias, dt, **kw):
    m = p.metrics.add()
    if name is not None: m.name = name
    if alias is not None: m.alias = alias
    if dt is not None: m.datatype = T[dt]
    for k, v in kw.items():
        setattr(m, k, v)
    return m

fixtures = []
def save(key, topic, p, note):
    fixtures.append({"key": key, "topic": topic, "payload": base64.b64encode(p.SerializeToString()).decode(), "note": note})

TS = 1764000000123  # 2025-11-24T16:00:00.123Z

# NBIRTH with every scalar type, signed values sign-extended like Tahu Java, UInt32 in long_value.
p = P(timestamp=TS, seq=0)
add(p, "bdSeq", 0, "Int64", long_value=3)
add(p, "Node Control/Rebirth", 1, "Boolean", boolean_value=False)
add(p, "Line1/Temp", 2, "Float", float_value=72.5)
m = add(p, "Line1/Pressure", 3, "Double", double_value=101.325)
m.properties.keys.extend(["engUnit", "engHigh", "documentation"])
v = m.properties.values.add(); v.type = T["String"]; v.string_value = "kPa"
v = m.properties.values.add(); v.type = T["Double"]; v.double_value = 150.0
v = m.properties.values.add(); v.type = T["String"]; v.is_null = True
add(p, "Line1/Offset", 4, "Int8", int_value=(-5) & 0xFFFFFFFF)
add(p, "Line1/Delta", 5, "Int16", int_value=(-300) & 0xFFFFFFFF)
add(p, "Line1/Count", 6, "Int32", int_value=(-70000) & 0xFFFFFFFF)
add(p, "Line1/Total", 7, "Int64", long_value=(-5000000000) & 0xFFFFFFFFFFFFFFFF)
add(p, "Line1/Mode", 8, "UInt8", int_value=200)
add(p, "Line1/Port", 9, "UInt16", int_value=65000)
add(p, "Line1/Serial", 10, "UInt32", long_value=4000000000)
add(p, "Line1/Energy", 11, "UInt64", long_value=18000000000000000000)
add(p, "Line1/Running", 12, "Boolean", boolean_value=True)
add(p, "Line1/Recipe", 13, "String", string_value="PVC-40mm")
add(p, "Line1/LastStart", 14, "DateTime", long_value=1763990000000)
add(p, "Line1/Notes", 15, "Text", string_value="Ölwechsel fällig")
add(p, "Line1/Batch", 16, "UUID", string_value="0f8e4b9a-3c1d-4e5f-9a7b-1c2d3e4f5a6b")
add(p, "Line1/Raw", 17, "Bytes", bytes_value=b"\x00\x01\xfe\xff")
add(p, "Line1/Alarm", 18, "Boolean", is_null=True)
save("nbirth_scalars", "spBv1.0/Plant1/NBIRTH/Edge01", p, "every scalar type, Java sign extension")

# DBIRTH with arrays (spec layout), a DataSet and a Template instance.
p = P(timestamp=TS, seq=1)
add(p, "Zones/Int8s", 20, "Int8Array", bytes_value=struct.pack("<3b", -1, 0, 127))
add(p, "Zones/Int16s", 21, "Int16Array", bytes_value=struct.pack("<2h", -32768, 32767))
add(p, "Zones/Temps", 22, "FloatArray", bytes_value=struct.pack("<3f", 20.5, 21.25, -3.5))
add(p, "Zones/Doubles", 23, "DoubleArray", bytes_value=struct.pack("<2d", 1.5, -2.25))
add(p, "Zones/UInt32s", 24, "UInt32Array", bytes_value=struct.pack("<2I", 0, 4294967295))
add(p, "Zones/Int64s", 25, "Int64Array", bytes_value=struct.pack("<2q", -9007199254740993, 42))
add(p, "Zones/Valves", 26, "BooleanArray", bytes_value=java_bools([False, False, True, True, False, True, False, False, True, True, False, True]))
add(p, "Zones/Names", 27, "StringArray", bytes_value=b"North\x00South\x00\x00East\x00")
add(p, "Zones/Times", 28, "DateTimeArray", bytes_value=struct.pack("<2q", 1763990000000, 1764000000000))
m = add(p, "Line1/Schedule", 29, "DataSet")
ds = m.dataset_value
ds.num_of_columns = 3
ds.columns.extend(["shift", "target", "active"])
ds.types.extend([T["String"], T["Int32"], T["Boolean"]])
for shift, target, active in [("A", 1200, True), ("B", -1, False)]:
    r = ds.rows.add()
    e = r.elements.add(); e.string_value = shift
    e = r.elements.add(); e.int_value = target & 0xFFFFFFFF
    e = r.elements.add(); e.boolean_value = active
m = add(p, "Line1/Motor", 30, "Template")
t = m.template_value
t.template_ref = "Motor"
t.version = "1.2"
t.is_definition = False
pr = t.parameters.add(); pr.name = "ratedKw"; pr.type = T["Float"]; pr.float_value = 7.5
sub = t.metrics.add(); sub.name = "Speed"; sub.datatype = T["Int16"]; sub.int_value = 1450
sub = t.metrics.add(); sub.name = "Faulted"; sub.datatype = T["Boolean"]; sub.boolean_value = False
save("dbirth_complex", "spBv1.0/Plant1/DBIRTH/Edge01/Extruder2", p, "arrays (spec), DataSet, Template")

# DDATA: aliases only, no names or data types (as Sparkplug recommends after BIRTH).
p = P(timestamp=TS + 1000, seq=2)
m = p.metrics.add(); m.alias = 4; m.int_value = (-7) & 0xFFFFFFFF; m.timestamp = TS + 900
m = p.metrics.add(); m.alias = 2; m.float_value = 73.0
m = p.metrics.add(); m.alias = 99; m.double_value = 1.0
save("ddata_aliases", "spBv1.0/Plant1/DDATA/Edge01/Extruder2", p, "aliases only")

# Arrays written by Tahu Python's array_packer (bits LSB first, hex strings).
p = P(timestamp=TS, seq=3)
add(p, "Valves", 1, "BooleanArray", bytes_value=python_bools([True, False, False, False, False, False, False, False, True]))
add(p, "Names", 2, "StringArray", bytes_value=b"".join(s.encode().hex().encode() + b"\x00" for s in ["Ab", "Zone 2"]))
save("python_arrays", "spBv1.0/Plant1/NDATA/Edge01", p, "Tahu Python array layout")

# NDEATH, historical + transient flags, payload body.
p = P(timestamp=TS)
add(p, "bdSeq", None, "Int64", long_value=3)
save("ndeath", "spBv1.0/Plant1/NDEATH/Edge01", p, "death certificate")
p = P(timestamp=TS, seq=9, uuid="custom-1", body=b"hello")
add(p, "Hist", None, "Int32", int_value=5, is_historical=True, is_transient=True, timestamp=TS - 60000)
save("flags_body", "spBv1.0/Plant1/NDATA/Edge01", p, "flags, uuid, body")

json.dump(fixtures, open(sys.argv[1], "w"), indent=1, ensure_ascii=False)
print(len(fixtures), "fixtures")
