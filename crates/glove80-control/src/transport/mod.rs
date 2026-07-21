//! Rynk transport selection shared by the CLI commands.

pub mod ids;
pub mod usb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preference {
    Auto,
    Usb,
    Ble,
}

#[derive(Debug, Clone)]
pub struct Selector {
    pub preference: Preference,
    pub device: Option<String>,
}

pub fn is_ble_address(device: &str) -> bool {
    let bytes: Vec<&str> = device.split(':').collect();
    bytes.len() == 6
        && bytes
            .iter()
            .all(|byte| byte.len() == 2 && byte.chars().all(|c| c.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_ble_addresses() {
        assert!(is_ble_address("AA:BB:CC:DD:EE:FF"));
        assert!(!is_ble_address("/dev/hidraw0"));
    }
}
