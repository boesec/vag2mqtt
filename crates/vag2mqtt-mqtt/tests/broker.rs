//! Integration tests against an in-process rumqttd broker.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, HashMap};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use rumqttd::{Broker, Config, ConnectionSettings, RouterConfig, ServerSettings};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener as TokioListener, TcpStream};
use tokio::task::JoinHandle;
use vag2mqtt_domain::{
    AccountConnectionState, AccountId, Brand, MqttConfig, VehicleDataState, VehicleState, Vin,
};
use vag2mqtt_mqtt::{Incoming, MqttHandle, MqttPublisher, ServiceInfo, StatePayload, VehicleMeta};

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start_broker() -> u16 {
    let port = free_port();
    let router = RouterConfig {
        max_connections: 100,
        max_outgoing_packet_count: 200,
        max_segment_size: 104_857_600,
        max_segment_count: 10,
        ..Default::default()
    };
    let connections = ConnectionSettings {
        connection_timeout_ms: 60_000,
        max_payload_size: 1_048_576,
        max_inflight_count: 100,
        auth: None,
        external_auth: None,
        dynamic_filters: true,
    };
    let server = ServerSettings {
        name: "v4-1".to_string(),
        listen: SocketAddr::from(([127, 0, 0, 1], port)),
        tls: None,
        next_connection_delay_ms: 1,
        connections,
    };
    let config = Config {
        id: 0,
        router,
        v4: Some(HashMap::from([("1".to_string(), server)])),
        ..Default::default()
    };
    let mut broker = Broker::new(config);
    std::thread::spawn(move || {
        let _ = broker.start();
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(std::time::Instant::now() < deadline, "broker did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    port
}

fn config(prefix: &str, port: u16) -> MqttConfig {
    let mut config = MqttConfig::defaults("127.0.0.1", format!("vag2mqtt-test-{prefix}"));
    config.port = port;
    config.topic_prefix = prefix.to_string();
    config.keep_alive = Duration::from_secs(5);
    config
}

fn service_info() -> ServiceInfo {
    ServiceInfo {
        name: "vag2mqtt".into(),
        version: "0.1.0-test".into(),
        started_at: Utc::now(),
    }
}

async fn spawn_publisher(prefix: &str, port: u16) -> (MqttHandle, JoinHandle<()>) {
    let (handle, task) = MqttPublisher::spawn(&config(prefix, port), None, service_info()).unwrap();
    wait_until(|| handle.is_connected(), Duration::from_secs(10)).await;
    (handle, task)
}

async fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) {
    let deadline = tokio::time::Instant::now() + timeout;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met in time"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn fixture_state() -> VehicleState {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vag2mqtt-domain/tests/fixtures/vehicle_state_full.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn vin() -> Vin {
    Vin::new("WAUZZZ0000000TEST").unwrap()
}

fn meta() -> VehicleMeta {
    VehicleMeta {
        vin: vin(),
        brand: Brand::Audi,
        model: Some("A6 e-tron".into()),
        display_name: Some("Family car".into()),
    }
}

/// A received message: payload and retain flag.
#[derive(Clone, Debug)]
struct Received {
    payload: Vec<u8>,
    retain: bool,
}

/// Subscribes to `filter` and collects messages until `quiet` passes with nothing new or
/// `timeout` elapses. Returns them grouped by topic in arrival order.
async fn collect(
    port: u16,
    client_id: &str,
    filter: &str,
    quiet: Duration,
    timeout: Duration,
) -> BTreeMap<String, Vec<Received>> {
    let mut options = MqttOptions::new(client_id, "127.0.0.1", port);
    options.set_keep_alive(Duration::from_secs(5));
    let (client, mut eventloop) = AsyncClient::new(options, 64);
    client.subscribe(filter, QoS::AtLeastOnce).await.unwrap();
    let mut received: BTreeMap<String, Vec<Received>> = BTreeMap::new();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last = tokio::time::Instant::now();
    loop {
        let remaining_quiet = quiet.saturating_sub(last.elapsed());
        let now = tokio::time::Instant::now();
        if now >= deadline || (remaining_quiet.is_zero() && !received.is_empty()) {
            break;
        }
        let wait = remaining_quiet
            .min(deadline - now)
            .max(Duration::from_millis(10));
        match tokio::time::timeout(wait, eventloop.poll()).await {
            Ok(Ok(Event::Incoming(Packet::Publish(p)))) => {
                received.entry(p.topic.clone()).or_default().push(Received {
                    payload: p.payload.to_vec(),
                    retain: p.retain,
                });
                last = tokio::time::Instant::now();
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => panic!("subscriber connection failed: {e}"),
            Err(_) => {}
        }
    }
    let _ = client.disconnect().await;
    received
}

fn text(received: &Received) -> String {
    String::from_utf8_lossy(&received.payload).into_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_subscriber_receives_every_retained_topic() {
    let port = start_broker();
    let (handle, _task) = spawn_publisher("t1", port).await;
    let state = fixture_state();
    let report = handle.publish_state(&meta(), &state).unwrap();
    assert_eq!(report.published, vag2mqtt_mqtt::flatten(&state).len());
    assert_eq!(report.last_changes.len(), report.published + 1);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let received = collect(
        port,
        "late",
        "t1/#",
        Duration::from_millis(500),
        Duration::from_secs(10),
    )
    .await;

    let expected_status: Vec<String> = vag2mqtt_mqtt::flatten(&state)
        .iter()
        .map(|v| format!("t1/status/WAUZZZ0000000TEST/{}", v.path))
        .collect();
    for topic in &expected_status {
        let messages = received
            .get(topic)
            .unwrap_or_else(|| panic!("missing {topic}"));
        assert!(messages[0].retain, "{topic} not retained");
        let payload: StatePayload = serde_json::from_slice(&messages[0].payload).unwrap();
        assert!(payload.ts > 1_700_000_000_000, "{topic} ts is not ms");
        assert!(payload.lc <= payload.ts, "{topic} lc > ts");
    }
    assert!(received.contains_key("t1/status/WAUZZZ0000000TEST/full"));
    assert_eq!(text(&received["t1/connected"][0]), "1");
    let info: serde_json::Value = serde_json::from_slice(&received["t1/info"][0].payload).unwrap();
    assert_eq!(info["name"], "vag2mqtt");
    assert_eq!(info["spec"], "mqtt-smarthome 2.0");
    assert_eq!(info["contract_version"], 1);
    assert_eq!(
        received.len(),
        expected_status.len() + 3,
        "unexpected topics: {:?}",
        received.keys()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn last_will_publishes_connected_zero_on_a_crash() {
    let port = start_broker();
    let (handle, task) = spawn_publisher("t2", port).await;
    task.abort();
    drop(handle);

    let received = collect(
        port,
        "will",
        "t2/connected",
        Duration::from_millis(800),
        Duration::from_secs(15),
    )
    .await;
    let values: Vec<String> = received["t2/connected"].iter().map(text).collect();
    assert_eq!(values.last().map(String::as_str), Some("0"), "{values:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graceful_shutdown_publishes_connected_zero() {
    let port = start_broker();
    let (handle, task) = spawn_publisher("t3", port).await;
    handle.shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("task ends after shutdown")
        .unwrap();
    assert!(!handle.is_connected());

    let received = collect(
        port,
        "grace",
        "t3/connected",
        Duration::from_millis(300),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(text(&received["t3/connected"][0]), "0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn operational_flag_switches_connected_between_1_and_2() {
    let port = start_broker();
    let (handle, _task) = spawn_publisher("t4", port).await;
    handle.set_operational(true).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let received = collect(
        port,
        "op",
        "t4/connected",
        Duration::from_millis(300),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(text(&received["t4/connected"][0]), "2");
    handle.set_operational(false).unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let received = collect(
        port,
        "op2",
        "t4/connected",
        Duration::from_millis(300),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(text(&received["t4/connected"][0]), "1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clearing_a_vehicle_removes_its_retained_topics() {
    let port = start_broker();
    let (handle, _task) = spawn_publisher("t5", port).await;
    handle.publish_state(&meta(), &fixture_state()).unwrap();
    handle
        .publish_vehicle_availability(&vin(), VehicleDataState::Fresh, Utc::now())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let cleared = handle.clear_vehicle(&vin()).unwrap();
    assert!(cleared > 30, "{cleared}");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let received = collect(
        port,
        "clear",
        "t5/#",
        Duration::from_millis(500),
        Duration::from_secs(10),
    )
    .await;
    let vehicle_topics: Vec<&String> = received
        .keys()
        .filter(|t| t.starts_with("t5/status/WAUZZZ"))
        .collect();
    assert!(vehicle_topics.is_empty(), "{vehicle_topics:?}");
    assert!(received.contains_key("t5/connected"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_availability_and_generic_consumer() {
    let port = start_broker();
    let (handle, _task) = spawn_publisher("t6", port).await;
    let id = AccountId::new("acc-1").unwrap();
    handle
        .publish_account_availability(&id, AccountConnectionState::Ok, Utc::now())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;

    // A consumer that knows only the mqtt-smarthome spec.
    let received = collect(
        port,
        "generic",
        "t6/#",
        Duration::from_millis(400),
        Duration::from_secs(5),
    )
    .await;
    let connected: u8 = text(&received["t6/connected"][0]).parse().unwrap();
    assert!(connected <= 2);
    let info: serde_json::Value = serde_json::from_slice(&received["t6/info"][0].payload).unwrap();
    assert!(info["name"].is_string() && info["version"].is_string() && info["spec"].is_string());
    let status: serde_json::Value =
        serde_json::from_slice(&received["t6/status/account/acc-1/availability"][0].payload)
            .unwrap();
    assert_eq!(status["val"], "ok");
    assert!(status["ts"].is_i64() && status["lc"].is_i64());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_set_messages_are_forwarded() {
    let port = start_broker();
    let (handle, _task) = spawn_publisher("t7", port).await;
    let mut incoming = handle.incoming();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut options = MqttOptions::new("sender", "127.0.0.1", port);
    options.set_keep_alive(Duration::from_secs(5));
    let (client, mut eventloop) = AsyncClient::new(options, 16);
    let pump = tokio::spawn(async move { while eventloop.poll().await.is_ok() {} });
    client
        .publish(
            "t7/maintenance/set/loglevel",
            QoS::AtLeastOnce,
            false,
            "debug\n",
        )
        .await
        .unwrap();
    client
        .publish(
            "t7/set/WAUZZZ0000000TEST/climatisation",
            QoS::AtLeastOnce,
            false,
            "on",
        )
        .await
        .unwrap();

    let first = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .unwrap()
        .unwrap();
    let mut got = [first, second];
    got.sort_by_key(|m| matches!(m, Incoming::SetCommand { .. }));
    assert_eq!(got[0], Incoming::LogLevel("debug".into()));
    assert_eq!(
        got[1],
        Incoming::SetCommand {
            vin: "WAUZZZ0000000TEST".into(),
            command: "climatisation".into(),
            payload: b"on".to_vec(),
        }
    );
    pump.abort();
}

/// A TCP proxy the test can cut to simulate a broker outage.
struct Proxy {
    accept: JoinHandle<()>,
    connections: std::sync::Arc<std::sync::Mutex<Vec<JoinHandle<()>>>>,
}

impl Proxy {
    async fn start(listen_port: u16, broker_port: u16) -> Proxy {
        let listener = TokioListener::bind(("127.0.0.1", listen_port))
            .await
            .unwrap();
        let connections = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let pool = std::sync::Arc::clone(&connections);
        let accept = tokio::spawn(async move {
            loop {
                let Ok((mut client, _)) = listener.accept().await else {
                    break;
                };
                let pool = std::sync::Arc::clone(&pool);
                let task = tokio::spawn(async move {
                    let Ok(mut upstream) = TcpStream::connect(("127.0.0.1", broker_port)).await
                    else {
                        return;
                    };
                    let (mut cr, mut cw) = client.split();
                    let (mut ur, mut uw) = upstream.split();
                    let a = async {
                        let _ = tokio::io::copy(&mut cr, &mut uw).await;
                        let _ = uw.shutdown().await;
                    };
                    let b = async {
                        let _ = tokio::io::copy(&mut ur, &mut cw).await;
                        let _ = cw.shutdown().await;
                    };
                    tokio::join!(a, b);
                    let _ = (cr.read(&mut []).await, ur.read(&mut []).await);
                });
                pool.lock().unwrap().push(task);
            }
        });
        Proxy {
            accept,
            connections,
        }
    }

    fn cut(&self) {
        self.accept.abort();
        for task in self.connections.lock().unwrap().drain(..) {
            task.abort();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn publisher_reconnects_after_the_connection_drops() {
    let broker_port = start_broker();
    let proxy_port = free_port();
    let proxy = Proxy::start(proxy_port, broker_port).await;

    let (handle, _task) = spawn_publisher("t8", proxy_port).await;
    assert!(handle.is_connected());

    proxy.cut();
    wait_until(|| !handle.is_connected(), Duration::from_secs(15)).await;

    // Bring the "broker" back on the same address and wait for the reconnect.
    let proxy = Proxy::start(proxy_port, broker_port).await;
    wait_until(|| handle.is_connected(), Duration::from_secs(20)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let received = collect(
        broker_port,
        "reconnect",
        "t8/#",
        Duration::from_millis(400),
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(text(&received["t8/connected"][0]), "1");
    assert!(received.contains_key("t8/info"));
    proxy.cut();
}
