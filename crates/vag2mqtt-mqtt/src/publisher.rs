//! The rumqttc client task and the handle the runtime publishes through.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rumqttc::{
    AsyncClient, ClientError, ConnectReturnCode, Event, EventLoop, LastWill, MqttOptions, Outgoing,
    Packet, QoS, Transport,
};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use vag2mqtt_domain::{
    AccountConnectionState, AccountId, MqttConfig, MqttProtocol, Secret, VehicleDataState,
    VehicleState, Vin,
};

use crate::error::MqttError;
use crate::flatten::flatten;
use crate::last_change::LastChangeTracker;
use crate::payload::{FullPayload, InfoPayload, ServiceInfo, StatePayload, VehicleMeta, to_ms};
use crate::topic::TopicBuilder;

const QOS: QoS = QoS::AtLeastOnce;
const REQUEST_QUEUE: usize = 1024;
const INCOMING_QUEUE: usize = 64;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const LOG_TARGET: &str = "vag2mqtt::mqtt";

/// A message that arrived on one of the subscribed `set` topics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Incoming {
    /// `<prefix>/set/<VIN>/<command>`; received and ignored until commands exist.
    SetCommand {
        /// The VIN segment as received (not validated).
        vin: String,
        /// The command segment.
        command: String,
        /// The raw payload.
        payload: Vec<u8>,
    },
    /// `<prefix>/maintenance/set/loglevel`; not acted on yet.
    LogLevel(String),
    /// Anything else under the subscriptions.
    Other {
        /// The topic.
        topic: String,
        /// The raw payload.
        payload: Vec<u8>,
    },
}

/// What [`MqttHandle::publish_state`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishReport {
    /// Number of scalar topics published (without `full`).
    pub published: usize,
    /// The `lc` per path under `status/<VIN>/`, for persistence.
    pub last_changes: BTreeMap<String, i64>,
}

struct Inner {
    client: AsyncClient,
    topics: TopicBuilder,
    tracker: Mutex<LastChangeTracker>,
    operational: AtomicBool,
    connected: AtomicBool,
    stopping: AtomicBool,
    incoming: broadcast::Sender<Incoming>,
}

impl Inner {
    fn tracker(&self) -> MutexGuard<'_, LastChangeTracker> {
        // Never held across an await. A poisoned lock means a panic while tracking, and the
        // tracker's state is still consistent enough to continue.
        self.tracker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn try_publish(&self, topic: &str, retain: bool, payload: Vec<u8>) -> Result<(), MqttError> {
        self.client
            .try_publish(topic, QOS, retain, payload)
            .map_err(|error| self.map_client_error(error))
    }

    /// rumqttc hands back the rejected request without saying whether the queue was full or
    /// closed; the stop flag tells the two apart.
    fn map_client_error(&self, error: ClientError) -> MqttError {
        if self.stopping.load(Ordering::SeqCst) {
            return MqttError::Stopped;
        }
        match error {
            ClientError::TryRequest(_) => MqttError::Backpressure,
            ClientError::Request(_) => MqttError::Stopped,
        }
    }

    fn publish_json<T: Serialize>(
        &self,
        topic: &str,
        retain: bool,
        value: &T,
    ) -> Result<(), MqttError> {
        let payload = serde_json::to_vec(value).map_err(|_| MqttError::Serialisation {
            topic: topic.to_string(),
        })?;
        self.try_publish(topic, retain, payload)
    }

    fn connected_value(&self) -> &'static str {
        if self.operational.load(Ordering::SeqCst) {
            "2"
        } else {
            "1"
        }
    }
}

/// Spawns the publisher task.
pub struct MqttPublisher;

impl MqttPublisher {
    /// Connects to the broker described by `config` and returns the handle and the task.
    ///
    /// The task reconnects on its own until [`MqttHandle::shutdown`] is called. `connected` and
    /// `info` are published on every successful connect.
    pub fn spawn(
        config: &MqttConfig,
        password: Option<&Secret<String>>,
        info: ServiceInfo,
    ) -> Result<(MqttHandle, JoinHandle<()>), MqttError> {
        let topics = TopicBuilder::new(config.topic_prefix.clone())?;

        if config.protocol == MqttProtocol::V5 {
            tracing::warn!(
                target: LOG_TARGET,
                "MQTT 5 is not implemented yet; connecting with MQTT 3.1.1"
            );
        }

        let mut options =
            MqttOptions::new(config.client_id.clone(), config.host.clone(), config.port);
        options.set_keep_alive(config.keep_alive);
        options.set_clean_session(true);
        options.set_last_will(LastWill::new(topics.connected(), "0", QOS, true));
        if let Some(username) = &config.username {
            let password = password
                .map(|p| p.expose_secret().clone())
                .unwrap_or_default();
            options.set_credentials(username.clone(), password);
        }
        if config.tls {
            // rumqttc is built without a crypto provider so that no crate in the graph forces
            // aws-lc-rs; ring is installed once here. A second install is a harmless error.
            let _ = rustls::crypto::ring::default_provider().install_default();
            options.set_transport(Transport::tls_with_default_config());
        }

        let (client, eventloop) = AsyncClient::new(options, REQUEST_QUEUE);
        let (incoming, _) = broadcast::channel(INCOMING_QUEUE);
        let inner = Arc::new(Inner {
            client,
            topics,
            tracker: Mutex::new(LastChangeTracker::new()),
            operational: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            incoming,
        });
        let task = tokio::spawn(run(eventloop, Arc::clone(&inner), info));
        Ok((MqttHandle { inner }, task))
    }
}

/// The cheap, clonable handle the runtime publishes through.
#[derive(Clone)]
pub struct MqttHandle {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for MqttHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MqttHandle")
            .field("prefix", &self.inner.topics.prefix())
            .field("is_connected", &self.is_connected())
            .finish()
    }
}

impl MqttHandle {
    /// The topic builder for this connection.
    pub fn topics(&self) -> &TopicBuilder {
        &self.inner.topics
    }

    /// `true` while the client has a live session with the broker.
    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::SeqCst)
    }

    /// Publishes every present value of the snapshot as `status/<VIN>/<path>` and the `full`
    /// payload, all retained. Returns the `lc` map to persist.
    pub fn publish_state(
        &self,
        meta: &VehicleMeta,
        state: &VehicleState,
    ) -> Result<PublishReport, MqttError> {
        let inner = &self.inner;
        let values = flatten(state);
        let mut published = 0;
        for value in &values {
            let topic = inner.topics.status(&meta.vin, &value.path);
            let ts = to_ms(value.ts);
            let lc = inner.tracker().observe(&topic, &value.val, ts);
            let payload = StatePayload {
                val: value.val.clone(),
                ts,
                lc,
            };
            inner.publish_json(&topic, true, &payload)?;
            published += 1;
        }
        let full_topic = inner.topics.full(&meta.vin);
        let full = FullPayload::new(meta, state, Utc::now());
        inner.publish_json(&full_topic, true, &full)?;
        inner
            .tracker()
            .observe(&full_topic, &Value::Null, to_ms(state.fetched_at));

        let last_changes = inner
            .tracker()
            .export_under(&inner.topics.vehicle_status_prefix(&meta.vin));
        tracing::debug!(
            target: LOG_TARGET,
            vin = %meta.vin.short(),
            topics = published,
            "vehicle state published"
        );
        Ok(PublishReport {
            published,
            last_changes,
        })
    }

    /// Publishes `status/<VIN>/availability`.
    pub fn publish_vehicle_availability(
        &self,
        vin: &Vin,
        state: VehicleDataState,
        now: DateTime<Utc>,
    ) -> Result<(), MqttError> {
        let topic = self.inner.topics.vehicle_availability(vin);
        self.publish_enum(&topic, &state, now)
    }

    /// Publishes `status/account/<id>/availability`.
    pub fn publish_account_availability(
        &self,
        id: &AccountId,
        state: AccountConnectionState,
        now: DateTime<Utc>,
    ) -> Result<(), MqttError> {
        let topic = self.inner.topics.account_availability(id);
        self.publish_enum(&topic, &state, now)
    }

    fn publish_enum<T: Serialize>(
        &self,
        topic: &str,
        value: &T,
        now: DateTime<Utc>,
    ) -> Result<(), MqttError> {
        let val = serde_json::to_value(value).map_err(|_| MqttError::Serialisation {
            topic: topic.to_string(),
        })?;
        let ts = to_ms(now);
        let lc = self.inner.tracker().observe(topic, &val, ts);
        self.inner
            .publish_json(topic, true, &StatePayload { val, ts, lc })
    }

    /// Publishes an arbitrary JSON value, for `maintenance/stats`. The topic comes from
    /// [`topics`](Self::topics).
    pub fn publish_json(&self, topic: &str, value: &Value, retain: bool) -> Result<(), MqttError> {
        self.inner.publish_json(topic, retain, value)
    }

    /// Switches `connected` between `1` (no fresh data) and `2` (at least one account delivers).
    pub fn set_operational(&self, operational: bool) -> Result<(), MqttError> {
        let before = self.inner.operational.swap(operational, Ordering::SeqCst);
        if before != operational && self.is_connected() {
            let topic = self.inner.topics.connected();
            self.inner
                .try_publish(&topic, true, self.inner.connected_value().into())?;
        }
        Ok(())
    }

    /// Seeds `lc` for a vehicle from the persisted map, before the first publish.
    pub fn seed_last_changes(&self, vin: &Vin, last_changes: &BTreeMap<String, i64>) {
        let prefix = self.inner.topics.vehicle_status_prefix(vin);
        let mut tracker = self.inner.tracker();
        for (path, lc) in last_changes {
            let mut topic = prefix.clone();
            topic.push_str(path);
            tracker.seed(&topic, *lc);
        }
    }

    /// Removes every retained topic of the vehicle with an empty retained publish and forgets
    /// its `lc` state. Returns how many topics were cleared.
    pub fn clear_vehicle(&self, vin: &Vin) -> Result<usize, MqttError> {
        let prefix = self.inner.topics.vehicle_status_prefix(vin);
        let mut topics = self.inner.tracker().topics_under(&prefix);
        let full = self.inner.topics.full(vin);
        let availability = self.inner.topics.vehicle_availability(vin);
        for extra in [full, availability] {
            if !topics.contains(&extra) {
                topics.push(extra);
            }
        }
        for topic in &topics {
            self.inner.try_publish(topic, true, Vec::new())?;
        }
        self.inner.tracker().forget_under(&prefix);
        tracing::info!(
            target: LOG_TARGET,
            vin = %vin.short(),
            topics = topics.len(),
            "retained vehicle topics cleared"
        );
        Ok(topics.len())
    }

    /// Removes the account's availability topic.
    pub fn clear_account(&self, id: &AccountId) -> Result<(), MqttError> {
        let topic = self.inner.topics.account_availability(id);
        self.inner.try_publish(&topic, true, Vec::new())?;
        self.inner.tracker().forget(&topic);
        Ok(())
    }

    /// Messages received on the `set` subscriptions.
    pub fn incoming(&self) -> broadcast::Receiver<Incoming> {
        self.inner.incoming.subscribe()
    }

    /// Publishes `connected = 0`, disconnects and lets the task finish.
    pub async fn shutdown(&self) {
        self.inner.stopping.store(true, Ordering::SeqCst);
        if self.is_connected() {
            let topic = self.inner.topics.connected();
            if let Err(error) = self.inner.client.publish(&topic, QOS, true, "0").await {
                tracing::debug!(target: LOG_TARGET, %error, "could not publish connected=0 on shutdown");
            }
        }
        if let Err(error) = self.inner.client.disconnect().await {
            tracing::debug!(target: LOG_TARGET, %error, "disconnect request failed");
        }
    }
}

async fn run(mut eventloop: EventLoop, inner: Arc<Inner>, info: ServiceInfo) {
    let mut reported_failure = false;
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                if ack.code != ConnectReturnCode::Success {
                    tracing::warn!(target: LOG_TARGET, code = ?ack.code, "broker refused the connection");
                    continue;
                }
                inner.connected.store(true, Ordering::SeqCst);
                reported_failure = false;
                tracing::info!(target: LOG_TARGET, prefix = inner.topics.prefix(), "connected to the broker");
                on_connect(&inner, &info);
            }
            Ok(Event::Incoming(Packet::Publish(publish))) => {
                dispatch_incoming(&inner, publish.topic, publish.payload.to_vec());
            }
            Ok(Event::Outgoing(Outgoing::Disconnect)) => {
                if inner.stopping.load(Ordering::SeqCst) {
                    inner.connected.store(false, Ordering::SeqCst);
                    tracing::info!(target: LOG_TARGET, "publisher stopped");
                    break;
                }
            }
            Ok(_) => {}
            Err(error) => {
                let was_connected = inner.connected.swap(false, Ordering::SeqCst);
                if inner.stopping.load(Ordering::SeqCst) {
                    tracing::info!(target: LOG_TARGET, "publisher stopped");
                    break;
                }
                if was_connected || !reported_failure {
                    tracing::warn!(target: LOG_TARGET, %error, "broker connection lost, reconnecting");
                    reported_failure = true;
                } else {
                    tracing::debug!(target: LOG_TARGET, %error, "reconnect attempt failed");
                }
                tokio::time::sleep(RECONNECT_DELAY).await;
            }
        }
    }
}

fn on_connect(inner: &Inner, info: &ServiceInfo) {
    let connected = inner.topics.connected();
    if let Err(error) = inner.try_publish(&connected, true, inner.connected_value().into()) {
        tracing::warn!(target: LOG_TARGET, %error, "could not publish connected");
    }
    let info_topic = inner.topics.info();
    if let Err(error) = inner.publish_json(&info_topic, true, &InfoPayload::new(info)) {
        tracing::warn!(target: LOG_TARGET, %error, "could not publish info");
    }
    for filter in [
        inner.topics.set_wildcard(),
        inner.topics.maintenance_set_wildcard(),
    ] {
        if let Err(error) = inner.client.try_subscribe(&filter, QOS) {
            tracing::warn!(target: LOG_TARGET, %error, filter, "could not subscribe");
        }
    }
}

fn dispatch_incoming(inner: &Inner, topic: String, payload: Vec<u8>) {
    let message = if topic == inner.topics.maintenance_set_loglevel() {
        Incoming::LogLevel(String::from_utf8_lossy(&payload).trim().to_string())
    } else if let Some((vin, command)) = inner.topics.parse_set(&topic) {
        Incoming::SetCommand {
            vin,
            command,
            payload,
        }
    } else {
        Incoming::Other { topic, payload }
    };
    // No receiver is fine: nobody is interested yet.
    let _ = inner.incoming.send(message);
}
