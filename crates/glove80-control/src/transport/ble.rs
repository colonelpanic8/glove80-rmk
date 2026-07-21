//! BLE transport: BlueZ GATT client over the D-Bus API, via pure-Rust
//! `zbus`.
//!
//! Rationale for zbus over `bluer`/`btleplug`: both of those route through
//! the C `libdbus` (via the `dbus` crate), which is not available in this
//! repo's plain build environment; zbus is pure Rust and its blocking API
//! keeps the CLI synchronous (no tokio runtime). BlueZ's D-Bus surface is
//! all we need: discover by service UUID, `WriteValue` with
//! `{"type": "command"}` for write-without-response, and `StartNotify` +
//! `PropertiesChanged` signals for notifications.
//!
//! Identifiers (service/characteristic UUIDs) live in [`super::ids`],
//! kept in sync with the firmware's `HostProtoService` definition.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::ids;
use super::Transport;

const BLUEZ: &str = "org.bluez";
const DEVICE_IFACE: &str = "org.bluez.Device1";
const ADAPTER_IFACE: &str = "org.bluez.Adapter1";
const CHARACTERISTIC_IFACE: &str = "org.bluez.GattCharacteristic1";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";

/// Local-name fallback used when a known device has no cached UUIDs yet.
const BLE_NAME_HINT: &str = "Glove80";

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(12);
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(20);

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

fn managed_objects(connection: &Connection) -> Result<ManagedObjects> {
    let object_manager: Proxy<'_> =
        Proxy::new(connection, BLUEZ, "/", "org.freedesktop.DBus.ObjectManager")
            .context("could not reach BlueZ on the system bus (is bluetoothd running?)")?;
    object_manager
        .call("GetManagedObjects", &())
        .context("BlueZ GetManagedObjects failed")
}

fn device_proxy(connection: &Connection, path: &str) -> Result<Proxy<'static>> {
    Ok(Proxy::new(
        connection,
        BLUEZ.to_owned(),
        path.to_owned(),
        DEVICE_IFACE.to_owned(),
    )?)
}

fn characteristic_proxy(connection: &Connection, path: &str) -> Result<Proxy<'static>> {
    Ok(Proxy::new(
        connection,
        BLUEZ.to_owned(),
        path.to_owned(),
        CHARACTERISTIC_IFACE.to_owned(),
    )?)
}

/// Find Device1 paths that advertise the host-protocol service (or match
/// the address filter / name hint).
fn matching_devices(
    connection: &Connection,
    objects: &ManagedObjects,
    address_filter: Option<&str>,
) -> Vec<(String, String)> {
    let wanted_uuid = ids::BLE_SERVICE_UUID.to_ascii_lowercase();
    let mut matches = Vec::new();
    for (path, interfaces) in objects {
        if !interfaces.contains_key(DEVICE_IFACE) {
            continue;
        }
        let Ok(device) = device_proxy(connection, path.as_str()) else {
            continue;
        };
        let Ok(address) = device.get_property::<String>("Address") else {
            continue;
        };
        if let Some(filter) = address_filter {
            // An explicit address is trusted outright; the characteristic
            // UUIDs are verified after connecting.
            if address.eq_ignore_ascii_case(filter) {
                matches.push((path.as_str().to_owned(), address));
            }
            continue;
        }
        let advertises_service = device
            .get_property::<Vec<String>>("UUIDs")
            .map(|uuids| uuids.iter().any(|u| u.eq_ignore_ascii_case(&wanted_uuid)))
            .unwrap_or(false);
        let named_glove80 = device
            .get_property::<String>("Name")
            .map(|name| name.contains(BLE_NAME_HINT))
            .unwrap_or(false);
        // Known-but-never-resolved devices have no cached UUIDs; fall back
        // to the advertised name for those.
        if advertises_service || named_glove80 {
            matches.push((path.as_str().to_owned(), address));
        }
    }
    matches.sort();
    matches
}

fn discover(connection: &Connection, address_filter: Option<&str>) -> Result<(String, String)> {
    let objects = managed_objects(connection)?;
    let mut found = matching_devices(connection, &objects, address_filter);
    if found.is_empty() {
        // Active scan, filtered to the protocol service where possible.
        let adapters: Vec<String> = objects
            .keys()
            .filter(|path| objects[*path].contains_key(ADAPTER_IFACE))
            .map(|path| path.as_str().to_owned())
            .collect();
        if adapters.is_empty() {
            bail!("no Bluetooth adapter found");
        }
        let mut scanning = Vec::new();
        for adapter_path in &adapters {
            let Ok(adapter) = Proxy::new(
                connection,
                BLUEZ.to_owned(),
                adapter_path.clone(),
                ADAPTER_IFACE.to_owned(),
            ) else {
                continue;
            };
            let filter: HashMap<&str, Value<'_>> = HashMap::from([
                ("UUIDs", Value::from(vec![ids::BLE_SERVICE_UUID.to_owned()])),
                ("Transport", Value::from("le")),
            ]);
            let _ = adapter.call::<_, _, ()>("SetDiscoveryFilter", &(filter,));
            if adapter.call::<_, _, ()>("StartDiscovery", &()).is_ok() {
                scanning.push(adapter);
            }
        }
        let deadline = Instant::now() + DISCOVERY_TIMEOUT;
        while found.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
            let objects = managed_objects(connection)?;
            found = matching_devices(connection, &objects, address_filter);
        }
        for adapter in &scanning {
            let _ = adapter.call::<_, _, ()>("StopDiscovery", &());
        }
    }
    match found.len() {
        0 => match address_filter {
            Some(address) => bail!("no BLE device {address} advertising the host protocol found"),
            None => bail!(
                "no BLE device advertising service {} found (is the keyboard on \
                 and paired/advertising?)",
                ids::BLE_SERVICE_UUID
            ),
        },
        1 => Ok(found.into_iter().next().unwrap()),
        _ => bail!(
            "multiple matching BLE devices found ({}); pick one with --device <ADDRESS>",
            found
                .iter()
                .map(|(_, address)| address.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn wait_services_resolved(device: &Proxy<'_>) -> Result<()> {
    let deadline = Instant::now() + RESOLVE_TIMEOUT;
    loop {
        if device
            .get_property::<bool>("ServicesResolved")
            .unwrap_or(false)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("BLE services were not resolved within {RESOLVE_TIMEOUT:?}");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Find the request/response characteristics under `device_path`.
fn find_characteristics(connection: &Connection, device_path: &str) -> Result<(String, String)> {
    let objects = managed_objects(connection)?;
    let mut request_path = None;
    let mut response_path = None;
    let prefix = format!("{device_path}/");
    for (path, interfaces) in &objects {
        if !path.as_str().starts_with(&prefix) || !interfaces.contains_key(CHARACTERISTIC_IFACE) {
            continue;
        }
        let Ok(characteristic) = characteristic_proxy(connection, path.as_str()) else {
            continue;
        };
        let Ok(uuid) = characteristic.get_property::<String>("UUID") else {
            continue;
        };
        if uuid.eq_ignore_ascii_case(ids::BLE_REQUEST_CHAR_UUID) {
            request_path = Some(path.as_str().to_owned());
        } else if uuid.eq_ignore_ascii_case(ids::BLE_RESPONSE_CHAR_UUID) {
            response_path = Some(path.as_str().to_owned());
        }
    }
    match (request_path, response_path) {
        (Some(request), Some(response)) => Ok((request, response)),
        _ => bail!(
            "device does not expose the host-protocol characteristics \
             (request {}, response {})",
            ids::BLE_REQUEST_CHAR_UUID,
            ids::BLE_RESPONSE_CHAR_UUID
        ),
    }
}

fn value_to_bytes(value: &OwnedValue) -> Option<Vec<u8>> {
    match &**value {
        Value::Array(array) => {
            let mut bytes = Vec::with_capacity(array.len());
            for item in array.iter() {
                match item {
                    Value::U8(byte) => bytes.push(*byte),
                    _ => return None,
                }
            }
            Some(bytes)
        }
        _ => None,
    }
}

pub struct BleTransport {
    request_char: Proxy<'static>,
    notifications: mpsc::Receiver<Vec<u8>>,
    chunk_len: usize,
    #[allow(dead_code)] // consumed only by the retained product-protocol client
    description: String,
}

impl BleTransport {
    /// Connect to the Glove80 over BlueZ. `address` (AA:BB:CC:DD:EE:FF)
    /// disambiguates when several matching devices are known.
    pub fn connect(address: Option<&str>) -> Result<BleTransport> {
        let connection =
            Connection::system().context("could not connect to the D-Bus system bus")?;
        let (device_path, device_address) = discover(&connection, address)?;
        let device = device_proxy(&connection, &device_path)?;
        if !device.get_property::<bool>("Connected").unwrap_or(false) {
            device
                .call::<_, _, ()>("Connect", &())
                .with_context(|| format!("could not connect to {device_address}"))?;
        }
        wait_services_resolved(&device)?;
        let (request_path, response_path) = find_characteristics(&connection, &device_path)?;
        let request_char = characteristic_proxy(&connection, &request_path)?;
        let response_char = characteristic_proxy(&connection, &response_path)?;

        // ATT payload for a write-without-response is MTU - 3. BlueZ only
        // exposes MTU on newer versions; PROTOCOL.md lets us assume >= 20.
        let chunk_len = response_char
            .get_property::<u16>("MTU")
            .map(|mtu| usize::from(mtu).saturating_sub(3))
            .unwrap_or(20)
            .max(20);

        // Subscribe to PropertiesChanged on the response characteristic
        // BEFORE StartNotify so no notification is missed. Raw signal
        // matching (not zbus property caching) so repeated identical values
        // are never deduplicated.
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface(PROPERTIES_IFACE)?
            .member("PropertiesChanged")?
            .path(response_path.as_str())?
            .build();
        let messages = MessageIterator::for_match_rule(rule, &connection, Some(64))?;
        let (sender, notifications) = mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name("ble-notify".into())
            .spawn(move || {
                for message in messages {
                    let Ok(message) = message else { break };
                    let Ok((interface, changed, _invalidated)) =
                        message
                            .body()
                            .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
                    else {
                        continue;
                    };
                    if interface != CHARACTERISTIC_IFACE {
                        continue;
                    }
                    let Some(bytes) = changed.get("Value").and_then(value_to_bytes) else {
                        continue;
                    };
                    if sender.send(bytes).is_err() {
                        break;
                    }
                }
            })
            .context("could not spawn the BLE notification thread")?;
        response_char
            .call::<_, _, ()>("StartNotify", &())
            .context("StartNotify on the response characteristic failed")?;

        Ok(BleTransport {
            request_char,
            notifications,
            chunk_len,
            description: format!("BLE ({device_address}, ATT payload {chunk_len})"),
        })
    }
}

impl Transport for BleTransport {
    fn chunk_len(&self) -> usize {
        self.chunk_len
    }

    fn pads_chunks(&self) -> bool {
        false
    }

    fn send_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        // Write-without-response, per PROTOCOL.md.
        let options: HashMap<&str, Value<'_>> = HashMap::from([("type", Value::from("command"))]);
        self.request_char
            .call::<_, _, ()>("WriteValue", &(chunk, options))
            .context("BLE WriteValue failed")?;
        Ok(())
    }

    fn recv_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        match self.notifications.recv_timeout(timeout) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow!(
                "BLE notification stream ended (device disconnected?)"
            )),
        }
    }

    fn description(&self) -> String {
        self.description.clone()
    }
}
