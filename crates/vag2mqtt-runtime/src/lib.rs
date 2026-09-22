//! The supervisor that keeps one isolated task per active account.
//!
//! Responsibilities: authenticate, discover vehicles, poll on a configurable interval,
//! back off on transient errors, pause on rate limits and apply configuration changes
//! without a process restart (requirements FR-003 to FR-006, FR-013, ER-001 to ER-005).
//!
//! Configuration changes arrive as commands over a channel; the admin layer never touches
//! connector tasks directly. A failure in one account never blocks another.
//!
//! Implemented by WP-05.
