# sparkplug-b-decoder

Decodes Sparkplug B MQTT messages into JSON, and encodes command messages to send back. Sparkplug B is the MQTT standard that PLC gateways, edge devices and SCADA systems use on the factory floor. Its payloads are binary protobuf, so they can't be read as JSON. This module turns them into metrics with names, data types, values and times, ready to store in a database or check against limits.

- Every Sparkplug B data type, including arrays, DataSets, Templates (UDTs) and property sets (engineering units, ranges)
- Topic parts: group, message type, edge node and device
- Metric aliases resolved with the map from the BIRTH message, including the data type, so signed values decode correctly
- Sequence gaps flagged, so a flow can request a rebirth
- One record per metric, for history tables
- NCMD/DCMD payloads built from JSON, to write setpoints or ask a node to rebirth

## Decode

Pass the MQTT topic and the payload as base64:

```json
{
  "topic": "spBv1.0/Plant1/DDATA/Edge01/Extruder2",
  "payload": "COPY37SrMxIPEAQY/9fftKszUPn///8PEgcQAmUAAJJCEgsQY2kAAAAAAADwPxgC",
  "_options": {
    "rows": true,
    "last_seq": 1,
    "aliases": {
      "2": { "name": "Line1/Temp", "datatype": "Float" },
      "4": { "name": "Line1/Offset", "datatype": "Int8" }
    }
  }
}
```

| `_options` key | Meaning | Default |
|---|---|---|
| `field` / `topic_field` | Fields with the payload and the topic | `payload` / `topic` |
| `encoding` | How the payload is passed: `base64`, `hex` or `text` | `base64` |
| `aliases` | Alias map saved from the node's BIRTH message (`{"4": {"name", "datatype"}}` or `{"4": "name"}`) | none |
| `last_seq` | The previous message's `seq`, to set `seq_gap` | none |
| `rows` | One record per metric instead of one record per message | `false` |
| `array_style` | `spec` (Sparkplug 3.0 and Eclipse Tahu Java) or `python` (Eclipse Tahu Python) | `spec` |
| `keep_payload` | Keep the base64 payload in the output | `false` |

### Output (one record per message)

```json
{
  "namespace": "spBv1.0",
  "group_id": "Plant1",
  "message_type": "NBIRTH",
  "edge_node_id": "Edge01",
  "device_id": null,
  "timestamp": 1764000000123,
  "time": "2025-11-24T16:00:00.123Z",
  "seq": 0,
  "metrics": [
    ...,
    {
      "name": "Line1/Pressure",
      "alias": 3,
      "datatype": "Double",
      "value": 101.325,
      "timestamp": null,
      "time": null,
      "properties": { "engUnit": "kPa", "engHigh": 150.0, "documentation": null }
    }
  ],
  "aliases": { ..., "3": { "name": "Line1/Pressure", "datatype": "Double" } },
  "ok": true
}
```

| Field | Meaning |
|---|---|
| `metrics[].value` | Numbers, text, `true`/`false`, arrays as JSON arrays, Bytes and File as base64, DateTime as epoch milliseconds (with `value_time` in ISO 8601) |
| DataSet value | `{ "columns": [...], "types": [...], "rows": [{ "shift": "A", "target": 1200, "active": true }] }` |
| Template value | `{ "template_ref": "Motor", "version": "1.2", "is_definition": false, "parameters": {...}, "metrics": [...] }` |
| `is_historical`, `is_transient`, `is_null` | Added to a metric only when set |
| `aliases` | On NBIRTH and DBIRTH: save this per edge node (a Data Table works) and pass it with later DATA messages |
| `seq_gap` | With `last_seq`: `true` when a message was missed (`seq` counts 0 to 255 and wraps; a BIRTH starts again) |
| `warning` | Metrics that arrived with only an alias that the map doesn't have |
| `state` | For STATE messages (`spBv1.0/STATE/host`): the JSON body, or `{"online": ...}` for the older ONLINE/OFFLINE text |

### Output with `rows: true`

Each metric becomes a record with the message fields copied on. A metric without its own timestamp takes the message's:

```json
[
  { "group_id": "Plant1", "message_type": "DDATA", "edge_node_id": "Edge01", "device_id": "Extruder2", "seq": 2, "seq_gap": false,
    "name": "Line1/Offset", "alias": 4, "datatype": "Int8", "value": -7, "timestamp": 1764000001023, "time": "2025-11-24T16:00:01.023Z", "ok": true },
  { "group_id": "Plant1", "message_type": "DDATA", "edge_node_id": "Edge01", "device_id": "Extruder2", "seq": 2, "seq_gap": false,
    "name": "Line1/Temp", "alias": 2, "datatype": "Float", "value": 73.0, "timestamp": 1764000001123, "time": "2025-11-24T16:00:01.123Z", "ok": true }
]
```

## Encode (NCMD and DCMD)

Set `_options.op` to `encode` and list the metrics with their data types:

```json
{
  "group_id": "Plant1", "message_type": "DCMD", "edge_node_id": "Edge01", "device_id": "Extruder2",
  "timestamp": 1764000000123,
  "metrics": [
    { "name": "Line1/Setpoint", "datatype": "Float", "value": 75.5 },
    { "name": "Node Control/Rebirth", "datatype": "Boolean", "value": true }
  ],
  "_options": { "op": "encode" }
}
```

The record comes back with `payload` (base64), `payload_size` and `topic` (`spBv1.0/Plant1/DCMD/Edge01/Extruder2`), ready for an MQTT publish step. Scalars, Bytes, File and all array types can be encoded; DataSets and Templates can't. Values are range-checked (`300` is refused for an Int8), and signed integers are written the way Eclipse Tahu Java writes them.

## Array layouts

Sparkplug 3.0 packs arrays into bytes. The spec and Eclipse Tahu Java write boolean arrays with the most significant bit first and string arrays as UTF-8 strings each ended by a zero byte. Eclipse Tahu Python writes booleans least significant bit first and hex-encodes the strings. The module reads the spec layout by default; set `array_style` to `python` for payloads from Tahu Python. Read with the wrong layout, the same bytes give different booleans and hex text, so check one known value when connecting a new source.

## Use it in a Neblex flow

1. Download `sparkplug-b-decoder.wasm` from the [latest release](https://github.com/neblexhq/neblex-wasm-modules/releases/latest).
2. In Neblex, open **Custom Integrations**, choose **Add Sandboxed Module** and upload the file. Name it `sparkplug-b-decoder`.
3. Start the flow with an MQTT stream trigger on `spBv1.0/#` (or one group, `spBv1.0/Plant1/#`). Add a step that uses the module's **Run module** operation with the topic and the payload as base64. To keep names on DATA messages, store `aliases` from each BIRTH and look it up before decoding.

## Build and test

```sh
../build.sh sparkplug-b-decoder
docker run --rm -v "$PWD":/src -w /src rust:1-slim cargo test --lib
```

The tests decode payloads built with Google's protobuf library from the official `sparkplug_b.proto` (see `test/make_fixtures.py` and `test/fixtures/payloads.json`): every scalar type, arrays, a DataSet, a Template, alias-only DATA, Tahu Python arrays, NDEATH, flags and STATE. Encoded payloads are checked by decoding them back. `test/sample.json` is a DDATA message with an alias map, decoded as rows.
