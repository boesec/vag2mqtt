//! The publisher the runtime talks to, so runtime tests need no broker.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use vag2mqtt_domain::{
    AccountConnectionState, AccountId, MqttConfig, Secret, VehicleDataState, VehicleState, Vin,
};
use vag2mqtt_mqtt::{MqttHandle, MqttPublisher, ServiceInfo, VehicleMeta};

use crate::error::RuntimeError;

/// The `lc` map to persist after a state publish.
pub type StatePublishReport = BTreeMap<String, i64>;

/// What the account tasks and the supervisor need from a publisher. Implemented by the MQTT
/// handle of WP-04 and by recording fakes in tests.
pub trait Publisher: Send + Sync {
    /// Publishes a snapshot; returns the `lc` map to persist.
    fn publish_state(
        &self,
        meta: &VehicleMeta,
        state: &VehicleState,
    ) -> Result<StatePublishReport, RuntimeError>;
    /// Publishes the vehicle's availability.
    fn publish_vehicle_availability(
        &self,
        vin: &Vin,
        state: VehicleDataState,
        now: DateTime<Utc>,
    ) -> Result<(), RuntimeError>;
    /// Publishes the account's availability.
    fn publish_account_availability(
        &self,
        id: &AccountId,
        state: AccountConnectionState,
        now: DateTime<Utc>,
    ) -> Result<(), RuntimeError>;
    /// Seeds `lc` for a vehicle from persistence before its first publish.
    fn seed_last_changes(&self, vin: &Vin, last_changes: &BTreeMap<String, i64>);
    /// Clears every retained topic of the vehicle.
    fn clear_vehicle(&self, vin: &Vin) -> Result<(), RuntimeError>;
    /// Clears the account's availability topic.
    fn clear_account(&self, id: &AccountId) -> Result<(), RuntimeError>;
    /// `connected = 2` while true, `1` otherwise.
    fn set_operational(&self, operational: bool) -> Result<(), RuntimeError>;
    /// `true` while connected to the broker.
    fn is_connected(&self) -> bool;
    /// Publishes `connected = 0` and disconnects.
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

impl Publisher for MqttHandle {
    fn publish_state(
        &self,
        meta: &VehicleMeta,
        state: &VehicleState,
    ) -> Result<StatePublishReport, RuntimeError> {
        Ok(MqttHandle::publish_state(self, meta, state)?.last_changes)
    }

    fn publish_vehicle_availability(
        &self,
        vin: &Vin,
        state: VehicleDataState,
        now: DateTime<Utc>,
    ) -> Result<(), RuntimeError> {
        Ok(MqttHandle::publish_vehicle_availability(
            self, vin, state, now,
        )?)
    }

    fn publish_account_availability(
        &self,
        id: &AccountId,
        state: AccountConnectionState,
        now: DateTime<Utc>,
    ) -> Result<(), RuntimeError> {
        Ok(MqttHandle::publish_account_availability(
            self, id, state, now,
        )?)
    }

    fn seed_last_changes(&self, vin: &Vin, last_changes: &BTreeMap<String, i64>) {
        MqttHandle::seed_last_changes(self, vin, last_changes);
    }

    fn clear_vehicle(&self, vin: &Vin) -> Result<(), RuntimeError> {
        MqttHandle::clear_vehicle(self, vin)?;
        Ok(())
    }

    fn clear_account(&self, id: &AccountId) -> Result<(), RuntimeError> {
        Ok(MqttHandle::clear_account(self, id)?)
    }

    fn set_operational(&self, operational: bool) -> Result<(), RuntimeError> {
        Ok(MqttHandle::set_operational(self, operational)?)
    }

    fn is_connected(&self) -> bool {
        MqttHandle::is_connected(self)
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(MqttHandle::shutdown(self))
    }
}

/// Creates a publisher for a broker configuration. The real one spawns the WP-04 task; tests
/// hand in recording fakes.
pub type PublisherFactory = Arc<
    dyn Fn(&MqttConfig, Option<&Secret<String>>) -> Result<Arc<dyn Publisher>, RuntimeError>
        + Send
        + Sync,
>;

/// The factory that spawns the real MQTT publisher.
pub fn mqtt_publisher_factory(info: ServiceInfo) -> PublisherFactory {
    Arc::new(move |config, password| {
        let (handle, _task) = MqttPublisher::spawn(config, password, info.clone())?;
        Ok(Arc::new(handle) as Arc<dyn Publisher>)
    })
}
