//! What a pane is doing: from `/proc` on Linux, from the kernel's process
//! info on macOS.

use std::os::fd::AsFd;
use std::path::PathBuf;

use shed_model::Tool;

/// A process's working directory. A deleted one is still named by its path
/// (the kernel adds " (deleted)"), so a folder made again there is used.
#[cfg(target_os = "linux")]
pub fn cwd(pid: u32) -> Option<PathBuf> {
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    match path.to_str().and_then(|p| p.strip_suffix(" (deleted)")) {
        Some(p) => Some(p.into()),
        None => Some(path),
    }
}

/// A process's working directory.
#[cfg(target_os = "macos")]
pub fn cwd(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: an all-zero `proc_vnodepathinfo` is valid; the kernel fills it.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: `info` is writable for `size` bytes.
    let n = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size) };
    if n != size {
        return None;
    }
    // SAFETY: `vip_path` is MAXPATHLEN bytes, NUL-terminated by the kernel.
    let bytes: &[u8] = unsafe { std::slice::from_raw_parts(info.pvi_cdir.vip_path.as_ptr().cast(), 32 * 32) };
    let end = bytes.iter().position(|&b| b == 0)?;
    (end > 0).then(|| PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..end])))
}

/// A process's command line.
#[cfg(target_os = "linux")]
pub fn cmdline(pid: u32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv: Vec<String> = raw
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!argv.is_empty()).then_some(argv)
}

/// A process's command line, from `KERN_PROCARGS2`: argc, the executable's
/// path, padding, then argv, each NUL-terminated.
#[cfg(target_os = "macos")]
pub fn cmdline(pid: u32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size: libc::size_t = 0;
    // SAFETY: a null buffer asks only for the size.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    let mut buf = vec![0u8; size];
    // SAFETY: `buf` is writable for `size` bytes.
    if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) } != 0 {
        return None;
    }
    buf.truncate(size);
    parse_procargs(&buf)
}

#[cfg(any(target_os = "macos", test))]
fn parse_procargs(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?) as usize;
    let rest = &buf[4..];
    // Skip the executable's path and the NULs after it.
    let path_end = rest.iter().position(|&b| b == 0)?;
    let start = path_end + rest[path_end..].iter().position(|&b| b != 0)?;
    let argv: Vec<String> = rest[start..]
        .split(|&b| b == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (!argv.is_empty()).then_some(argv)
}

/// The tool in the foreground of a pane: the leader of the PTY's foreground
/// process group, unless that is the shell itself.
pub fn foreground_tool(master: &impl AsFd, shell_pid: u32) -> Option<Tool> {
    let pgrp = crate::pty::foreground_pgrp(master)?.as_raw() as u32;
    if pgrp == shell_pid {
        return None;
    }
    cmdline(pgrp).map(|argv| Tool { argv })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_own_process() {
        let me = std::process::id();
        assert_eq!(cwd(me), std::env::current_dir().ok());
        assert!(cmdline(me).is_some_and(|a| !a.is_empty()));
    }

    #[test]
    fn macos_process_arguments() {
        let mut buf = 2i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/bin/tail\0\0\0tail\0-f\0HOME=/x\0");
        assert_eq!(parse_procargs(&buf), Some(vec!["tail".into(), "-f".into()]));
    }
}
