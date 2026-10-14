// keyd © 2019 Raheman Vaiya. FreeBSD evdev/uinput boundary. See LICENSE.
use crate::freebsd::{syscall_result, syscall6};
use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd},
};

pub(crate) const IOCTL: usize = 54;
pub(crate) const IOC_VOID: usize = 0x2000_0000;
pub(crate) const IOC_OUT: usize = 0x4000_0000;
pub(crate) const IOCPARM_MASK: usize = 0x1fff;
pub(crate) const EV_SYN: u16 = 0;
pub(crate) const EV_KEY: u16 = 1;
pub(crate) const EV_REL: u16 = 2;
pub(crate) const EV_ABS: u16 = 3;
pub(crate) const EV_LED: u16 = 17;
pub(crate) const EV_REP: u16 = 20;
pub(crate) const REL_X: u16 = 0;
pub(crate) const REL_Y: u16 = 1;
pub(crate) const REL_Z: u16 = 2;
pub(crate) const REL_HWHEEL: u16 = 6;
pub(crate) const REL_WHEEL: u16 = 8;
pub(crate) const ABS_X: u16 = 0;
pub(crate) const ABS_Y: u16 = 1;
pub(crate) const KEY_MAX: usize = 0x2ff;
pub(crate) const KEY_MASK_SIZE: usize = KEY_MAX.div_ceil(8);

pub(crate) const fn ioctl_request(direction: usize, group: u8, number: u8, size: usize) -> usize {
    direction | ((size & IOCPARM_MASK) << 16) | ((group as usize) << 8) | number as usize
}

/// # Safety
/// The argument must be a live ABI buffer or scalar of the selected request's form.
unsafe fn ioctl(fd: BorrowedFd<'_>, request: usize, argument: usize) -> io::Result<usize> {
    // SAFETY: the caller supplies the selected request's argument and keeps fd open.
    syscall_result(unsafe {
        syscall6(IOCTL, [fd.as_raw_fd() as usize, request, argument, 0, 0, 0])
    })
}

fn evdev_read(fd: BorrowedFd<'_>, number: u8, bytes: &mut [u8]) -> io::Result<usize> {
    if bytes.len() > IOCPARM_MASK {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    // SAFETY: these fixed EVIOCG* requests write exactly the encoded buffer length.
    unsafe {
        ioctl(
            fd,
            ioctl_request(IOC_OUT, b'E', number, bytes.len()),
            bytes.as_mut_ptr() as usize,
        )
    }
}

pub(crate) fn evdev_bits(fd: BorrowedFd<'_>, kind: u16, bytes: &mut [u8]) -> io::Result<usize> {
    if kind > 0x1f {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    evdev_read(fd, 0x20 + kind as u8, bytes)
}

pub(crate) fn evdev_name(fd: BorrowedFd<'_>) -> io::Result<[u8; 64]> {
    let mut name = [0; 64];
    evdev_read(fd, 0x06, &mut name)?;
    name[63] = 0;
    Ok(name)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InputId {
    pub bustype: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
}

pub(crate) fn evdev_id(fd: BorrowedFd<'_>) -> io::Result<InputId> {
    let mut bytes = [0; 8];
    evdev_read(fd, 0x02, &mut bytes)?;
    Ok(InputId {
        bustype: u16::from_ne_bytes(bytes[0..2].try_into().unwrap()),
        vendor: u16::from_ne_bytes(bytes[2..4].try_into().unwrap()),
        product: u16::from_ne_bytes(bytes[4..6].try_into().unwrap()),
        version: u16::from_ne_bytes(bytes[6..8].try_into().unwrap()),
    })
}

pub(crate) fn evdev_abs_bounds(fd: BorrowedFd<'_>, axis: u16) -> io::Result<(u32, u32)> {
    if axis > 0x3f {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut bytes = [0; 24];
    evdev_read(fd, 0x40 + axis as u8, &mut bytes)?;
    Ok((
        u32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
        u32::from_ne_bytes(bytes[8..12].try_into().unwrap()),
    ))
}

pub(crate) fn evdev_key_state(
    fd: BorrowedFd<'_>,
    state: &mut [u8; KEY_MASK_SIZE],
) -> io::Result<()> {
    evdev_read(fd, 0x18, state).map(|_| ())
}

pub(crate) fn evdev_grab(fd: BorrowedFd<'_>, grab: bool) -> io::Result<()> {
    // SAFETY: FreeBSD EVIOCGRAB uses _IOWINT and takes a scalar, not int *.
    unsafe { ioctl(fd, ioctl_request(IOC_VOID, b'E', 0x90, 4), grab as usize).map(|_| ()) }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum UinputBit {
    Event,
    Key,
    Relative,
    Absolute,
    Led,
}

pub(crate) fn uinput_set_bit(fd: BorrowedFd<'_>, kind: UinputBit, code: u16) -> io::Result<()> {
    let number = match kind {
        UinputBit::Event => 100,
        UinputBit::Key => 101,
        UinputBit::Relative => 102,
        UinputBit::Absolute => 103,
        UinputBit::Led => 105,
    };
    // SAFETY: every supported UI_SET_*BIT uses FreeBSD _IOWINT's scalar ABI.
    unsafe { ioctl(fd, ioctl_request(IOC_VOID, b'U', number, 4), code as usize).map(|_| ()) }
}

pub(crate) fn uinput_create(fd: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: UI_DEV_CREATE is an IOC_VOID request with no argument.
    unsafe { ioctl(fd, ioctl_request(IOC_VOID, b'U', 1, 0), 0).map(|_| ()) }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InputEvent {
    pub seconds: isize,
    pub microseconds: isize,
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEvent {
    pub const SIZE: usize = 2 * size_of::<isize>() + 8;

    pub const fn new(kind: u16, code: u16, value: i32) -> Self {
        Self {
            seconds: 0,
            microseconds: 0,
            kind,
            code,
            value,
        }
    }

    pub fn encode(self) -> [u8; Self::SIZE] {
        let mut bytes = [0; Self::SIZE];
        let word = size_of::<isize>();
        bytes[..word].copy_from_slice(&self.seconds.to_ne_bytes());
        bytes[word..2 * word].copy_from_slice(&self.microseconds.to_ne_bytes());
        bytes[2 * word..2 * word + 2].copy_from_slice(&self.kind.to_ne_bytes());
        bytes[2 * word + 2..2 * word + 4].copy_from_slice(&self.code.to_ne_bytes());
        bytes[2 * word + 4..].copy_from_slice(&self.value.to_ne_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() != Self::SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid input_event size",
            ));
        }
        let word = size_of::<isize>();
        Ok(Self {
            seconds: isize::from_ne_bytes(bytes[..word].try_into().unwrap()),
            microseconds: isize::from_ne_bytes(bytes[word..2 * word].try_into().unwrap()),
            kind: u16::from_ne_bytes(bytes[2 * word..2 * word + 2].try_into().unwrap()),
            code: u16::from_ne_bytes(bytes[2 * word + 2..2 * word + 4].try_into().unwrap()),
            value: i32::from_ne_bytes(bytes[2 * word + 4..].try_into().unwrap()),
        })
    }
}
