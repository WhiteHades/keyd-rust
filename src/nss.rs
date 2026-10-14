// OS NSS lookup for keyd's configured group. See LICENSE.

use crate::runtime::{FdIo, poll_interruptible, shutdown_signal};
use std::io::{self, Read};
use std::os::fd::AsFd;
use std::process::{Command, Stdio};

pub(crate) fn keyd_group_id() -> io::Result<Option<u32>> {
    if shutdown_signal() != 0 {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let mut child = match Command::new("/usr/bin/getent")
        .args(["group", "keyd-rust"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) if shutdown_signal() != 0 => return Err(io::ErrorKind::Interrupted.into()),
        Err(_) => return Ok(None),
    };
    let result: io::Result<Option<u32>> = (|| {
        let stdout = child.stdout.take().unwrap();
        let mut input = FdIo::new(stdout.as_fd())?;
        // NSS may return a canonical name; trust the fixed query, not a literal name match.
        let mut have_name = false;
        let mut have_digit = false;
        let mut id = 0u32;
        let mut colons = 0;
        let mut ended = false;
        let mut buffer = [0; 8192];
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            for &byte in &buffer[..count] {
                if colons < 3 {
                    if byte == b'\n' || byte == 0 {
                        return Err(io::ErrorKind::InvalidData.into());
                    }
                    if byte == b':' {
                        if (colons == 0 && !have_name) || (colons == 2 && !have_digit) {
                            return Err(io::ErrorKind::InvalidData.into());
                        }
                        colons += 1;
                    } else if colons == 0 {
                        have_name = true;
                    } else if colons == 2 {
                        if !byte.is_ascii_digit() {
                            return Err(io::ErrorKind::InvalidData.into());
                        }
                        id = id
                            .checked_mul(10)
                            .and_then(|id| id.checked_add(u32::from(byte - b'0')))
                            .ok_or(io::ErrorKind::InvalidData)?;
                        have_digit = true;
                    }
                } else if ended {
                    return Err(io::ErrorKind::InvalidData.into());
                } else if byte == b'\n' {
                    ended = true;
                }
            }
        }
        drop(input);
        drop(stdout);
        let status = loop {
            if shutdown_signal() != 0 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            // Poll only while our helper is alive, as with mapper child waits.
            match poll_interruptible(&mut [], 10) {
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted && shutdown_signal() == 0 => {}
                result => {
                    result?;
                }
            }
        };
        if !status.success() || colons != 3 {
            return Ok(None);
        }
        Ok(Some(id))
    })();
    if result.is_err() {
        let _ = child.kill();
        // Reap even after shutdown. SIGKILL cannot unblock kernel-uninterruptible I/O.
        loop {
            match child.wait() {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                _ => break,
            }
        }
    }
    if shutdown_signal() != 0 {
        Err(io::ErrorKind::Interrupted.into())
    } else {
        Ok(result.ok().flatten())
    }
}
