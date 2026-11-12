//! `shedd attach ID`: the server end of a remote tab.
//!
//! A remote tab's local pane runs `ssh -t host shedd attach ID`. On the
//! server this finds (or starts) the keeper there, finds the bench kept for
//! `ID` (running, saved after a restart, or new), and joins its one pane to
//! the terminal ssh gave it: what the pane shows comes out, what is typed
//! goes in. Only terminal bytes cross ssh, so the two ends never have to be
//! the same version of Shed. Closing the local bench just ends ssh, and the
//! shell stays; the next attach shows it again, scrollback and all.
//!
//! Several PCs may attach to the same shell at once (each sees everything);
//! the one typing sets its size. `shedd list` lists the shells kept here, so
//! another PC can join one; `shedd logbook` sends a copy of the commands run
//! here, for the remote tab's Ctrl+R.

use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use shed_model::{BenchId, PaneId, Shed};
use shed_proto::{ClientKind, ClientMsg, Frame, FrameDecoder, KeeperMsg, PROTOCOL_VERSION, ShowTarget};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Typed into the pane by the local keeper when the remote tab is closed:
/// the remote shell is closed too.
pub const CLOSE_REQUEST: &[u8] = b"\x1b]7777;shed;remote-close\x07";

/// Exit codes: the shell ended (or was closed); this end let go; the shell
/// was lost with its keeper (the server stopping), to be joined again.
pub const EXIT_ENDED: i32 = 0;
pub const EXIT_DETACHED: i32 = 2;
pub const EXIT_LOST: i32 = 3;

struct Conn {
    stream: UnixStream,
    decoder: FrameDecoder,
    buf: Vec<u8>,
}

impl Conn {
    async fn send(&mut self, frame: Frame) -> std::io::Result<()> {
        self.stream.write_all(&frame.encode()).await
    }

    async fn next(&mut self) -> anyhow::Result<Frame> {
        loop {
            if let Some(frame) = self.decoder.next_frame()? {
                return Ok(frame);
            }
            let n = self.stream.read(&mut self.buf).await?;
            if n == 0 {
                anyhow::bail!("the keeper went away");
            }
            self.decoder.push(&self.buf[..n]);
        }
    }

    async fn tree(&mut self) -> anyhow::Result<(Shed, Vec<String>)> {
        self.send(Frame::Client(ClientMsg::Status)).await?;
        loop {
            if let Frame::Keeper(KeeperMsg::Status(s)) = self.next().await? {
                return Ok((s.tree, s.stored));
            }
        }
    }
}

/// Connects to the keeper, starting one (detached, so it outlives this
/// connection) if none runs and `start` asks for it.
async fn connect(start: bool) -> anyhow::Result<Option<UnixStream>> {
    let socket = shed_model::paths::socket_path();
    if let Ok(s) = UnixStream::connect(&socket).await {
        return Ok(Some(s));
    }
    if !start {
        return Ok(None);
    }
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // ssh brings the PC's locale (`SendEnv LANG LC_*`); the keeper's shells
    // keep only what this server has.
    let vars: Vec<(String, String)> = std::env::vars().collect();
    let default_lang = std::fs::read_to_string("/etc/default/locale").ok().and_then(|t| {
        t.lines().find_map(|l| l.trim().strip_prefix("LANG=").map(|v| v.trim_matches('"').to_string()))
    });
    let (remove, lang) = server_locale(&vars, locale_exists, default_lang);
    for key in remove {
        cmd.env_remove(key);
    }
    if let Some(lang) = lang {
        cmd.env("LANG", lang);
    }
    use std::os::unix::process::CommandExt;
    // SAFETY: setsid only detaches the child from this session.
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    cmd.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(s) = UnixStream::connect(&socket).await {
            return Ok(Some(s));
        }
        if Instant::now() > deadline {
            anyhow::bail!("the keeper did not start");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Whether this machine has the locale `name`.
fn locale_exists(name: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(name) else { return false };
    // SAFETY: newlocale reads the name and returns a new locale or null; the
    // locale is freed right away.
    unsafe {
        let loc = libc::newlocale(libc::LC_ALL_MASK, c.as_ptr(), std::ptr::null_mut());
        if loc.is_null() {
            return false;
        }
        libc::freelocale(loc);
    }
    true
}

/// Of the locale variables in `vars`, those this server lacks (to remove),
/// and the `LANG` to set when its own is missing or lacking: the server's
/// default if it has it, else `C.UTF-8`.
fn server_locale(vars: &[(String, String)], exists: impl Fn(&str) -> bool, default_lang: Option<String>) -> (Vec<String>, Option<String>) {
    let locale_var = |k: &str| k == "LANG" || k == "LC_ALL" || k.starts_with("LC_");
    let remove: Vec<String> = vars.iter().filter(|(k, v)| locale_var(k) && !v.is_empty() && !exists(v)).map(|(k, _)| k.clone()).collect();
    let has_lang = vars.iter().any(|(k, _)| k == "LANG") && !remove.iter().any(|k| k == "LANG");
    let lang = (!has_lang).then(|| default_lang.filter(|l| exists(l)).unwrap_or_else(|| "C.UTF-8".into()));
    (remove, lang)
}

fn terminal_size() -> (u16, u16) {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: TIOCGWINSZ fills the struct for a terminal; on error it is left zeroed.
    let ok = unsafe { libc::ioctl(std::io::stdin().as_raw_fd(), libc::TIOCGWINSZ, &mut ws) } == 0;
    if ok && ws.ws_col > 0 && ws.ws_row > 0 { (ws.ws_col, ws.ws_row) } else { (80, 24) }
}

/// Puts the terminal in raw mode (keys pass through as bytes, Ctrl+C
/// included) until dropped.
struct Raw(Option<nix::sys::termios::Termios>);

impl Raw {
    fn new() -> Self {
        use nix::sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr};
        let stdin = std::io::stdin();
        let Ok(saved) = tcgetattr(&stdin) else { return Raw(None) };
        let mut raw = saved.clone();
        cfmakeraw(&mut raw);
        let _ = tcsetattr(&stdin, SetArg::TCSANOW, &raw);
        Raw(Some(saved))
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        if let Some(saved) = &self.0 {
            let _ = nix::sys::termios::tcsetattr(std::io::stdin(), nix::sys::termios::SetArg::TCSANOW, saved);
        }
    }
}

/// Says hello to the keeper (started if `start` and none runs). A newer
/// shedd (Shed was updated) meeting an older keeper still holding the shells
/// lets the keeper's own program do the work, run with `args`: its file may
/// have been replaced, but /proc still has the one it runs. Keepers older
/// than protocol `oldest` cannot do it.
async fn open(start: bool, oldest: u32, args: &[&std::ffi::OsStr]) -> anyhow::Result<Option<Conn>> {
    let Some(stream) = connect(start).await? else { return Ok(None) };
    let mut conn = Conn { stream, decoder: FrameDecoder::default(), buf: vec![0u8; 64 * 1024] };
    conn.send(Frame::Client(ClientMsg::Hello { version: PROTOCOL_VERSION, kind: ClientKind::Cli })).await?;
    loop {
        if let Frame::Keeper(KeeperMsg::Welcome { version, keeper_pid }) = conn.next().await? {
            if version != PROTOCOL_VERSION {
                if version < oldest {
                    anyhow::bail!("the Shed keeping the shells here is older; it can list them once it restarts");
                }
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new(format!("/proc/{keeper_pid}/exe")).args(args).exec();
                anyhow::bail!("cannot run the keeper's shedd: {err}");
            }
            return Ok(Some(conn));
        }
    }
}

/// `~/x` for paths in the home folder.
fn tilde(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if path == home => "~".into(),
        Some(home) => match path.strip_prefix(&home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}

/// `shedd list`: the remote shells kept here, a line each, tab-separated:
/// id, `running` or `saved`, how many attach to it now, its bench and tab
/// ids (for the logbook), its folder, and the program in front (or empty).
/// Stops nothing and starts nothing.
pub async fn list() -> anyhow::Result<String> {
    let store = shed_save::Store::default();
    let mut lines = Vec::new();
    let mut seen = Vec::new();
    if let Some(mut conn) = open(false, 24, &["list".as_ref()]).await? {
        conn.send(Frame::Client(ClientMsg::Status)).await?;
        let status = loop {
            if let Frame::Keeper(KeeperMsg::Status(s)) = conn.next().await? {
                break s;
            }
        };
        for b in &status.tree.benches {
            let (Some(id), Some(tab)) = (b.name.strip_prefix("remote-"), b.tabs.first()) else { continue };
            let Some(pane) = tab.panes.iter().find(|p| p.id == tab.active_pane).or(tab.panes.first()) else { continue };
            // Each PC joined to it now is one `shedd attach` connection.
            let attached = status.attached.iter().find(|(x, _)| *x == b.id).map_or(0, |(_, n)| *n);
            let tool = pane.tool.as_ref().map(|t| t.display()).unwrap_or_default();
            lines.push(format!("{id}\trunning\t{attached}\t{}\t{}\t{}\t{tool}", b.id, tab.id, tilde(&pane.cwd)));
            seen.push(b.name.clone());
        }
    }
    for info in store.list() {
        let Some(id) = info.name.strip_prefix("remote-") else { continue };
        if seen.contains(&info.name) {
            continue;
        }
        let Ok(bench) = store.load(&info.name) else { continue };
        let Some(tab) = bench.tabs.first() else { continue };
        let cwd = tab.panes.first().map(|p| tilde(std::path::Path::new(&p.cwd))).unwrap_or_default();
        lines.push(format!("{id}\tsaved\t0\t{}\t{}\t{cwd}\t", bench.id, tab.id));
    }
    Ok(lines.iter().map(|l| format!("{l}\n")).collect())
}

/// `shedd logbook`: a copy of this server's command history (an SQLite
/// file), written to stdout; nothing when there is none.
pub fn logbook() -> anyhow::Result<()> {
    use std::io::Write;
    let path = shed_logbook::default_path();
    if !path.exists() {
        return Ok(());
    }
    let book = shed_logbook::Logbook::open_readonly(&path)?;
    let copy = shed_model::paths::state_dir().join(format!("logbook-copy-{}.sqlite", std::process::id()));
    let _ = std::fs::remove_file(&copy);
    let bytes = book.copy_to(&copy).map_err(anyhow::Error::from).and_then(|()| Ok(std::fs::read(&copy)?));
    let _ = std::fs::remove_file(&copy);
    std::io::stdout().write_all(&bytes?)?;
    Ok(())
}

/// Runs the attach; returns the exit code.
pub async fn attach(id: &str, cwd: Option<PathBuf>) -> anyhow::Result<i32> {
    let mut args: Vec<&std::ffi::OsStr> = vec!["attach".as_ref(), id.as_ref()];
    if let Some(cwd) = &cwd {
        args.extend(["--cwd".as_ref(), cwd.as_os_str()]);
    }
    let Some(mut conn) = open(true, 0, &args).await? else { anyhow::bail!("no keeper") };

    // The bench kept for this tab: running, saved, or new.
    let name = format!("remote-{id}");
    let find = |tree: &Shed| tree.benches.iter().find(|b| b.name == name).map(|b| (b.id, b.tabs.first().map(|t| t.active_pane)));
    let (tree, stored) = conn.tree().await?;
    let mut found = find(&tree);
    if found.is_none() {
        let target = if stored.contains(&name) {
            ShowTarget::Load { name: name.clone() }
        } else {
            let cwd = cwd.filter(|d| d.is_dir());
            ShowTarget::NewBench { name: Some(name.clone()), cwd }
        };
        conn.send(Frame::Client(ClientMsg::Show(target))).await?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while found.is_none() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
            found = find(&conn.tree().await?.0);
        }
    }
    let Some((bench, Some(pane))) = found else { anyhow::bail!("cannot open the remote shell") };

    // Its size first, so the replay fits this terminal.
    let resize = |pane: PaneId| {
        let (cols, rows) = terminal_size();
        Frame::Client(ClientMsg::Resize { pane, cols, rows, cell_width: 0, cell_height: 0 })
    };
    conn.send(resize(pane)).await?;
    conn.send(Frame::Client(ClientMsg::Attach { bench: Some(bench) })).await?;
    let _raw = Raw::new();
    relay(conn, bench, &name, pane, resize).await
}

async fn relay(mut conn: Conn, bench: BenchId, name: &str, pane: PaneId, resize: impl Fn(PaneId) -> Frame) -> anyhow::Result<i32> {
    let mut stdout = tokio::io::stdout();
    let mut stdin = tokio::io::stdin();
    let mut winch = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::window_change())?;
    let mut input = vec![0u8; 16 * 1024];
    // Another PC attached to the same shell may have resized it: typing here
    // gives it this terminal's size again (at most twice a second).
    let mut sized = Instant::now();
    loop {
        tokio::select! {
            frame = conn.next() => match frame? {
                Frame::Keeper(KeeperMsg::Replay { pane: p, bytes, .. }) if p == pane => {
                    // A full reset first: the local pane has shown other things.
                    stdout.write_all(b"\x1bc").await?;
                    stdout.write_all(&bytes).await?;
                    stdout.flush().await?;
                }
                Frame::Output { pane: p, bytes } if p == pane => {
                    stdout.write_all(&bytes).await?;
                    stdout.flush().await?;
                }
                // Exited by itself (or closed from the tab): ended. Killed by
                // a signal otherwise: its keeper is stopping, as at a reboot.
                Frame::Keeper(KeeperMsg::PaneExited { pane: p, code }) if p == pane => {
                    return Ok(if code.is_some() { EXIT_ENDED } else { EXIT_LOST });
                }
                _ => {}
            },
            n = stdin.read(&mut input) => {
                let n = n?;
                if n == 0 {
                    let _ = conn.send(Frame::Client(ClientMsg::Detach { bench })).await;
                    return Ok(EXIT_DETACHED);
                }
                let bytes = &input[..n];
                if let Some(at) = bytes.windows(CLOSE_REQUEST.len()).position(|w| w == CLOSE_REQUEST) {
                    // The local tab closed: the remote shell goes too, and its
                    // saved copy (nothing will ask for it again).
                    if at > 0 {
                        conn.send(Frame::Input { pane, bytes: bytes[..at].to_vec() }).await?;
                    }
                    conn.send(Frame::Client(ClientMsg::DeleteBench { name: name.to_string() })).await?;
                    // Done: the bench goes before its shell exits, so no exit comes to wait for.
                    let _ = bench;
                    return Ok(EXIT_ENDED);
                }
                if sized.elapsed() > Duration::from_millis(500) {
                    conn.send(resize(pane)).await?;
                    sized = Instant::now();
                }
                conn.send(Frame::Input { pane, bytes: bytes.to_vec() }).await?;
            }
            _ = winch.recv() => {
                conn.send(resize(pane)).await?;
                sized = Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pcs_locale_is_kept_only_where_the_server_has_it() {
        let vars: Vec<(String, String)> = [("LANG", "en_GB.UTF-8"), ("LC_TIME", "ro_RO.UTF-8"), ("LC_PAPER", "en_GB.UTF-8"), ("HOME", "/x")]
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .into();
        let has = |l: &str| l != "ro_RO.UTF-8";
        assert_eq!(server_locale(&vars, has, None), (vec!["LC_TIME".to_string()], None));
        // A LANG the server lacks: its own default instead.
        let only_c = |l: &str| l.starts_with("C") || l == "en_US.UTF-8";
        let (remove, lang) = server_locale(&vars, only_c, Some("en_US.UTF-8".into()));
        assert_eq!(remove, ["LANG", "LC_TIME", "LC_PAPER"]);
        assert_eq!(lang.as_deref(), Some("en_US.UTF-8"));
        assert_eq!(server_locale(&[], only_c, None).1.as_deref(), Some("C.UTF-8"));
        assert!(locale_exists("C"));
        assert!(!locale_exists("xx_NOWHERE.UTF-8"));
    }
}
