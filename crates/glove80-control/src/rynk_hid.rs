//! Native Rynk transport over the fixed-size vendor HID reports also used by
//! Rynk's BLE WebHID link.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use rynk::io::{ErrorType, Read, Write};
use rynk::rmk_types::protocol::rynk::{RynkHeader, RYNK_HID_REPORT_SIZE};
use rynk::{RynkDevice, RynkHostError};
use tokio::io::unix::AsyncFd;

use crate::transport::ids::{USB_PID, USB_VID};
use crate::transport::usb::{descriptor_usages, raw_info, report_descriptor};

const RYNK_USAGE_PAGE: u16 = 0xff60;
const RYNK_USAGE: u32 = 0x61;

pub struct HidDevice {
    path: PathBuf,
}

impl HidDevice {
    pub fn discover() -> Result<Vec<Self>, RynkHostError> {
        let entries = std::fs::read_dir("/dev")
            .map_err(|error| RynkHostError::Transport("hidraw_discovery", error.to_string()))?;
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("hidraw"))
            })
            .collect();
        paths.sort();

        Ok(paths
            .into_iter()
            .filter_map(|path| {
                let file = open_file(&path).ok()?;
                let fd = file.as_raw_fd();
                if raw_info(fd).ok()? != (USB_VID, USB_PID) {
                    return None;
                }
                let descriptor = report_descriptor(fd).ok()?;
                descriptor_usages(&descriptor)
                    .contains(&(RYNK_USAGE_PAGE, RYNK_USAGE))
                    .then_some(Self { path })
            })
            .collect())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn open_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

impl RynkDevice for HidDevice {
    type Read = HidReader;
    type Write = HidWriter;

    fn label(&self) -> String {
        format!("Rynk USB HID ({})", self.path.display())
    }

    async fn open(self) -> Result<(Self::Read, Self::Write), RynkHostError> {
        let reader = open_file(&self.path)
            .and_then(AsyncFd::new)
            .map_err(|error| RynkHostError::Transport("open_hid_reader", error.to_string()))?;
        let writer = open_file(&self.path)
            .and_then(AsyncFd::new)
            .map_err(|error| RynkHostError::Transport("open_hid_writer", error.to_string()))?;
        Ok((
            HidReader {
                file: reader,
                report: [0; RYNK_HID_REPORT_SIZE],
                pos: 0,
                end: 0,
                remaining: 0,
            },
            HidWriter { file: writer },
        ))
    }
}

pub struct HidReader {
    file: AsyncFd<File>,
    report: [u8; RYNK_HID_REPORT_SIZE],
    pos: usize,
    end: usize,
    remaining: usize,
}

impl ErrorType for HidReader {
    type Error = std::io::Error;
}

impl Read for HidReader {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.pos < self.end {
                let n = (self.end - self.pos).min(buf.len());
                buf[..n].copy_from_slice(&self.report[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }

            let n = loop {
                let mut ready = self.file.readable().await?;
                match ready.try_io(|inner| {
                    let mut file = inner.get_ref();
                    file.read(&mut self.report)
                }) {
                    Ok(result) => break result?,
                    Err(_) => continue,
                }
            };
            if n == 0 {
                return Ok(0);
            }
            if self.remaining == 0 {
                let Some(frame_len) = RynkHeader::peek_frame_len(&self.report[..n]) else {
                    continue;
                };
                self.remaining = frame_len;
            }
            self.pos = 0;
            self.end = self.remaining.min(n);
            self.remaining -= self.end;
        }
    }
}

pub struct HidWriter {
    file: AsyncFd<File>,
}

impl ErrorType for HidWriter {
    type Error = std::io::Error;
}

impl Write for HidWriter {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        for chunk in buf.chunks(RYNK_HID_REPORT_SIZE) {
            let mut report = [0u8; RYNK_HID_REPORT_SIZE + 1];
            report[1..1 + chunk.len()].copy_from_slice(chunk);
            loop {
                let mut ready = self.file.writable().await?;
                match ready.try_io(|inner| {
                    let mut file = inner.get_ref();
                    file.write_all(&report)
                }) {
                    Ok(result) => {
                        result?;
                        break;
                    }
                    Err(_) => continue,
                }
            }
        }
        Ok(buf.len())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
