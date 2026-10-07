//! Kernel exit notification plus an eventfd for cancellation; epoll blocks forever.
use super::StreamCancel;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;

fn owned(fd: i32) -> io::Result<OwnedFd> {
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}
pub fn watch(pid: u32, identity: u64, cancel: StreamCancel) -> io::Result<Arc<OwnedFd>> {
    let pidfd = owned(unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 })?;
    if crate::system::process::process_identity(pid)? != identity {
        return Err(io::Error::other(
            "Process identity changed before log streaming",
        ));
    }
    let wake = Arc::new(owned(unsafe {
        libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK)
    })?);
    let epoll = owned(unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) })?;
    for (fd, tag) in [(pidfd.as_raw_fd(), 1), (wake.as_raw_fd(), 2)] {
        let mut event = libc::epoll_event {
            events: libc::EPOLLIN as u32,
            u64: tag,
        };
        if unsafe { libc::epoll_ctl(epoll.as_raw_fd(), libc::EPOLL_CTL_ADD, fd, &mut event) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let worker_wake = wake.clone();
    std::thread::spawn(move || {
        // Owned descriptors live until either native exit or stream cancellation.
        let _pidfd = pidfd;
        let _wake = worker_wake;
        let mut events = [libc::epoll_event { events: 0, u64: 0 }; 2];
        loop {
            let count = unsafe { libc::epoll_wait(epoll.as_raw_fd(), events.as_mut_ptr(), 2, -1) };
            if count < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if count > 0 && events[..count as usize].iter().any(|e| e.u64 == 1) {
                cancel.cancel();
            }
            break;
        }
    });
    Ok(wake)
}
pub fn wake(fd: &OwnedFd) {
    let value = 1u64;
    unsafe {
        libc::write(fd.as_raw_fd(), (&value as *const u64).cast(), 8);
    }
}
