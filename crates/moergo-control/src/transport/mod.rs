//! Rynk transport selection shared by the CLI commands.

use anyhow::{bail, Result};

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

/// An explicit selector must match; it never falls back to another device.
pub fn select_candidate<T>(
    mut devices: Vec<T>,
    requested: Option<&str>,
    kind: &str,
    matches: impl Fn(&T, &str) -> bool,
) -> Result<T> {
    if let Some(requested) = requested {
        devices.retain(|device| matches(device, requested));
    }
    match devices.len() {
        0 => match requested {
            Some(requested) => bail!("no {kind} device matches {requested}"),
            None => bail!("no {kind} device found"),
        },
        1 => Ok(devices.pop().expect("length checked")),
        count => bail!("found {count} {kind} devices; pass --device to select one uniquely"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_selection_never_falls_back() {
        let choose = |devices, requested| {
            select_candidate(devices, requested, "test", |device, needle| {
                *device == needle
            })
        };
        assert!(choose(vec!["/dev/hidraw1"], Some("/dev/hidraw9")).is_err());
        assert!(choose(vec!["/dev/hidraw1"], Some("typo")).is_err());
        assert_eq!(choose(vec!["a", "b"], Some("b")).unwrap(), "b");
        assert_eq!(choose(vec!["a"], None).unwrap(), "a");
        assert!(choose(vec!["a", "b"], None).is_err());
        assert!(choose(vec![], None).is_err());
        assert!(choose(vec!["a", "a"], Some("a")).is_err());
    }

    #[test]
    fn recognizes_ble_addresses() {
        assert!(is_ble_address("AA:BB:CC:DD:EE:FF"));
        assert!(!is_ble_address("/dev/hidraw0"));
    }
}
