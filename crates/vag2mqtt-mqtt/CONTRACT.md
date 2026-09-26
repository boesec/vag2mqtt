# VAG2MQTT MQTT contract, version 1

The wire contract between VAG2MQTT and its consumers. It follows the mqtt-smarthome 2.0
convention (<https://github.com/mqtt-smarthome/mqtt-smarthome>). Renaming a topic or changing a payload shape
raises the major version; adding a topic does not. The version is published in `<prefix>/info`
and in every `full` payload. Frozen at version 1 with the first release.

The flattening code in `crates/vag2mqtt-mqtt/src/flatten.rs` is kept in sync with the vehicle
table below by a test.

## Conventions

- `<prefix>` defaults to `vag2mqtt` and is configurable.
- Every topic under `status/` is **retained** and published with **QoS 1**.
- Every `status/*` value (except `full`) is a JSON object `{"val": <scalar>, "ts": <ms>, "lc": <ms>}`:
  - `val`: the value as a JSON scalar (number, string or boolean).
  - `ts`: when the value was obtained, **milliseconds since the Unix epoch**. This is the
    manufacturer's own timestamp for the value where it delivers one, else the time VAG2MQTT
    fetched the snapshot.
  - `lc`: when the value last changed as seen by VAG2MQTT, milliseconds since the epoch.
    `lc <= ts`. Survives restarts.
- Units are fixed per topic (table below) and never part of the payload. Metric only.
- A value the vehicle does not support is **never published**. A value that is temporarily
  unavailable is **not republished**; the last retained value stays and `availability` tells the
  consumer that it is old.
- Timestamps outside `{val,ts,lc}` (in `full` and `info`) are RFC 3339 in UTC.

## Service level topics

| Topic | Retained | Payload |
|---|---|---|
| `<prefix>/connected` | yes, last will `0` | `0` not running, `1` connected to the broker but no account delivers fresh data, `2` at least one account delivers fresh data. Plain text. |
| `<prefix>/info` | yes | `{"name":"vag2mqtt","version":"0.1.0","spec":"mqtt-smarthome 2.0","contract_version":1,"started_at":"2026-09-22T10:00:00Z"}` |
| `<prefix>/status/account/<id>/availability` | yes | `{"val":"ok","ts":1790244000000,"lc":1790244000000}`; `val` from `pending`, `ok`, `auth_error`, `rate_limited`, `unreachable`, `error`, `disabled` |
| `<prefix>/maintenance/stats` | yes | JSON with runtime statistics, not published yet |
| `<prefix>/maintenance/set/loglevel` | no, subscribed | Plain text `tracing` filter, for example `info,vag2mqtt::mqtt=debug` (not acted on yet) |
| `<prefix>/set/<VIN>/<command>` | no, subscribed | Vehicle commands, planned. Until then received and ignored with a log line. |

## Vehicle topics

All under `<prefix>/status/<VIN>/`. The VIN is upper case, seventeen characters.

| Path | `val` | Unit | Example `val` |
|---|---|---|---|
| `odometer` | integer | km | `12345` |
| `battery/soc` | integer 0 to 100 | % | `80` |
| `battery/range` | integer | km | `410` |
| `fuel/level` | integer 0 to 100 | % | `55` |
| `fuel/range` | integer | km | `620` |
| `charging/state` | `off`, `ready_for_charging`, `charging`, `conservation`, `discharging`, `error`, `unknown` | – | `"charging"` |
| `charging/mode` | `manual`, `timer`, `preferred_times`, `off`, `unknown` | – | `"manual"` |
| `charging/current_type` | `ac`, `dc`, `unknown` | – | `"ac"` |
| `charging/power` | number | kW | `11.0` |
| `charging/remaining_time` | integer | min | `95` |
| `charging/target_soc` | integer 0 to 100 | % | `90` |
| `plug/connection` | `connected`, `disconnected`, `unknown` | – | `"connected"` |
| `plug/lock` | `locked`, `unlocked`, `unknown` | – | `"locked"` |
| `plug/external_power` | `available`, `active`, `unavailable`, `unknown` | – | `"active"` |
| `doors/open` | `open`, `closed`, `ajar`, `unknown` (any door) | – | `"closed"` |
| `doors/lock` | `locked`, `unlocked`, `unknown` (central lock) | – | `"locked"` |
| `doors/<position>/open` | as `doors/open`; `<position>` is one of `front_left`, `front_right`, `rear_left`, `rear_right`, `trunk`, `bonnet` | – | `"closed"` |
| `windows/open` | `open`, `closed`, `ajar`, `unknown` (any window) | – | `"closed"` |
| `windows/<position>/open` | as `windows/open`; `<position>` is one of `front_left`, `front_right`, `rear_left`, `rear_right`, `sun_roof` | – | `"closed"` |
| `position/latitude` | number, −90 to 90 | ° | `51.0` |
| `position/longitude` | number, −180 to 180 | ° | `9.0` |
| `position/heading` | number, 0 to 360, only when the manufacturer delivers it | ° | `180.0` |
| `climatisation/state` | `off`, `heating`, `cooling`, `ventilation`, `unknown` | – | `"heating"` |
| `climatisation/target_temperature` | number | °C | `21.5` |
| `climatisation/remaining_time` | integer | min | `20` |
| `status/connection` | `online`, `offline`, `unknown` | – | `"online"` |
| `status/activity` | `parked`, `ignition_on`, `driving`, `unknown` | – | `"parked"` |
| `status/secured` | boolean (locked and everything closed) | – | `true` |
| `status/outside_temperature` | number | °C | `14.0` |
| `service/inspection_due_days` | integer, negative when overdue | d | `120` |
| `service/inspection_due_km` | integer, negative when overdue | km | `-150` |
| `service/oil_service_due_days` | integer, negative when overdue | d | `200` |
| `service/oil_service_due_km` | integer, negative when overdue | km | `8000` |
| `availability` | `pending`, `fresh`, `stale`, `unavailable`, `error`, `disabled` | – | `"fresh"` |
| `full` | JSON object, see below; **not** `{val,ts,lc}` | – | – |

Example scalar payload on `vag2mqtt/status/WAUZZZ0000000TEST/battery/soc`:

```json
{"val": 80, "ts": 1790244120000, "lc": 1790240520000}
```

### `full`

The lossless view: the whole normalised snapshot in its three-state form plus identity and
timestamps. The scalar topics above are the convenient view.

```json
{
  "contract_version": 1,
  "vin": "WAUZZZ0000000TEST",
  "brand": "audi",
  "model": "A6 e-tron",
  "display_name": "Family car",
  "fetched_at": "2026-09-22T10:30:00Z",
  "published_at": "2026-09-22T10:30:01Z",
  "state": {
    "fetched_at": "2026-09-22T10:30:00Z",
    "odometer": {"state": "present", "value": 12345, "source_time": "2026-09-22T10:01:00Z"},
    "battery": {
      "soc": {"state": "present", "value": 80, "source_time": "2026-09-22T10:02:00Z"},
      "range": {"state": "present", "value": 410, "source_time": "2026-09-22T10:02:00Z"}
    },
    "fuel": {"level": {"state": "unsupported"}, "range": {"state": "unsupported"}},
    "...": "every category from crates/vag2mqtt-domain/tests/fixtures/vehicle_state_full.json"
  }
}
```

Each value inside `state` is one of `{"state":"unsupported"}`, `{"state":"unavailable"}` or
`{"state":"present","value":…,"source_time":"<RFC 3339>"|null}`.

## Lifecycle

- On every connect the service publishes `connected` and `info`, then subscribes to `set/#` and
  `maintenance/set/#`.
- On graceful shutdown it publishes `connected = 0` before disconnecting; on a crash the broker
  publishes the last will `0`.
- When a vehicle or account is deleted, every retained topic of it is cleared with an empty
  retained publish.
- When the broker configuration changes, the service disconnects (publishing `connected = 0`),
  connects to the new broker and republishes every vehicle's last snapshot and availability.
