# VAG2MQTT

A standalone service that connects vehicles of the Volkswagen Group to MQTT.

VAG2MQTT talks to the manufacturer cloud services directly, normalises what it reads into a
brand independent vehicle model and publishes it over MQTT following the
[mqtt-smarthome](https://github.com/mqtt-smarthome/mqtt-smarthome) conventions. Audi is the
first supported brand; Volkswagen, Škoda, Seat and Cupra are planned.

> **Status: paused (since 2026-09-26).** Development is on hold until the manufacturer
> fixes its side. See [Why development is paused](#why-development-is-paused).

## Why development is paused

Audi's native app backend is closed to third parties, so VAG2MQTT reads Audi vehicles through
the official **EU Data Act portal** (`eu-data-act.drivesomethinggreater.com`). The login, the
session handling, vehicle discovery and the processing of data packages are built and tested.

What is missing is data. For the car this is being developed against (an Audi A6 e-tron on the
PPE platform), the portal delivers the continuous 15 minute data packages **permanently empty**
(`…_no_content_found.zip`). Only the one-time export, which can take up to 24 hours, carries
content. Other users report the same for PPE based Audis. Without continuous packages there is
nothing to publish, so work stays paused until Audi fixes the delivery.

What already works:

- login through the EU Data Act portal, with the session kept encrypted across restarts;
- discovery of the vehicles linked to the account;
- download and normalisation of data packages into the vehicle model (state of charge, range,
  odometer, charging, plug and climatisation);
- MQTT publishing following mqtt-smarthome, and the embedded admin interface.

## Design in one paragraph

One process, one binary, no plugins and no configuration file. Accounts, vehicles, the MQTT
broker and the polling interval are configured at runtime through a local web interface and
a JSON API, and every change takes effect without a restart. State lives in a SQLite
database in the data directory; credentials and tokens are encrypted at rest. A failing
account never stops the others.

## Requirements

- Rust (stable, 1.85 or newer, for edition 2024)
- On Windows: the MSVC build tools, as required by the default `x86_64-pc-windows-msvc` target

## Build and run

```sh
cargo build --workspace
cargo run -p vag2mqtt -- --help
```

## Bootstrap settings

Everything else is configured at runtime, not here.

| Environment variable | Flag | Default | Purpose |
|---|---|---|---|
| `VAG2MQTT_DATA_DIR` | `--data-dir` | `./data` | Database, master key, runtime state |
| `VAG2MQTT_LISTEN` | `--listen` | `127.0.0.1:8080` | Address of the admin API and UI |
| `VAG2MQTT_LOG` | `--log` | `info` | Log filter in `tracing` EnvFilter syntax |
| `VAG2MQTT_LOG_FORMAT` | `--log-format` | `pretty` | `pretty` or `json` |
| `VAG2MQTT_MASTER_KEY` | – | generated | Key for encrypting stored credentials |

The master key has no command line flag on purpose, so it cannot leak through `--help` or
the process list. When it is unset, a key file is created in the data directory on first
start. Back that file up: without it the stored credentials cannot be read.

## Checks

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
```

## Layout

| Path | Contents |
|---|---|
| `crates/vag2mqtt-domain` | Brand independent vehicle model, no I/O |
| `crates/vag2mqtt-connector-api` | The trait every brand connector implements |
| `crates/vag2mqtt-connector-audi` | Audi endpoints, authentication and normalisation |
| `crates/vag2mqtt-persistence` | SQLite repositories, migrations, secret encryption |
| `crates/vag2mqtt-mqtt` | mqtt-smarthome publisher and topic builder |
| `crates/vag2mqtt-runtime` | Account supervisor and polling scheduler |
| `crates/vag2mqtt-admin` | Admin API and embedded web UI |
| `vag2mqtt` | The binary; wiring only |
| `fixtures` | Anonymised manufacturer responses used by the tests |

## Relationship to other projects

The [CarConnectivity](https://github.com/tillsteinbach/CarConnectivity) family of projects is
used as a technical reference for authentication flows, endpoints and known quirks. VAG2MQTT
is not a port of it: it shares no architecture, object model or code, and it is written in
Rust rather than Python. Endpoints and parameters taken from a reference are documented with
their source in the code that uses them.

## How it was built

VAG2MQTT is developed with [Claude Code](https://claude.com/claude-code) as co-author: the code,
the tests and the reverse engineering notes were written in pair work between the maintainer and
Claude, Anthropic's AI model. Commits made that way carry a `Co-Authored-By: Claude` trailer.

## Licence

[MIT](LICENSE). Use it for anything, including commercially, as long as the
copyright notice travels with it.

Contributions are welcome, see [CONTRIBUTING.md](CONTRIBUTING.md).
