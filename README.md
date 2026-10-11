# VAG2MQTT

A standalone service that connects vehicles of the Volkswagen Group to MQTT.

VAG2MQTT talks to the manufacturer cloud services directly, normalises what it reads into a
brand independent vehicle model and publishes it over MQTT following the
[mqtt-smarthome](https://github.com/mqtt-smarthome/mqtt-smarthome) conventions. Audi is the
first supported brand; Volkswagen, Škoda, Seat and Cupra are planned.

> **Status: early development, not released yet.** Development was paused from 2026-09-26
> to 2026-10-10 while the manufacturer's data packages arrived empty; they now carry data.

## Status

Audi's native app backend is closed to third parties, so VAG2MQTT reads Audi vehicles through
the official **EU Data Act portal** (`eu-data-act.drivesomethinggreater.com`). The portal
produces a data package about every 15 minutes; it is a read-only source, so there are no remote
commands.

What works today:

- login through the EU Data Act portal, with the session kept encrypted across restarts;
- discovery of the vehicles linked to the account;
- download of the 15 minute packages and normalisation into the vehicle model: state of charge,
  range, odometer, doors and locks, windows, outside temperature and service intervals;
- MQTT publishing following mqtt-smarthome, and the embedded admin interface.

In progress: the packages carry more than is published so far (tyre pressures, parking brake,
12 V battery and more), and a few values are not yet marked correctly as unsupported.

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

## Running as a systemd service

On a Linux host or container (for example a Debian LXC), after `cargo build --release`:

```sh
useradd --system --home-dir /var/lib/vag2mqtt --shell /usr/sbin/nologin vag2mqtt
install -m 0755 target/release/vag2mqtt /usr/local/bin/vag2mqtt
install -m 0644 packaging/systemd/vag2mqtt.service /etc/systemd/system/vag2mqtt.service
systemctl daemon-reload
systemctl enable --now vag2mqtt
journalctl -u vag2mqtt -f
```

State lives in `/var/lib/vag2mqtt`, including the master key; back it up. The unit listens on
`0.0.0.0:8080` so the admin interface is reachable from the LAN. It has no authentication yet,
so use it only in a trusted network, or set `VAG2MQTT_LISTEN=127.0.0.1:8080` in
`/etc/vag2mqtt/env`. Comments in the unit file explain the remaining settings and what to do if
the hardening options are refused inside an unprivileged container.

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
| `packaging` | systemd unit |
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
