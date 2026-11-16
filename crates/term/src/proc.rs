//! The process in a terminal's foreground: the PTY's foreground process
//! group (`tcgetpgrp` on the master side), and its name.

use std::os::fd::RawFd;

/// The process group in the foreground of the PTY whose master is `master`.
pub fn foreground_pgid(master: RawFd) -> Option<u32> {
    // SAFETY: a plain syscall on a descriptor the caller keeps open.
    let pgid = unsafe { libc::tcgetpgrp(master) };
    u32::try_from(pgid).ok().filter(|&p| p > 0)
}

/// The name of a process: its executable's file name (`zsh`, `nvim`, `node`).
#[cfg(target_os = "macos")]
pub fn process_name(pid: u32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    // SAFETY: `buf` is as long as the size passed.
    let len = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if len > 0 {
        buf.truncate(len as usize);
        let path = String::from_utf8_lossy(&buf).into_owned();
        return path.rsplit('/').next().map(str::to_owned).filter(|n| !n.is_empty());
    }
    // (A process of another user: the kernel's short name.)
    let mut buf = vec![0u8; 64];
    // SAFETY: as above.
    let len = unsafe { libc::proc_name(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    (len > 0).then(|| String::from_utf8_lossy(&buf[..len as usize]).into_owned())
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn process_name(pid: u32) -> Option<String> {
    let name = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(name.trim_end().to_owned()).filter(|n| !n.is_empty())
}

#[cfg(not(unix))]
pub fn process_name(_pid: u32) -> Option<String> {
    None
}

/// The working directory of a process (a shell's, after its `cd`s).
#[cfg(target_os = "macos")]
pub fn process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    // SAFETY: `info` is plain data the call fills in, and is as large as the size passed.
    let info = unsafe {
        let mut info: libc::proc_vnodepathinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as i32;
        let got = libc::proc_pidinfo(pid as i32, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size);
        if got != size {
            return None;
        }
        info
    };
    // The path is a flat array of bytes (libc spells it as 32 arrays of 32).
    let bytes: Vec<u8> = info.pvi_cdir.vip_path.iter().flatten().map(|&c| c as u8).take_while(|&b| b != 0).collect();
    (!bytes.is_empty()).then(|| std::path::PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn process_cwd(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

#[cfg(not(unix))]
pub fn process_cwd(_pid: u32) -> Option<std::path::PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_a_running_process() {
        let mut child = std::process::Command::new("sleep").arg("5").spawn().unwrap();
        let name = process_name(child.id());
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(name.as_deref(), Some("sleep"));
    }

    #[test]
    fn finds_the_working_directory_of_a_process() {
        let here = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert_eq!(process_cwd(std::process::id()).and_then(|p| p.canonicalize().ok()), Some(here));
    }

    #[test]
    fn this_process_has_a_name_and_nothing_is_named_after_it_exits() {
        assert!(process_name(std::process::id()).is_some());
        assert_eq!(process_name(u32::MAX - 7), None);
    }
}
