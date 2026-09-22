//! MQTT publishing following the mqtt-smarthome conventions.
//!
//! This crate owns the topic builder, the publisher and the availability handling:
//! `<prefix>/connected` as a retained tri-state and last will, `<prefix>/status/...` as
//! retained `{"val", "ts", "lc"}` payloads, `<prefix>/set/...` for later commands
//! (requirements section 9, NFR-011).
//!
//! Topic strings are assembled in exactly one place so that the published contract stays
//! stable and reviewable. The crate sees [`vag2mqtt_domain`] types only, never a brand
//! specific one.
//!
//! Implemented by WP-04.
