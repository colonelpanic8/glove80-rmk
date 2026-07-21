//! USB transport: Linux hidraw, no C library dependencies.
//!
//! Rationale for direct ioctls over the `hidapi` crate: hidapi's hidraw
//! backend links libudev (and its libusb backend links libusb), neither of
//! which is available in this repo's plain build environment, and the crate
//! adds nothing we need — enumeration is a readdir of `/dev/hidraw*` plus
//! three ioctls, and matching on the vendor usage page requires parsing the
//! report descriptor ourselves anyway (hidapi only exposes usage pages via
//! platform-specific quirks).
//!
//! Interface selection: match VID/PID ([`ids::USB_VID`]/[`ids::USB_PID`]),
//! then pick the hidraw node whose report descriptor contains the vendor
//! usage pair in [`ids`] (that module is the single place the identifiers
//! live, kept in sync with the firmware's report descriptor).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use super::ids;
use super::Transport;

// hidraw ioctls: _IOR('H', nr, size).
const fn hidraw_ior(nr: u8, size: usize) -> libc::c_ulong {
    (2 << 30) | ((size as libc::c_ulong) << 16) | (b'H' as libc::c_ulong) << 8 | nr as libc::c_ulong
}

const HID_MAX_DESCRIPTOR_SIZE: usize = 4096;
const HIDIOCGRDESCSIZE: libc::c_ulong = hidraw_ior(0x01, 4);
const HIDIOCGRDESC: libc::c_ulong = hidraw_ior(0x02, 4 + HID_MAX_DESCRIPTOR_SIZE);
const HIDIOCGRAWINFO: libc::c_ulong = hidraw_ior(0x03, 8);

#[repr(C)]
struct HidrawDevinfo {
    bustype: u32,
    vendor: i16,
    product: i16,
}

#[repr(C)]
struct HidrawReportDescriptor {
    size: u32,
    value: [u8; HID_MAX_DESCRIPTOR_SIZE],
}

pub(crate) fn raw_info(fd: libc::c_int) -> Result<(u16, u16)> {
    let mut info = HidrawDevinfo {
        bustype: 0,
        vendor: 0,
        product: 0,
    };
    // Safety: HIDIOCGRAWINFO writes a HidrawDevinfo into the pointed-to struct.
    let rc = unsafe { libc::ioctl(fd, HIDIOCGRAWINFO, &mut info) };
    if rc < 0 {
        bail!("HIDIOCGRAWINFO failed: {}", std::io::Error::last_os_error());
    }
    Ok((info.vendor as u16, info.product as u16))
}

pub(crate) fn report_descriptor(fd: libc::c_int) -> Result<Vec<u8>> {
    let mut size: libc::c_int = 0;
    // Safety: HIDIOCGRDESCSIZE writes an int.
    let rc = unsafe { libc::ioctl(fd, HIDIOCGRDESCSIZE, &mut size) };
    if rc < 0 {
        bail!(
            "HIDIOCGRDESCSIZE failed: {}",
            std::io::Error::last_os_error()
        );
    }
    let mut descriptor = HidrawReportDescriptor {
        size: size.clamp(0, HID_MAX_DESCRIPTOR_SIZE as libc::c_int) as u32,
        value: [0; HID_MAX_DESCRIPTOR_SIZE],
    };
    // Safety: HIDIOCGRDESC reads `size` and fills `value`.
    let rc = unsafe { libc::ioctl(fd, HIDIOCGRDESC, &mut descriptor) };
    if rc < 0 {
        bail!("HIDIOCGRDESC failed: {}", std::io::Error::last_os_error());
    }
    Ok(descriptor.value[..descriptor.size as usize].to_vec())
}

/// All (usage page, usage) pairs declared by Usage items in a report
/// descriptor. Extended (4-byte) usages carry their own page in the high
/// 16 bits; short usages inherit the current global usage page.
pub fn descriptor_usages(descriptor: &[u8]) -> Vec<(u16, u32)> {
    let mut usages = Vec::new();
    let mut usage_page: u16 = 0;
    let mut position = 0;
    while position < descriptor.len() {
        let prefix = descriptor[position];
        if prefix == 0xFE {
            // Long item: [0xFE, bDataSize, bLongItemTag, data...]
            let Some(&data_len) = descriptor.get(position + 1) else {
                break;
            };
            position += 3 + data_len as usize;
            continue;
        }
        let data_len = match prefix & 0x03 {
            3 => 4,
            n => n as usize,
        };
        let data_end = position + 1 + data_len;
        if data_end > descriptor.len() {
            break;
        }
        let mut value: u32 = 0;
        for (index, byte) in descriptor[position + 1..data_end].iter().enumerate() {
            value |= (*byte as u32) << (8 * index);
        }
        match prefix & 0xFC {
            0x04 => usage_page = value as u16, // Global: Usage Page
            0x08 => {
                // Local: Usage
                if data_len == 4 {
                    usages.push(((value >> 16) as u16, value & 0xFFFF));
                } else {
                    usages.push((usage_page, value));
                }
            }
            _ => {}
        }
        position = data_end;
    }
    usages
}

fn matches_vendor_usage(descriptor: &[u8]) -> bool {
    descriptor_usages(descriptor).contains(&(ids::USB_VENDOR_USAGE_PAGE, ids::USB_VENDOR_USAGE))
}

pub struct UsbTransport {
    file: File,
    path: PathBuf,
}

impl UsbTransport {
    /// Open a specific hidraw node (used by `--device /dev/hidrawN`).
    /// VID/PID and the vendor usage page are still verified so we never
    /// stream protocol frames at the keyboard's boot/NKRO interfaces.
    pub fn open_path(path: impl AsRef<Path>) -> Result<UsbTransport> {
        let path = path.as_ref();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .with_context(|| {
                format!(
                    "could not open {} (missing permissions? a udev rule granting \
                     access to VID {:04x} PID {:04x} may be needed)",
                    path.display(),
                    ids::USB_VID,
                    ids::USB_PID
                )
            })?;
        let fd = file.as_raw_fd();
        let (vid, pid) = raw_info(fd).with_context(|| format!("{} ", path.display()))?;
        if (vid, pid) != (ids::USB_VID, ids::USB_PID) {
            bail!(
                "{} is {vid:04x}:{pid:04x}, not a Glove80 ({:04x}:{:04x})",
                path.display(),
                ids::USB_VID,
                ids::USB_PID
            );
        }
        let descriptor = report_descriptor(fd)?;
        if !matches_vendor_usage(&descriptor) {
            bail!(
                "{} does not expose the host-protocol vendor usage \
                 (page {:04x}, usage {:04x}); wrong interface?",
                path.display(),
                ids::USB_VENDOR_USAGE_PAGE,
                ids::USB_VENDOR_USAGE
            );
        }
        Ok(UsbTransport {
            file,
            path: path.to_path_buf(),
        })
    }

    /// Enumerate `/dev/hidraw*` and open the Glove80 interface whose report
    /// descriptor carries the protocol's vendor usage page.
    pub fn find() -> Result<UsbTransport> {
        let mut candidates: Vec<PathBuf> = std::fs::read_dir("/dev")
            .context("could not list /dev")?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("hidraw"))
            })
            .collect();
        candidates.sort();

        let mut denied = Vec::new();
        for path in candidates {
            let Ok(file) = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
            else {
                // Remember nodes we could not even probe; only relevant if
                // nothing else matches.
                if let Ok(probe) = File::open(&path) {
                    drop(probe);
                } else {
                    denied.push(path);
                }
                continue;
            };
            let fd = file.as_raw_fd();
            let Ok((vid, pid)) = raw_info(fd) else {
                continue;
            };
            if (vid, pid) != (ids::USB_VID, ids::USB_PID) {
                continue;
            }
            let Ok(descriptor) = report_descriptor(fd) else {
                continue;
            };
            if matches_vendor_usage(&descriptor) {
                return Ok(UsbTransport { file, path });
            }
        }
        if denied.is_empty() {
            bail!(
                "no hidraw interface with VID {:04x} PID {:04x} and vendor usage \
                 page {:04x} found (is the keyboard plugged in and running the \
                 RMK firmware?)",
                ids::USB_VID,
                ids::USB_PID,
                ids::USB_VENDOR_USAGE_PAGE
            );
        }
        bail!(
            "no matching hidraw interface found, but {} could not be opened \
             (permissions); a udev rule for VID {:04x} PID {:04x} may be needed",
            denied
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", "),
            ids::USB_VID,
            ids::USB_PID
        );
    }
}

impl Transport for UsbTransport {
    fn chunk_len(&self) -> usize {
        ids::USB_REPORT_LEN
    }

    fn pads_chunks(&self) -> bool {
        true
    }

    fn send_chunk(&mut self, chunk: &[u8]) -> Result<()> {
        // hidraw write: first byte is the report ID (0 for unnumbered
        // reports; the kernel strips it), then the full fixed-size report.
        let mut report = Vec::with_capacity(1 + chunk.len());
        report.push(ids::USB_OUTPUT_REPORT_ID);
        report.extend_from_slice(chunk);
        self.file
            .write_all(&report)
            .with_context(|| format!("could not write to {}", self.path.display()))?;
        Ok(())
    }

    fn recv_chunk(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>> {
        let mut poll_fd = libc::pollfd {
            fd: self.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        // Safety: valid pollfd array of length 1.
        let ready = unsafe { libc::poll(&mut poll_fd, 1, millis.max(1)) };
        if ready < 0 {
            bail!(
                "poll on {} failed: {}",
                self.path.display(),
                std::io::Error::last_os_error()
            );
        }
        if ready == 0 {
            return Ok(None);
        }
        let mut buffer = [0u8; 64];
        let length = self
            .file
            .read(&mut buffer)
            .with_context(|| format!("could not read from {}", self.path.display()))?;
        let start = usize::from(ids::USB_INPUT_HAS_REPORT_ID && length > 0);
        Ok(Some(buffer[start..length].to_vec()))
    }

    fn description(&self) -> String {
        format!("USB hidraw ({})", self.path.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vendor_usage_from_descriptor() {
        // Usage Page (vendor 0xFF88), Usage (0x01), Collection (Application),
        // Usage 0x02, Input, Usage 0x03, Output, End Collection.
        let descriptor = [
            0x06, 0x88, 0xFF, // Usage Page (0xFF88)
            0x09, 0x01, // Usage (0x01)
            0xA1, 0x01, // Collection (Application)
            0x09, 0x02, // Usage (0x02)
            0x15, 0x00, // Logical Minimum (0)
            0x26, 0xFF, 0x00, // Logical Maximum (255)
            0x75, 0x08, // Report Size (8)
            0x95, 0x20, // Report Count (32)
            0x81, 0x02, // Input
            0x09, 0x03, // Usage (0x03)
            0x91, 0x02, // Output
            0xC0, // End Collection
        ];
        let usages = descriptor_usages(&descriptor);
        assert!(usages.contains(&(0xFF88, 0x01)));
        assert!(usages.contains(&(0xFF88, 0x02)));
        assert!(matches_vendor_usage(&descriptor));
    }

    #[test]
    fn keyboard_descriptor_does_not_match() {
        // Generic Desktop / Keyboard boot interface prefix.
        let descriptor = [
            0x05, 0x01, // Usage Page (Generic Desktop)
            0x09, 0x06, // Usage (Keyboard)
            0xA1, 0x01, // Collection (Application)
            0xC0,
        ];
        assert_eq!(descriptor_usages(&descriptor), vec![(0x0001, 0x06)]);
        assert!(!matches_vendor_usage(&descriptor));
    }

    #[test]
    fn extended_usage_carries_its_own_page() {
        let descriptor = [
            0x05, 0x01, // Usage Page (Generic Desktop)
            0x0B, 0x01, 0x00, 0x88, 0xFF, // Usage (extended: page 0xFF88, usage 1)
        ];
        assert_eq!(descriptor_usages(&descriptor), vec![(0xFF88, 0x0001)]);
        assert!(matches_vendor_usage(&descriptor));
    }
}
