// GNOME's extension owns the FIFO protocol and remains JavaScript. See LICENSE.
use crate::{linux, runtime};
use std::io::{self, Read};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};

pub struct Monitor {
    path: PathBuf,
}

impl Monitor {
    pub fn connect() -> io::Result<Self> {
        if !std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase()
            .contains("gnome")
            && std::env::var_os("GNOME_SETUP_DISPLAY").is_none_or(|value| value.is_empty())
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "GNOME desktop environment not detected",
            ));
        }
        let directory = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                #[cfg(target_arch = "x86_64")]
                const GETUID: usize = 102;
                #[cfg(target_arch = "aarch64")]
                const GETUID: usize = 174;
                #[cfg(target_arch = "x86")]
                const GETUID: usize = 199;
                // SAFETY: getuid has no arguments and cannot fail.
                let uid = unsafe { linux::syscall6(GETUID, [0; 6]) as u32 };
                PathBuf::from(format!("/run/user/{uid}"))
            });
        Ok(Self {
            path: directory.join("keyd.fifo"),
        })
    }

    pub fn init(&self) -> io::Result<bool> {
        if !self.path.exists() {
            super::output(format_args!(
                r#"Gnome extension doesn't appear to be running:

You will need to install the keyd gnome extension in order for the
application mapper to work correctly.

This can usually be achieved by running:

rm -r ~/.local/share/gnome-shell/extensions/keyd # Remove any older versions of the extension
mkdir -p ~/.local/share/gnome-shell/extensions

Followed by:

Gnome 42-44:
    ln -s /usr/local/share/keyd/gnome-extension ~/.local/share/gnome-shell/extensions/keyd

Gnome 45/46:
    ln -s /usr/local/share/keyd/gnome-extension-45 ~/.local/share/gnome-shell/extensions/keyd

Finally restart Gnome and run:

            gnome-extensions enable keyd
            gnome-extensions show keyd (verify the extension is enabled)

NOTE:
    You may need to adjust the above paths (e.g /usr/share/keyd/gnome-extension)
    depending on your distro.
"#
            ))?;
            return Ok(false);
        }
        Ok(true)
    }

    pub fn run(
        &mut self,
        on_change: impl FnMut(&str, &str) -> io::Result<()>,
        wait: impl FnMut(BorrowedFd<'_>) -> io::Result<bool>,
    ) -> io::Result<()> {
        let fd = linux::open(Path::new(&self.path), linux::O_NONBLOCK)?;
        Self::read(fd, on_change, wait)
    }

    pub(super) fn read(
        fd: OwnedFd,
        mut on_change: impl FnMut(&str, &str) -> io::Result<()>,
        mut wait: impl FnMut(BorrowedFd<'_>) -> io::Result<bool>,
    ) -> io::Result<()> {
        let mut last = (String::new(), String::new());
        let mut buffer = [0; 4096];
        let mut pending = Vec::new();
        let mut input = runtime::FdIo::new(fd.as_fd())?;
        loop {
            if wait(fd.as_fd())? {
                on_change(&last.0, &last.1)?;
                continue;
            }
            let count = input.read(&mut buffer)?;
            if count == 0 {
                if !pending.is_empty() {
                    Self::line(&pending, &mut last, &mut on_change)?;
                }
                return Ok(());
            }
            pending.extend_from_slice(&buffer[..count]);
            while let Some(end) = pending.iter().position(|&byte| byte == b'\n') {
                Self::line(&pending[..end], &mut last, &mut on_change)?;
                pending.drain(..=end);
            }
            if pending.len() > 1024 * 1024 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "GNOME window record exceeds 1 MiB",
                ));
            }
        }
    }

    fn line(
        bytes: &[u8],
        last: &mut (String, String),
        on_change: &mut impl FnMut(&str, &str) -> io::Result<()>,
    ) -> io::Result<()> {
        let line = std::str::from_utf8(bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let mut parts = line.split('\t');
        let class = parts.next().unwrap();
        if let Some(title) = parts.next()
            && parts.next().is_none()
        {
            *last = (class.into(), title.into());
            on_change(class, title)
        } else {
            on_change("", "")
        }
    }
}
