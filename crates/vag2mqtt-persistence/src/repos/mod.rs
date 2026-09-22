//! Repositories, one module per table group. Each is a zero-cost view on the [`Database`](crate::Database).

pub(crate) mod accounts;
pub(crate) mod mqtt;
pub(crate) mod secrets;
pub(crate) mod settings;
pub(crate) mod vehicle_states;
pub(crate) mod vehicles;
