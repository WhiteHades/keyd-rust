// Rust port of keyd's evloop.c. © 2019 Raheman Vaiya. See LICENSE.

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, RawFd};

use crate::device::{self, Device, DeviceEvent, DeviceMonitor, MAX_DEVICES};
use crate::keys::{KEYD_BACKSPACE, KEYD_ENTER, KEYD_ESC};
use crate::linux::{self, POLLERR, POLLIN, PollFd};

const MAX_AUX_FDS: usize = 32;

pub struct DeviceSlot {
    pub device: Device,
    pub owner: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    DeviceAdd(usize),
    DeviceRemove(usize),
    DeviceInput(usize, DeviceEvent),
    FdActivity(RawFd),
    FdError(RawFd),
    Timeout,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub kind: EventKind,
    pub timestamp: i32,
}

pub struct EventLoop<'fd> {
    monitor: DeviceMonitor,
    pub devices: Vec<DeviceSlot>,
    auxiliary: [Option<BorrowedFd<'fd>>; MAX_AUX_FDS],
    nr_auxiliary: usize,
    timeout: i32,
    panic_keys: [u8; 3],
}

pub fn monotonic_ms() -> io::Result<i64> {
    #[cfg(target_arch = "x86")]
    const CLOCK_GETTIME: usize = 265;
    #[cfg(target_arch = "x86_64")]
    const CLOCK_GETTIME: usize = 228;
    #[cfg(target_arch = "aarch64")]
    const CLOCK_GETTIME: usize = 113;
    let mut timespec = [0isize; 2];
    // SAFETY: CLOCK_MONOTONIC writes two aligned native words to this live array.
    linux::syscall_result(unsafe {
        linux::syscall6(
            CLOCK_GETTIME,
            [1, timespec.as_mut_ptr() as usize, 0, 0, 0, 0],
        )
    })?;
    Ok(timespec[0] as i64 * 1000 + timespec[1] as i64 / 1_000_000)
}

impl<'fd> EventLoop<'fd> {
    pub fn new() -> io::Result<Self> {
        let monitor = DeviceMonitor::create()?;
        Self::from_devices(monitor, device::device_scan()?)
    }

    pub fn from_devices(monitor: DeviceMonitor, devices: Vec<Device>) -> io::Result<Self> {
        if devices.len() > MAX_DEVICES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "maximum devices exceeded",
            ));
        }
        Ok(Self {
            monitor,
            devices: devices
                .into_iter()
                .map(|device| DeviceSlot {
                    device,
                    owner: None,
                })
                .collect(),
            auxiliary: [None; MAX_AUX_FDS],
            nr_auxiliary: 0,
            timeout: 0,
            panic_keys: [0; 3],
        })
    }

    pub fn add_fd(&mut self, fd: BorrowedFd<'fd>) -> io::Result<()> {
        if self.nr_auxiliary == MAX_AUX_FDS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "maximum auxiliary descriptors exceeded",
            ));
        }
        self.auxiliary[self.nr_auxiliary] = Some(fd);
        self.nr_auxiliary += 1;
        Ok(())
    }

    pub fn announce_devices(
        &mut self,
        mut handler: impl FnMut(Event, &mut [DeviceSlot]) -> io::Result<i32>,
    ) -> io::Result<()> {
        for i in 0..self.devices.len() {
            handler(
                Event {
                    kind: EventKind::DeviceAdd(i),
                    timestamp: 0,
                },
                &mut self.devices,
            )?;
        }
        Ok(())
    }

    fn panic_check(&mut self, code: u8, pressed: u8) -> io::Result<()> {
        let index = match code {
            KEYD_ENTER => Some(0),
            KEYD_BACKSPACE => Some(1),
            KEYD_ESC => Some(2),
            _ => None,
        };
        if let Some(index) = index {
            self.panic_keys[index] = pressed;
        }
        if self.panic_keys.iter().all(|&state| state != 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "panic sequence detected",
            ));
        }
        Ok(())
    }

    pub fn step(
        &mut self,
        mut handler: impl FnMut(Event, &mut [DeviceSlot]) -> io::Result<i32>,
    ) -> io::Result<()> {
        let mut pollfds = [PollFd {
            fd: -1,
            events: 0,
            revents: 0,
        }; MAX_DEVICES + MAX_AUX_FDS + 1];
        pollfds[0] = PollFd {
            fd: self.monitor.fd().as_raw_fd(),
            events: POLLIN,
            revents: 0,
        };
        let device_count = self.devices.len();
        for (i, slot) in self.devices.iter().enumerate() {
            pollfds[i + 1] = PollFd {
                fd: slot.device.fd().map_or(-1, |fd| fd.as_raw_fd()),
                events: POLLIN | POLLERR,
                revents: 0,
            };
        }
        for i in 0..self.nr_auxiliary {
            pollfds[device_count + i + 1] = PollFd {
                fd: self.auxiliary[i].unwrap().as_raw_fd(),
                events: POLLIN | POLLERR,
                revents: 0,
            };
        }
        let start = monotonic_ms()? as i32;
        crate::runtime::poll_interruptible(
            &mut pollfds[..device_count + self.nr_auxiliary + 1],
            if self.timeout > 0 { self.timeout } else { -1 },
        )?;
        let timestamp = monotonic_ms()? as i32;
        let elapsed = timestamp.wrapping_sub(start);
        if self.timeout > 0 && elapsed >= self.timeout {
            self.timeout = handler(
                Event {
                    kind: EventKind::Timeout,
                    timestamp,
                },
                &mut self.devices,
            )?;
        } else {
            self.timeout = self.timeout.wrapping_sub(elapsed);
        }

        let mut removed = false;
        for i in 0..device_count {
            if pollfds[i + 1].revents == 0 {
                continue;
            }
            while let Some(input) = self.devices[i].device.read_event()? {
                if input == DeviceEvent::Removed {
                    self.timeout = handler(
                        Event {
                            kind: EventKind::DeviceRemove(i),
                            timestamp,
                        },
                        &mut self.devices,
                    )?;
                    removed = true;
                    break;
                }
                if !self.devices[i].device.metadata.is_virtual
                    && let DeviceEvent::Key { code, pressed } = input
                {
                    self.panic_check(code, pressed)?;
                }
                self.timeout = handler(
                    Event {
                        kind: EventKind::DeviceInput(i, input),
                        timestamp,
                    },
                    &mut self.devices,
                )?;
            }
        }
        for i in 0..self.nr_auxiliary {
            let pollfd = pollfds[device_count + i + 1];
            if pollfd.revents != 0 {
                let kind = if pollfd.revents & POLLERR != 0 {
                    EventKind::FdError(pollfd.fd)
                } else {
                    EventKind::FdActivity(pollfd.fd)
                };
                self.timeout = handler(Event { kind, timestamp }, &mut self.devices)?;
            }
        }
        if pollfds[0].revents != 0 {
            while let Some(device) = self.monitor.read_device()? {
                if self.devices.len() == MAX_DEVICES {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "maximum devices exceeded",
                    ));
                }
                self.devices.push(DeviceSlot {
                    device,
                    owner: None,
                });
                let kind = EventKind::DeviceAdd(self.devices.len() - 1);
                self.timeout = handler(Event { kind, timestamp }, &mut self.devices)?;
            }
        }
        if removed {
            self.devices.retain(|slot| slot.device.fd().is_some());
        }
        Ok(())
    }
}
