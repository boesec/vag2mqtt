# Contributing to VAG2MQTT

Bug reports, reverse engineering findings, fixtures from your own car and pull
requests are all welcome.

## Licensing of contributions

This project is dual licensed: everyone gets the
[PolyForm Noncommercial License](LICENSE) for free, and commercial users buy a
separate licence. That only works if the maintainer holds the rights to the
whole codebase.

So by opening a pull request you confirm that:

1. You wrote the contribution yourself, or you have the right to submit it.
2. You grant the maintainer a perpetual, worldwide, irrevocable right to use
   your contribution and to license it to others, including under commercial
   terms that differ from the PolyForm licence.
3. You keep your own copyright. This is a licence grant, not an assignment.

If you cannot agree to that, please open an issue describing the problem and the
fix instead of sending code. A good description is still very useful.

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
