// Rust port of dbg.c. © 2019 Raheman Vaiya. See LICENSE. Frozen f564288.
use crate::{linux, log};
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

const OUTPUT_SIZE: usize = 4096;
const BIT_ARRAY_SIZE: usize = 1024;

#[derive(Debug)]
struct FatalIoctl(io::Error);

impl std::fmt::Display for FatalIoctl {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(output)
    }
}

impl std::error::Error for FatalIoctl {}

pub fn is_fatal_ioctl(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|error| error.is::<FatalIoctl>())
}

// Keep C's (max + 7) / 8, including its omission of SW_MAX itself.
const CAPABILITIES: [(&str, u16, usize); 7] = [
    ("EV", 0, 0x1f),
    ("EV_KEY", 1, 0x2ff),
    ("EV_REL", 2, 0x0f),
    ("EV_ABS", 3, 0x3f),
    ("EV_MSC", 4, 0x07),
    ("EV_SW", 5, 0x10),
    ("EV_LED", 17, 0x0f),
];

fn append(output: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() >= OUTPUT_SIZE - output.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "debug output exceeds C buffer",
        ));
    }
    output.extend_from_slice(bytes);
    Ok(())
}

pub fn format_device_bits(name: &str, bits: &[u8]) -> io::Result<Vec<u8>> {
    if bits.len() > BIT_ARRAY_SIZE {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if bits.iter().all(|byte| *byte == 0) {
        return Ok(Vec::new());
    }
    let mut output = Vec::new();
    append(&mut output, b"\t")?;
    append(&mut output, name.split('\0').next().unwrap().as_bytes())?;
    append(&mut output, b": ")?;
    for (index, byte) in bits.iter().enumerate() {
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                append(&mut output, format!("{} ", 8 * index + bit).as_bytes())?;
            }
        }
    }
    append(&mut output, b"\n")?;
    Ok(output)
}

fn evdev_name(fd: BorrowedFd<'_>) -> io::Result<[u8; 128]> {
    let mut name = [0; 128];
    #[cfg(target_arch = "x86_64")]
    const IOCTL: usize = 16;
    #[cfg(target_arch = "aarch64")]
    const IOCTL: usize = 29;
    #[cfg(target_arch = "x86")]
    const IOCTL: usize = 54;
    // SAFETY: EVIOCGNAME copies at most 128 bytes into the live writable array;
    // the borrowed descriptor remains open for the complete syscall.
    linux::syscall_result(unsafe {
        linux::syscall6(
            IOCTL,
            [
                fd.as_raw_fd() as usize,
                linux::ioctl_request(2, b'E', 0x06, name.len()),
                name.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
        )
    })?;
    Ok(name)
}

/// Fatal capability ioctl errors propagate to the entry point for cleanup/exit.
pub fn print_evdev_details(path: &Path, level: i32, out: &mut impl Write) -> io::Result<()> {
    if level < 2 {
        return Ok(());
    }
    let fd = match linux::open(path, 0) {
        Ok(fd) => fd,
        Err(_) => {
            return log::debug(
                out,
                2,
                "src/dbg.c",
                51,
                "Failed to open %s",
                &[path.as_os_str().as_bytes()],
            );
        }
    };
    let name = match evdev_name(fd.as_fd()) {
        Ok(name) => name,
        Err(error) => return log::perror("ioctl", error),
    };
    let name = &name[..name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len())];
    let mut output = Vec::new();
    append(&mut output, b"(")?;
    append(&mut output, path.as_os_str().as_bytes())?;
    append(&mut output, b") (")?;
    append(&mut output, name)?;
    append(&mut output, b"):\n")?;
    for (label, kind, max) in CAPABILITIES {
        let mut bits = [0; BIT_ARRAY_SIZE];
        let bits = &mut bits[..max.div_ceil(8)];
        linux::evdev_bits(fd.as_fd(), kind, bits)
            .map_err(|error| io::Error::other(FatalIoctl(log::system_error("ioctl", error))))?;
        append(&mut output, &format_device_bits(label, bits)?)?;
    }
    log::write(out, 2, "r{DEBUG}: %s", &[&output])
}
