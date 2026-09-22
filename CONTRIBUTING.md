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

Read [CLAUDE.md](CLAUDE.md). It is written for AI agents but it documents the
rules that apply to everyone: the crate layout and its dependency rules, the
handling of secrets, the three state value semantics, and the work package
workflow in `Docs/roadmap-items/`.

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
it under `Docs/reference/` with a link to where you found it, rather than only
putting it in code. That keeps the manufacturer facing parts auditable when an
API changes.
