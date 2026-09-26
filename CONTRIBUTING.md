# Contributing to VAG2MQTT

Bug reports, reverse engineering findings, fixtures from your own car and pull
requests are all welcome.

## Licensing of contributions

This project is [MIT](LICENSE) licensed. By opening a pull request you confirm
that you wrote the contribution yourself or otherwise have the right to submit
it, and that it is contributed under the same MIT licence. You keep your
copyright.

That is all. There is no contributor licence agreement to sign.

## Before you send code

The rules that apply to everyone:

- **Crate layout.** `domain` depends on nothing internal; connectors depend only
  on `connector-api` and `domain`; `mqtt` never sees a brand specific type;
  `admin` depends on `runtime` and `persistence` only, and nothing depends on
  `admin`. The binary is wiring and the only place that names a brand crate.
- **Secrets never leak.** Passwords, tokens and cookies live in `Secret<T>` and
  never reach logs, errors, MQTT payloads, the UI or fixtures.
- **Three states, not one null.** Every vehicle value is *unsupported*,
  *unavailable* or *present*. Never invent a default value.
- **Tolerant parsing.** Manufacturer responses are parsed with every field
  optional and unknown fields ignored.

Run these and make sure they pass:

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

## Never commit

- Real VINs, tokens, passwords, coordinates, email addresses or names
- Unanonymised manufacturer responses
- Your data directory or master key file

Fixtures must be anonymised. If you are unsure whether a capture is clean, ask
in the issue before attaching it.

## Reverse engineering findings

If you work out an endpoint, a header or an authentication step, please document
it in the doc comment of the code that uses it, with a link to where you found
it. That keeps the manufacturer facing parts auditable when an API changes.
