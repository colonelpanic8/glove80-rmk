//! Minimal Linux hidraw helpers used by the native Rynk HID transport.

use anyhow::{bail, Result};

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
    // SAFETY: HIDIOCGRAWINFO writes a HidrawDevinfo to this valid pointer.
    let result = unsafe { libc::ioctl(fd, HIDIOCGRAWINFO, &mut info) };
    if result < 0 {
        bail!("HIDIOCGRAWINFO failed: {}", std::io::Error::last_os_error());
    }
    Ok((info.vendor as u16, info.product as u16))
}

pub(crate) fn report_descriptor(fd: libc::c_int) -> Result<Vec<u8>> {
    let mut size: libc::c_int = 0;
    // SAFETY: HIDIOCGRDESCSIZE writes an integer to this valid pointer.
    let result = unsafe { libc::ioctl(fd, HIDIOCGRDESCSIZE, &mut size) };
    if result < 0 {
        bail!(
            "HIDIOCGRDESCSIZE failed: {}",
            std::io::Error::last_os_error()
        );
    }
    let mut descriptor = HidrawReportDescriptor {
        size: size.clamp(0, HID_MAX_DESCRIPTOR_SIZE as libc::c_int) as u32,
        value: [0; HID_MAX_DESCRIPTOR_SIZE],
    };
    // SAFETY: HIDIOCGRDESC reads `size` and fills the in-bounds value buffer.
    let result = unsafe { libc::ioctl(fd, HIDIOCGRDESC, &mut descriptor) };
    if result < 0 {
        bail!("HIDIOCGRDESC failed: {}", std::io::Error::last_os_error());
    }
    Ok(descriptor.value[..descriptor.size as usize].to_vec())
}

/// Return all (usage page, usage) pairs declared by a HID report descriptor.
pub fn descriptor_usages(descriptor: &[u8]) -> Vec<(u16, u32)> {
    let mut usages = Vec::new();
    let mut usage_page = 0;
    let mut position = 0;
    while position < descriptor.len() {
        let prefix = descriptor[position];
        if prefix == 0xfe {
            let Some(&data_len) = descriptor.get(position + 1) else {
                break;
            };
            position += 3 + usize::from(data_len);
            continue;
        }
        let data_len = match prefix & 0x03 {
            3 => 4,
            size => usize::from(size),
        };
        let data_end = position + 1 + data_len;
        if data_end > descriptor.len() {
            break;
        }
        let value = descriptor[position + 1..data_end]
            .iter()
            .enumerate()
            .fold(0, |value, (index, byte)| {
                value | (u32::from(*byte) << (8 * index))
            });
        match prefix & 0xfc {
            0x04 => usage_page = value as u16,
            0x08 if data_len == 4 => usages.push(((value >> 16) as u16, value & 0xffff)),
            0x08 => usages.push((usage_page, value)),
            _ => {}
        }
        position = data_end;
    }
    usages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_short_and_extended_usages() {
        let descriptor = [0x06, 0x60, 0xff, 0x09, 0x61, 0x0b, 0x34, 0x12, 0x88, 0xff];
        assert_eq!(
            descriptor_usages(&descriptor),
            vec![(0xff60, 0x61), (0xff88, 0x1234)]
        );
    }
}
