use crate::{Error, Result};
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Child, Command},
    ptr::NonNull,
};

const SEALS: i32 = libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
const MARKER: u8 = 0xEB;

/// One bounded writable output, exclusively owned until native decode completes.
pub(crate) struct MutablePlane {
    pointer: Option<NonNull<u8>>,
    bytes: usize,
    descriptor: Option<OwnedFd>,
}
impl MutablePlane {
    /// Reserve one zero-initialized shared output before native decode.
    /// `bytes` is budgeted RGBA geometry. Returns an exclusively writable mapping.
    pub(crate) fn new(bytes: usize) -> Result<Self> {
        if bytes == 0 || bytes > 8192 * 8192 * 4 {
            return Err(Error::Invalid(
                "shared output geometry exceeds limits".into(),
            ));
        }
        let raw = unsafe {
            libc::memfd_create(
                c"editbay-rgba".as_ptr(),
                libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
            )
        };
        if raw < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let descriptor = unsafe { OwnedFd::from_raw_fd(raw) };
        let file = File::from(descriptor);
        file.set_len(bytes as u64)?;
        let pointer = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if pointer == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        let Some(pointer) = NonNull::new(pointer.cast()) else {
            unsafe {
                libc::munmap(pointer, bytes);
            }
            return Err(Error::Invalid("null writable mapping".into()));
        };
        Ok(Self {
            pointer: Some(pointer),
            bytes,
            descriptor: Some(file.into()),
        })
    }
    pub(crate) fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe {
            std::slice::from_raw_parts_mut(
                self.pointer
                    .expect("unsealed output owns its mapping")
                    .as_ptr(),
                self.bytes,
            )
        }
    }
    /// Finish native output and permanently disable writes before sharing.
    /// Takes this output. Returns a read-only mapping after releasing its writable one.
    pub(crate) fn seal(mut self) -> Result<Plane> {
        if let Some(pointer) = self.pointer.take() {
            unsafe {
                libc::munmap(pointer.as_ptr().cast(), self.bytes);
            }
        }
        let descriptor = self
            .descriptor
            .take()
            .ok_or_else(|| Error::Invalid("shared output descriptor absent".into()))?;
        if unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_ADD_SEALS, SEALS) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Plane::map(descriptor, self.bytes)
    }
}
impl Drop for MutablePlane {
    fn drop(&mut self) {
        if let Some(pointer) = self.pointer.take() {
            unsafe {
                libc::munmap(pointer.as_ptr().cast(), self.bytes);
            }
        }
    }
}

/// Sealed kernel-owned bytes mapped read-only for their entire borrowed lifetime.
pub(crate) struct Plane {
    pointer: NonNull<u8>,
    bytes: usize,
    descriptor: OwnedFd,
}

unsafe impl Send for Plane {}
unsafe impl Sync for Plane {}

impl Plane {
    /// Publish immutable pixels into an anonymous, sealed local file.
    /// `pixels` supplies exact test bytes. Returns a read-only sealed mapping.
    #[cfg(test)]
    pub(crate) fn create(pixels: &[u8]) -> Result<Self> {
        let mut output = MutablePlane::new(pixels.len())?;
        output.bytes_mut().copy_from_slice(pixels);
        output.seal()
    }

    /// Validate geometry and kernel-enforced immutability before borrowing bytes.
    /// `descriptor` transfers ownership; `bytes` is checked RGBA geometry.
    /// Returns a read-only mapping, refusing mutable, truncated or foreign files.
    pub(crate) fn map(descriptor: OwnedFd, bytes: usize) -> Result<Self> {
        Self::validate(&descriptor, bytes)?;
        let pointer = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ,
                libc::MAP_SHARED,
                descriptor.as_raw_fd(),
                0,
            )
        };
        if pointer == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error().into());
        }
        let Some(pointer) = NonNull::new(pointer.cast()) else {
            unsafe {
                libc::munmap(pointer, bytes);
            }
            return Err(Error::Invalid("null plane mapping".into()));
        };
        Ok(Self {
            pointer,
            bytes,
            descriptor,
        })
    }

    /// Check a received descriptor before mapping or reusing matching content.
    /// `descriptor` remains owned by the caller; `bytes` is exact expected size.
    /// Returns success only for a sealed regular file of that length.
    pub(crate) fn validate(descriptor: &OwnedFd, bytes: usize) -> Result<()> {
        if bytes == 0 || bytes > 8192 * 8192 * 4 {
            return Err(Error::Invalid(
                "shared plane exceeds geometry limits".into(),
            ));
        }
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(descriptor.as_raw_fd(), &mut stat) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let seals = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GET_SEALS) };
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG
            || stat.st_size != bytes as i64
            || seals < 0
            || seals & SEALS != SEALS
        {
            return Err(Error::Invalid(
                "shared plane is mutable, nonregular or has the wrong length".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.pointer.as_ptr(), self.bytes) }
    }
    pub(crate) fn descriptor(&self) -> &OwnedFd {
        &self.descriptor
    }
}

impl Drop for Plane {
    fn drop(&mut self) {
        unsafe {
            libc::munmap(self.pointer.as_ptr().cast(), self.bytes);
        }
    }
}

/// Start a child with exactly one private socket available at descriptor 3.
/// `command` specifies the packaged executable; `socket` stays alive until exec.
/// Returns the child; all other Rust-owned source/plane descriptors remain CLOEXEC.
pub(crate) fn spawn(command: &mut Command, socket: &UnixStream) -> Result<Child> {
    let descriptor = socket.as_raw_fd();
    let parent = std::process::id();
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                || libc::getppid() as u32 != parent
            {
                return Err(std::io::Error::other("picture worker lost its parent"));
            }
            if descriptor != 3 && libc::dup2(descriptor, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(command.spawn()?)
}

/// Take the packaged child's private inherited socket and restore CLOEXEC.
/// Takes no arguments. Returns a validated stream descriptor, owned once.
pub(crate) fn inherited() -> Result<UnixStream> {
    let mut kind: i32 = 0;
    let mut size = std::mem::size_of::<i32>() as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            3,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut i32).cast(),
            &mut size,
        )
    } != 0
        || kind != libc::SOCK_STREAM
    {
        return Err(Error::Invalid(
            "picture worker needs its private stream socket".into(),
        ));
    }
    let socket = unsafe { UnixStream::from_raw_fd(3) };
    if unsafe { libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(socket)
}

/// Send bounded metadata with an optional single plane descriptor.
/// `socket` is private, `bytes` is serialized metadata and `descriptor` is borrowed.
/// Returns after the complete length-framed message; no pixel bytes enter JSON.
pub(crate) fn send(
    socket: &mut UnixStream,
    bytes: &[u8],
    descriptor: Option<&OwnedFd>,
) -> Result<()> {
    if bytes.is_empty() || bytes.len() > crate::worker::MESSAGE_BYTES as usize {
        return Err(Error::Invalid(
            "picture metadata exceeds its wire budget".into(),
        ));
    }
    let mut marker = MARKER;
    let mut vector = libc::iovec {
        iov_base: (&mut marker as *mut u8).cast(),
        iov_len: 1,
    };
    let mut control = [0usize; 8];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    if let Some(descriptor) = descriptor {
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen =
            unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) } as usize;
        unsafe {
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as usize;
            std::ptr::write_unaligned(
                libc::CMSG_DATA(header).cast::<i32>(),
                descriptor.as_raw_fd(),
            );
        }
    }
    loop {
        let sent = unsafe { libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) };
        if sent == 1 {
            break;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
    socket.write_all(&(bytes.len() as u32).to_be_bytes())?;
    socket.write_all(bytes)?;
    Ok(())
}

/// Receive one bounded metadata message and own every received descriptor.
/// `socket` supplies a private ordered stream. Returns EOF or one complete message;
/// ancillary truncation, extra handles and malformed framing release all handles.
pub(crate) fn receive(socket: &mut UnixStream) -> Result<Option<(Vec<u8>, Option<OwnedFd>)>> {
    let mut marker = 0u8;
    let mut vector = libc::iovec {
        iov_base: (&mut marker as *mut u8).cast(),
        iov_len: 1,
    };
    let mut control = [0usize; 16];
    let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
    message.msg_iov = &mut vector;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = std::mem::size_of_val(&control);
    let received = loop {
        let received =
            unsafe { libc::recvmsg(socket.as_raw_fd(), &mut message, libc::MSG_CMSG_CLOEXEC) };
        if received >= 0 {
            break received;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    };
    let mut descriptors = Vec::new();
    let mut unexpected = false;
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            let length = (*header).cmsg_len;
            let base = libc::CMSG_LEN(0) as usize;
            if length < base {
                unexpected = true;
                break;
            }
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let count = (length - base) / std::mem::size_of::<i32>();
                for i in 0..count {
                    descriptors.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(
                        libc::CMSG_DATA(header).cast::<i32>().add(i),
                    )));
                }
            } else {
                unexpected = true;
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    if received == 0 {
        return Ok(None);
    }
    if marker != MARKER
        || unexpected
        || descriptors.len() > 1
        || message.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0
    {
        return Err(Error::Invalid("invalid picture descriptor envelope".into()));
    }
    let descriptor = descriptors.pop();
    let mut length = [0u8; 4];
    socket.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > crate::worker::MESSAGE_BYTES as usize {
        return Err(Error::Invalid(
            "picture metadata exceeds its read budget".into(),
        ));
    }
    let mut bytes = vec![0u8; length];
    socket.read_exact(&mut bytes)?;
    Ok(Some((bytes, descriptor)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sealed_planes_survive_sender_drop_and_refuse_mutation_geometry_and_unsealed_files() {
        let original = Plane::create(&[1, 2, 3, 255]).unwrap();
        let (mut a, mut b) = UnixStream::pair().unwrap();
        send(&mut a, b"{}", Some(original.descriptor())).unwrap();
        let (bytes, descriptor) = receive(&mut b).unwrap().unwrap();
        assert_eq!(bytes, b"{}");
        let descriptor = descriptor.unwrap();
        assert_ne!(
            unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        assert!(Plane::validate(&descriptor, 8).is_err());
        assert_eq!(unsafe { libc::ftruncate(descriptor.as_raw_fd(), 0) }, -1);
        assert_eq!(
            unsafe { libc::pwrite(descriptor.as_raw_fd(), b"x".as_ptr().cast(), 1, 0) },
            -1
        );
        let held = Plane::map(descriptor, 4).unwrap();
        drop(original);
        assert_eq!(held.bytes(), &[1, 2, 3, 255]);
        let unsealed = unsafe { libc::memfd_create(c"unsealed".as_ptr(), libc::MFD_CLOEXEC) };
        let mut file = unsafe { File::from_raw_fd(unsealed) };
        file.write_all(&[1, 2, 3, 4]).unwrap();
        assert!(Plane::map(file.into(), 4).is_err());
    }

    #[test]
    fn rejected_extra_and_truncated_ancillary_handles_are_all_closed() {
        use std::os::unix::fs::MetadataExt;
        let plane = Plane::create(&[1, 2, 3, 255]).unwrap();
        let inode = std::fs::metadata(format!("/proc/self/fd/{}", plane.descriptor().as_raw_fd()))
            .unwrap()
            .ino();
        let count = || {
            std::fs::read_dir("/proc/self/fd")
                .unwrap()
                .filter_map(|e| e.ok())
                .filter_map(|e| std::fs::metadata(e.path()).ok())
                .filter(|m| m.ino() == inode)
                .count()
        };
        for copies in [2, 40] {
            let (a, mut b) = UnixStream::pair().unwrap();
            let mut marker = MARKER;
            let mut vector = libc::iovec {
                iov_base: (&mut marker as *mut u8).cast(),
                iov_len: 1,
            };
            let mut control = [0usize; 64];
            let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
            message.msg_iov = &mut vector;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = unsafe { libc::CMSG_SPACE(copies * 4) } as usize;
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(copies * 4) as usize;
                for i in 0..copies {
                    std::ptr::write_unaligned(
                        libc::CMSG_DATA(header).cast::<i32>().add(i as usize),
                        plane.descriptor().as_raw_fd(),
                    );
                }
            }
            assert_eq!(count(), 1);
            assert_eq!(
                unsafe { libc::sendmsg(a.as_raw_fd(), &message, libc::MSG_NOSIGNAL) },
                1
            );
            assert!(receive(&mut b).is_err());
            assert_eq!(count(), 1);
        }
    }

    #[test]
    fn metadata_framing_refuses_oversized_and_truncated_input_before_parse() {
        for length in [crate::worker::MESSAGE_BYTES as u32 + 1, 4] {
            let (mut a, mut b) = UnixStream::pair().unwrap();
            a.write_all(&[MARKER]).unwrap();
            a.write_all(&length.to_be_bytes()).unwrap();
            a.write_all(b"{").unwrap();
            a.shutdown(std::net::Shutdown::Write).unwrap();
            assert!(receive(&mut b).is_err());
        }
    }
}
