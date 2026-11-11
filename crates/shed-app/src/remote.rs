//! `shed remote connect HOST ID`: what a remote tab's pane runs. It makes
//! sure the server has this Shed's `shedd` (copying it to `~/.local/bin`
//! over ssh when missing or different), then runs `ssh -t HOST shedd attach
//! ID`, which joins the shell the server keeps. A dropped connection is
//! tried again until the shell ends or Ctrl+C.
//!
//! ssh runs in the pane, so passwords and 2FA are asked right there; one
//! connection is shared by the steps (`ControlMaster`), so they are asked once.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Where `shedd` goes on the server, and the file saying which build it is.
const REMOTE_SHEDD: &str = ".local/bin/shedd";
const REMOTE_ID: &str = ".local/bin/.shedd-id";

/// The program standing in for ssh (tests use a stand-in).
fn ssh_program() -> String {
    std::env::var("SHED_SSH").unwrap_or_else(|_| "ssh".into())
}

fn ssh(host: &str) -> Command {
    let mut c = Command::new(ssh_program());
    let control = std::env::temp_dir().join("shed-ssh-%C");
    c.args(["-o", "ControlMaster=auto", "-o", &format!("ControlPath={}", control.display()), "-o", "ControlPersist=120"]);
    c.args(["-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3"]);
    c.arg(host);
    c
}

/// The `shedd` to put on a Linux server of architecture `arch`: a static
/// build for it if there is one beside `shed` (it runs on any Linux), else,
/// on a Linux machine of the same architecture, the one Shed itself uses.
fn local_shedd(arch: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SHED_REMOTE_SHEDD") {
        return Some(p.into());
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let static_build = dir.join(format!("shedd-{arch}-static"));
    if static_build.is_file() {
        return Some(static_build);
    }
    let same = cfg!(target_os = "linux") && arch == std::env::consts::ARCH;
    Some(dir.join("shedd")).filter(|p| same && p.is_file())
}

/// Which build a `shedd` is: its version and size, enough to tell builds apart.
fn build_id(path: &std::path::Path) -> String {
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    format!("{} {size}", env!("CARGO_PKG_VERSION"))
}

fn say(text: &str) {
    // Dimmed, so it reads as Shed's, not the server's.
    print!("\x1b[2m{text}\x1b[0m\r\n");
    let _ = std::io::stdout().flush();
}

/// Puts this Shed's `shedd` on the server unless it is there already.
fn ensure_shedd(host: &str) -> anyhow::Result<()> {
    let out = ssh(host).arg(format!("uname -s; uname -m; cat {REMOTE_ID} 2>/dev/null; true")).stdin(Stdio::inherit()).stderr(Stdio::inherit()).output()?;
    if !out.status.success() {
        anyhow::bail!("ssh {host} failed");
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines().map(str::trim);
    let (os, arch, theirs) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""), lines.next().unwrap_or(""));
    if os != "Linux" {
        anyhow::bail!("{host} runs {os}; remote tabs need a Linux server");
    }
    let Some(local) = local_shedd(arch) else {
        anyhow::bail!("no shedd for Linux {arch} here: put a static build beside shed as shedd-{arch}-static");
    };
    let id = build_id(&local);
    if theirs == id {
        return Ok(());
    }
    say(&format!("Putting shedd on {host} (~/{REMOTE_SHEDD})…"));
    let script = format!(
        "mkdir -p .local/bin && cat > {REMOTE_SHEDD}.new && chmod 755 {REMOTE_SHEDD}.new && mv -f {REMOTE_SHEDD}.new {REMOTE_SHEDD} && echo '{id}' > {REMOTE_ID}"
    );
    let mut child = ssh(host).arg(script).stdin(Stdio::piped()).spawn()?;
    std::io::copy(&mut std::fs::File::open(&local)?, child.stdin.as_mut().unwrap())?;
    drop(child.stdin.take());
    if !child.wait()?.success() {
        anyhow::bail!("could not copy shedd to {host}");
    }
    Ok(())
}

pub fn connect(host: &str, id: &str) -> anyhow::Result<()> {
    let mut installed = false;
    loop {
        say(&format!("Connecting to {host}…"));
        if !installed {
            match ensure_shedd(host) {
                Ok(()) => installed = true,
                Err(e) => {
                    say(&format!("{e}; trying again in 5 s (Ctrl+C stops)"));
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }
        }
        let status = ssh(host).arg("-t").arg(format!("exec {REMOTE_SHEDD} attach {id}")).status()?;
        match status.code() {
            // The shell ended, or the tab was closed.
            Some(0) => return Ok(()),
            // This end let go (the pane is closing).
            Some(2) => return Ok(()),
            _ => {
                say(&format!("The connection to {host} ended; joining the shell again in 3 s (Ctrl+C stops)"));
                std::thread::sleep(Duration::from_secs(3));
            }
        }
    }
}

/// Hosts named in `~/.ssh/config`, to offer for a new remote tab.
pub fn known_hosts() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME") else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(PathBuf::from(home).join(".ssh/config")) else { return Vec::new() };
    let mut hosts: Vec<String> = text
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let (key, rest) = l.split_once(char::is_whitespace)?;
            key.eq_ignore_ascii_case("host").then_some(rest)
        })
        .flat_map(|rest| rest.split_whitespace())
        .filter(|h| !h.contains(['*', '?', '!']))
        .map(String::from)
        .collect();
    hosts.dedup();
    hosts
}
