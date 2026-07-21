//! Transports for the RMK host protocol (PROTOCOL.md).
//!
//! A [`Transport`] moves raw frame chunks; segmentation/reassembly and
//! request/response correlation live in [`crate::hostproto`], built on the
//! `glove80-host-protocol` frame layer.

pub mod ble;
pub mod ids;
pub mod usb;

use std::time::Duration;

use anyhow::{bail, Result};

/// One protocol transport: sends and receives frame chunks
/// (one HID report / one ATT write or notification per chunk).
pub trait Transport {
    /// Chunk size to segment outgoing messages into.
    fn chunk_len(&self) -> usize;
    /// Whether outgoing chunks must be zero-padded to `chunk_len`
    /// (USB HID reports are fixed-size; BLE writes are not padded).
    fn pads_chunks(&self) -> bool;
    /// Send one chunk.
    fn send_chunk(&mut self, chunk: &[u8]) -> Result<()>;
    /// Receive one chunk, or `None` if `timeout` elapses first.
    fn recv_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>>;
    /// Human-readable description of the connected endpoint.
    #[allow(dead_code)] // consumed only by the retained product-protocol client
    fn description(&self) -> String;
}

/// Which transport the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    /// Prefer USB when a matching hidraw device is present, else BLE.
    Auto,
    Usb,
    Ble,
}

/// Transport selection derived from the global CLI flags.
#[derive(Debug, Clone)]
pub struct Selector {
    pub preference: Preference,
    /// Disambiguator: a `/dev/hidraw*` path or a BLE `AA:BB:CC:DD:EE:FF`
    /// address.
    pub device: Option<String>,
}

pub fn is_ble_address(device: &str) -> bool {
    let bytes: Vec<&str> = device.split(':').collect();
    bytes.len() == 6
        && bytes
            .iter()
            .all(|b| b.len() == 2 && b.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Open a transport per the selector. Auto prefers USB when present.
pub fn connect(selector: &Selector) -> Result<Box<dyn Transport>> {
    if let Some(device) = &selector.device {
        if device.starts_with("/dev/") {
            if selector.preference == Preference::Ble {
                bail!("--ble conflicts with --device {device} (a hidraw path)");
            }
            return Ok(Box::new(usb::UsbTransport::open_path(device)?));
        }
        if is_ble_address(device) {
            if selector.preference == Preference::Usb {
                bail!("--usb conflicts with --device {device} (a BLE address)");
            }
            return Ok(Box::new(ble::BleTransport::connect(Some(device))?));
        }
        bail!(
            "--device {device} is neither a /dev/hidraw* path nor a BLE address \
             (AA:BB:CC:DD:EE:FF)"
        );
    }
    match selector.preference {
        Preference::Usb => Ok(Box::new(usb::UsbTransport::find()?)),
        Preference::Ble => Ok(Box::new(ble::BleTransport::connect(None)?)),
        Preference::Auto => match usb::UsbTransport::find() {
            Ok(transport) => Ok(Box::new(transport)),
            Err(usb_error) => match ble::BleTransport::connect(None) {
                Ok(transport) => Ok(Box::new(transport)),
                Err(ble_error) => bail!(
                    "no Glove80 host-protocol endpoint found\n  USB: {usb_error:#}\n  BLE: {ble_error:#}"
                ),
            },
        },
    }
}

#[cfg(test)]
pub mod mock {
    //! In-memory transport for unit tests: decodes requests, answers from a
    //! script, and captures every decoded request for assertions.

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use anyhow::{anyhow, Result};
    use glove80_host_protocol::frame::{frame_count, write_frame, Reassembler};
    use glove80_host_protocol::{
        decode_request, encode_response, Request, Response, MAX_MESSAGE_LEN,
    };

    use super::Transport;

    type Handler = Box<dyn FnMut(u8, &Request) -> Vec<Response> + Send>;

    pub struct MockTransport {
        chunk: usize,
        pads: bool,
        handlers: VecDeque<Handler>,
        outgoing: VecDeque<Vec<u8>>,
        reassembler: Reassembler<MAX_MESSAGE_LEN>,
        pub requests: Arc<Mutex<Vec<Request>>>,
    }

    impl MockTransport {
        /// USB-shaped mock: 32-byte padded chunks.
        pub fn new() -> Self {
            Self {
                chunk: 32,
                pads: true,
                handlers: VecDeque::new(),
                outgoing: VecDeque::new(),
                reassembler: Reassembler::new(),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        pub fn requests_handle(&self) -> Arc<Mutex<Vec<Request>>> {
            Arc::clone(&self.requests)
        }

        /// Queue a handler for the next request. Each incoming request pops
        /// one handler; a request with no handler queued panics the test.
        pub fn expect(
            mut self,
            handler: impl FnMut(u8, &Request) -> Vec<Response> + Send + 'static,
        ) -> Self {
            self.handlers.push_back(Box::new(handler));
            self
        }

        fn queue_response(&mut self, response: &Response) {
            let mut message = [0u8; MAX_MESSAGE_LEN];
            let len = encode_response(response, &mut message).expect("mock response encodes");
            let frames = frame_count(len, self.chunk).expect("mock response frames");
            for index in 0..frames {
                let mut chunk = vec![0u8; self.chunk];
                let used =
                    write_frame(&message[..len], self.chunk, index, &mut chunk).expect("frame");
                if !self.pads {
                    chunk.truncate(used);
                }
                self.outgoing.push_back(chunk);
            }
        }
    }

    impl Transport for MockTransport {
        fn chunk_len(&self) -> usize {
            self.chunk
        }

        fn pads_chunks(&self) -> bool {
            self.pads
        }

        fn send_chunk(&mut self, chunk: &[u8]) -> Result<()> {
            let Some(message) = self
                .reassembler
                .push(chunk)
                .map_err(|e| anyhow!("mock reassembly failed: {e}"))?
            else {
                return Ok(());
            };
            let message = message.to_vec();
            let (request_id, request) =
                decode_request(&message).map_err(|e| anyhow!("mock decode failed: {e}"))?;
            self.requests.lock().unwrap().push(request.clone());
            let mut handler = self
                .handlers
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected request: {request:?}"));
            for response in handler(request_id, &request) {
                self.queue_response(&response);
            }
            Ok(())
        }

        fn recv_chunk(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>> {
            Ok(self.outgoing.pop_front())
        }

        fn description(&self) -> String {
            "mock".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_ble_addresses() {
        assert!(is_ble_address("DC:2C:26:00:12:34"));
        assert!(is_ble_address("dc:2c:26:00:12:34"));
        assert!(!is_ble_address("/dev/hidraw3"));
        assert!(!is_ble_address("DC:2C:26:00:12"));
        assert!(!is_ble_address("DC:2C:26:00:12:GG"));
    }
}
