//! What a pane is doing, read from `/proc`.

use std::os::fd::AsFd;
use std::path::PathBuf;

use shed_model::Tool;

/// A process's working directory. A deleted one is still named by its path
/// (the kernel adds " (deleted)"), so a folder made again there is used.
pub fn cwd(pid: u32) -> Option<PathBuf> {
    let path = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    match path.to_str().and_then(|p| p.strip_suffix(" (deleted)")) {
        Some(p) => Some(p.into()),
        None => Some(path),
    }
}

/// A process's command line.
pub fn cmdline(pid: u32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let argv: Vec<String> = raw
        .split(|&b| b == 0)
        .filter(|a| !a.is_empty())
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
}
