# EU Data Act export fixtures

## `one_time_export.json`

A one-time export of an Audi A6 e-tron from the EU Data Act portal, taken on 2026-09-24. What the
fields mean is documented in `crates/vag2mqtt-connector-audi/src/export/normalise.rs`.

Changed from the original:

- **Anonymised.** The VIN is `WAUZZZ0000000TEST`, the user id is all zeros, and every per-user or
  per-event identifier (vehicle identifier, session, message, tracking, transaction and timer ids)
  is replaced by a numbered placeholder. The row `key`s are kept: they are name-derived UUIDs that
  identify fields, not people.
- **Trimmed** from 5342 to 295 rows. The charge power curves keep sessions 0 and 1 with their first
  four points each; the trip log keeps the three most recent trips. Every other row is unchanged,
  including the odd timestamps (`N/A`, missing, zone-less, and seconds stored as milliseconds),
  because a parser has to survive exactly those.

The export contains no position data. The raw original never enters the repository; `/*.json` in
the root is ignored for that reason.
