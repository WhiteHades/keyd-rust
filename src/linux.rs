// keyd - A key remapping daemon.
// © 2019 Raheman Vaiya (see also: LICENSE). Ported from frozen f564288.
//! Linux kernel ABI boundary. No C FFI or C-backed syscall library.

#[cfg(not(all(
    target_os = "linux",
    target_endian = "little",
    any(
        all(target_arch = "x86", target_pointer_width = "32"),
        all(
            target_pointer_width = "64",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )
)))]
compile_error!("the Rust device backend supports little-endian Linux i686/x86_64/aarch64");

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub const EV_SYN: u16 = 0;
pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const EV_ABS: u16 = 3;
pub const EV_LED: u16 = 17;
pub const EV_REP: u16 = 20;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const REL_Z: u16 = 2;
pub const REL_HWHEEL: u16 = 6;
pub const REL_WHEEL: u16 = 8;
pub const ABS_X: u16 = 0;
pub const ABS_Y: u16 = 1;
pub const KEY_MAX: usize = 0x2ff;
pub const KEY_MASK_SIZE: usize = KEY_MAX.div_ceil(8);
pub const IN_CREATE: u32 = 0x100;
pub const O_WRONLY: usize = 1;
pub const O_RDWR: usize = 2;
pub const O_NONBLOCK: usize = 0x800;
pub const O_CLOEXEC: usize = 0x80000;

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const READ: usize = 0;
    pub const WRITE: usize = 1;
    pub const IOCTL: usize = 16;
    pub const OPENAT: usize = 257;
    pub const INOTIFY_INIT1: usize = 294;
    pub const INOTIFY_ADD_WATCH: usize = 254;
    pub const PPOLL: usize = 271;
}

#[cfg(target_arch = "aarch64")]
mod nr {
    pub const READ: usize = 63;
    pub const WRITE: usize = 64;
    pub const IOCTL: usize = 29;
    pub const OPENAT: usize = 56;
    pub const INOTIFY_INIT1: usize = 26;
    pub const INOTIFY_ADD_WATCH: usize = 27;
    pub const PPOLL: usize = 73;
}

#[cfg(target_arch = "x86")]
mod nr {
    pub const READ: usize = 3;
    pub const WRITE: usize = 4;
    pub const IOCTL: usize = 54;
    pub const OPENAT: usize = 295;
    pub const INOTIFY_INIT1: usize = 332;
    pub const INOTIFY_ADD_WATCH: usize = 292;
    pub const PPOLL: usize = 309;
}

#[cfg(target_arch = "x86")]
#[unsafe(naked)]
unsafe extern "C" fn syscall_i386(
    number: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
) -> isize {
    // cdecl arguments stay on the stack; preserve all four callee-saved registers.
    core::arch::naked_asm!(
        "push ebx",
        "push esi",
        "push edi",
        "push ebp",
        "mov eax, [esp + 20]",
        "mov ebx, [esp + 24]",
        "mov ecx, [esp + 28]",
        "mov edx, [esp + 32]",
        "mov esi, [esp + 36]",
        "mov edi, [esp + 40]",
        "mov ebp, [esp + 44]",
        "int 0x80",
        "pop ebp",
        "pop edi",
        "pop esi",
        "pop ebx",
        "ret",
    );
}

/// # Safety
/// The caller must satisfy the selected syscall's pointer, lifetime, aliasing,
/// and descriptor-ownership requirements. This does not retry interruptions.
pub(crate) unsafe fn syscall6(number: usize, args: [usize; 6]) -> isize {
    let result;
    #[cfg(target_arch = "x86")]
    // SAFETY: the cdecl entry saves registers and passes all six kernel arguments.
    unsafe {
        result = syscall_i386(number, args[0], args[1], args[2], args[3], args[4], args[5]);
    }
    #[cfg(target_arch = "x86_64")]
    // SAFETY: the caller supplies the kernel ABI arguments; syscall clobbers
    // rcx/r11 and memory, and does not use the userspace stack.
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") number => result,
            in("rdi") args[0], in("rsi") args[1], in("rdx") args[2],
            in("r10") args[3], in("r8") args[4], in("r9") args[5],
            lateout("rcx") _, lateout("r11") _, options(nostack)
        );
    }
    #[cfg(target_arch = "aarch64")]
    // SAFETY: Linux uses x8 for the syscall number and x0..x5 for arguments.
    unsafe {
        core::arch::asm!(
            "svc 0",
            in("x8") number, inlateout("x0") args[0] => result,
            in("x1") args[1], in("x2") args[2], in("x3") args[3],
            in("x4") args[4], in("x5") args[5], options(nostack)
        );
    }
    result
}

/// # Safety
/// The caller supplies a socketcall operation and its live kernel ABI arguments.
#[cfg(target_arch = "x86")]
pub(crate) unsafe fn socketcall(operation: usize, args: [usize; 6]) -> isize {
    // SAFETY: the argument array lives through socketcall, including pointed-to buffers.
    unsafe { syscall6(102, [operation, args.as_ptr() as usize, 0, 0, 0, 0]) }
}

pub(crate) fn syscall_result(result: isize) -> io::Result<usize> {
    if (-4095..0).contains(&result) {
        Err(io::Error::from_raw_os_error(-result as i32))
    } else {
        Ok(result as usize)
    }
}

pub fn path_cstring(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}

pub fn open(path: &Path, flags: usize) -> io::Result<OwnedFd> {
    if flags & !(O_WRONLY | O_RDWR | O_NONBLOCK | O_CLOEXEC) != 0 || flags & 3 == 3 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let path = path_cstring(path)?;
    // SAFETY: the terminated path lives through openat; no creation is requested.
    let fd = syscall_result(unsafe {
        syscall6(
            nr::OPENAT,
            [
                (-100_isize) as usize,
                path.as_ptr() as usize,
                flags | O_CLOEXEC,
                0,
                0,
                0,
            ],
        )
    })?;
    // SAFETY: successful openat returns a new, uniquely owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

pub fn read(fd: BorrowedFd<'_>, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: the descriptor stays open and the writable slice lives through read.
    syscall_result(unsafe {
        syscall6(
            nr::READ,
            [
                fd.as_raw_fd() as usize,
                buffer.as_mut_ptr() as usize,
                buffer.len(),
                0,
                0,
                0,
            ],
        )
    })
}

pub fn write(fd: BorrowedFd<'_>, buffer: &[u8]) -> io::Result<usize> {
    // SAFETY: the descriptor stays open and the initialized slice lives through write.
    syscall_result(unsafe {
        syscall6(
            nr::WRITE,
            [
                fd.as_raw_fd() as usize,
                buffer.as_ptr() as usize,
                buffer.len(),
                0,
                0,
                0,
            ],
        )
    })
}

pub fn write_all(fd: BorrowedFd<'_>, mut buffer: &[u8]) -> io::Result<()> {
    while !buffer.is_empty() {
        match write(fd, buffer) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => buffer = &buffer[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

pub const fn ioctl_request(direction: usize, kind: u8, number: u8, size: usize) -> usize {
    (direction << 30) | (size << 16) | ((kind as usize) << 8) | number as usize
}

unsafe fn ioctl(fd: BorrowedFd<'_>, request: usize, arg: usize) -> io::Result<usize> {
    // SAFETY: callers below supply valid buffers or scalar arguments for fixed requests.
    syscall_result(unsafe { syscall6(nr::IOCTL, [fd.as_raw_fd() as usize, request, arg, 0, 0, 0]) })
}

fn evdev_read(fd: BorrowedFd<'_>, number: u8, buffer: &mut [u8]) -> io::Result<usize> {
    // SAFETY: only EVIOCG* byte-copy requests call this; request length equals slice length.
    unsafe {
        ioctl(
            fd,
            ioctl_request(2, b'E', number, buffer.len()),
            buffer.as_mut_ptr() as usize,
        )
    }
}

pub fn evdev_bits(fd: BorrowedFd<'_>, kind: u16, buffer: &mut [u8]) -> io::Result<usize> {
    if kind > 0x1f || buffer.len() > 0x3fff {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    evdev_read(fd, 0x20 + kind as u8, buffer)
}

pub fn evdev_name(fd: BorrowedFd<'_>) -> io::Result<[u8; 64]> {
    let mut name = [0; 64];
    evdev_read(fd, 0x06, &mut name)?;
    name[63] = 0;
    Ok(name)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputId {
    pub bustype: u16,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
}

pub fn evdev_id(fd: BorrowedFd<'_>) -> io::Result<InputId> {
    let mut bytes = [0; 8];
    evdev_read(fd, 0x02, &mut bytes)?;
    Ok(InputId {
        bustype: u16::from_ne_bytes(bytes[0..2].try_into().unwrap()),
        vendor: u16::from_ne_bytes(bytes[2..4].try_into().unwrap()),
        product: u16::from_ne_bytes(bytes[4..6].try_into().unwrap()),
        version: u16::from_ne_bytes(bytes[6..8].try_into().unwrap()),
    })
}

pub fn evdev_abs_bounds(fd: BorrowedFd<'_>, axis: u16) -> io::Result<(u32, u32)> {
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

pub fn evdev_key_state(fd: BorrowedFd<'_>, state: &mut [u8; KEY_MASK_SIZE]) -> io::Result<()> {
    evdev_read(fd, 0x18, state).map(|_| ())
}

pub fn evdev_grab(fd: BorrowedFd<'_>, grab: bool) -> io::Result<()> {
    // SAFETY: EVIOCGRAB takes a scalar, despite its _IOW(int) encoding.
    unsafe { ioctl(fd, ioctl_request(1, b'E', 0x90, 4), grab as usize).map(|_| ()) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UinputBit {
    Event,
    Key,
    Relative,
    Absolute,
    Led,
}

pub fn uinput_set_bit(fd: BorrowedFd<'_>, kind: UinputBit, code: u16) -> io::Result<()> {
    let number = match kind {
        UinputBit::Event => 100,
        UinputBit::Key => 101,
        UinputBit::Relative => 102,
        UinputBit::Absolute => 103,
        UinputBit::Led => 105,
    };
    // SAFETY: these UI_SET_*BIT requests take a scalar code.
    unsafe { ioctl(fd, ioctl_request(1, b'U', number, 4), code as usize).map(|_| ()) }
}

pub fn uinput_create(fd: BorrowedFd<'_>) -> io::Result<()> {
    // SAFETY: UI_DEV_CREATE has no pointer argument.
    unsafe { ioctl(fd, ioctl_request(0, b'U', 1, 0), 0).map(|_| ()) }
}

pub fn inotify_create() -> io::Result<OwnedFd> {
    // SAFETY: inotify_init1 takes only scalar flags and returns a new descriptor.
    let fd = syscall_result(unsafe {
        syscall6(nr::INOTIFY_INIT1, [O_NONBLOCK | O_CLOEXEC, 0, 0, 0, 0, 0])
    })?;
    // SAFETY: successful inotify_init1 transfers a new descriptor to us.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

pub fn inotify_watch(fd: BorrowedFd<'_>, path: &CStr) -> io::Result<i32> {
    // SAFETY: the NUL-terminated path lives through the call; fd stays open.
    syscall_result(unsafe {
        syscall6(
            nr::INOTIFY_ADD_WATCH,
            [
                fd.as_raw_fd() as usize,
                path.as_ptr() as usize,
                IN_CREATE as usize,
                0,
                0,
                0,
            ],
        )
    })
    .map(|wd| wd as i32)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InputEvent {
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

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

pub const POLLIN: i16 = 1;
pub const POLLERR: i16 = 8;

pub fn poll(fds: &mut [PollFd], timeout_ms: i32) -> io::Result<usize> {
    let mut timeout = [
        timeout_ms as isize / 1000,
        timeout_ms as isize % 1000 * 1_000_000,
    ];
    let timeout_ptr = if timeout_ms < 0 {
        0
    } else {
        timeout.as_mut_ptr() as usize
    };
    // SAFETY: pollfd and the two-native-word timespec have the gated Linux ABI;
    // both slices live through ppoll, and no signal mask is supplied.
    syscall_result(unsafe {
        syscall6(
            nr::PPOLL,
            [fds.as_mut_ptr() as usize, fds.len(), timeout_ptr, 0, 0, 0],
        )
    })
}
