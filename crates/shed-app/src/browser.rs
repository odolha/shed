//! Opening a project's links in the browser, bringing forward a tab that
//! already shows one instead of opening it again.
//!
//! No browser lets another program find and focus its tabs, so Shed has a
//! small extension (`browser/chrome/`). Chrome starts Shed's native
//! messaging host (`shed browser-host`) for it; the host listens on a socket
//! in Shed's runtime folder. To open a link, Shed writes a line to that
//! socket; the host hands it to the extension, which focuses a matching tab
//! or opens one, and the answer comes back the same way. Without the
//! extension, links open the plain way.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The extension's ID, fixed by the public key in its manifest.
pub const EXTENSION_ID: &str = "mdlnmokkpngbjmhgikghpijjgebblfai";
/// The native messaging host's name, as the extension asks for it.
pub const HOST_NAME: &str = "dev.shed.browser";

const EXTENSION: &[(&str, &[u8])] = &[
    ("manifest.json", include_bytes!("../../../browser/chrome/manifest.json")),
    ("background.js", include_bytes!("../../../browser/chrome/background.js")),
    ("icon.png", include_bytes!("../../../browser/chrome/icon.png")),
];

/// Where browsers built on Chromium look for native messaging hosts, under
/// the config folder (`~/.config`, or on macOS `~/Library/Application Support`).
const BROWSERS: &[&str] = &["google-chrome", "google-chrome-beta", "chromium", "BraveSoftware/Brave-Browser", "microsoft-edge", "vivaldi"];

fn socket_path() -> PathBuf {
    shed_model::paths::runtime_dir().join("browser.sock")
}

/// The extension's folder, to load it from the browser's extensions page.
pub fn extension_dir() -> PathBuf {
    shed_model::paths::data_dir().join("browser-extension")
}

/// Writes the extension and Shed's native messaging host, and registers
/// the host with each browser found (or with the given profile folders).
/// Returns the browsers registered with.
pub fn install(profiles: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let ext = extension_dir();
    std::fs::create_dir_all(&ext)?;
    for (name, bytes) in EXTENSION {
        std::fs::write(ext.join(name), bytes)?;
    }
    // The browser starts the host with its own environment: a Shed run with
    // its own runtime folder (a test, a dev build) keeps it.
    let exe = std::env::current_exe()?;
    let mut script = String::from("#!/bin/sh\n");
    if let Some(dir) = std::env::var_os("SHED_RUNTIME_DIR") {
        script.push_str(&format!("export SHED_RUNTIME_DIR='{}'\n", Path::new(&dir).display()));
    }
    script.push_str(&format!("exec '{}' browser-host \"$@\"\n", exe.display()));
    let host = shed_model::paths::data_dir().join("browser-host");
    std::fs::write(&host, script)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&host, std::fs::Permissions::from_mode(0o755))?;
    let manifest = format!(
        "{{\n  \"name\": \"{HOST_NAME}\",\n  \"description\": \"Shed: focus a browser tab for a project's link\",\n  \"path\": \"{}\",\n  \"type\": \"stdio\",\n  \"allowed_origins\": [\"chrome-extension://{EXTENSION_ID}/\"]\n}}\n",
        host.display()
    );
    let targets: Vec<PathBuf> = if profiles.is_empty() {
        let config = config_home();
        BROWSERS.iter().map(|b| config.join(b)).filter(|d| d.is_dir()).collect()
    } else {
        profiles.to_vec()
    };
    for dir in &targets {
        let hosts = dir.join("NativeMessagingHosts");
        std::fs::create_dir_all(&hosts)?;
        std::fs::write(hosts.join(format!("{HOST_NAME}.json")), &manifest)?;
    }
    Ok(targets)
}

/// Claude in Chrome's ID in the Chrome Web Store (the same in Brave).
const CLAUDE_EXTENSION_ID: &str = "fcoeoabgfenejglbffodgkkbkcdhcgfn";

/// The browsers with Claude in Chrome installed in some profile, by name
/// ("Chrome", "Brave"), for an AI tab connected to the browser to say where
/// Claude will work, or how to set it up when none has it.
pub fn claude_browsers() -> Vec<&'static str> {
    let config = config_home();
    let mut found = Vec::new();
    for (dir, name) in [("google-chrome", "Chrome"), ("google-chrome-beta", "Chrome"), ("chromium", "Chromium"), ("BraveSoftware/Brave-Browser", "Brave"), ("microsoft-edge", "Edge")] {
        let dir = if cfg!(target_os = "macos") {
            config.join(match dir {
                "google-chrome" => "Google/Chrome",
                "google-chrome-beta" => "Google/Chrome Beta",
                "chromium" => "Chromium",
                "microsoft-edge" => "Microsoft Edge",
                other => other,
            })
        } else {
            config.join(dir)
        };
        let Ok(profiles) = std::fs::read_dir(&dir) else { continue };
        let has = profiles.flatten().any(|p| p.path().join("Extensions").join(CLAUDE_EXTENSION_ID).is_dir());
        if has && !found.contains(&name) {
            found.push(name);
        }
    }
    found
}

fn config_home() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support");
    }
    // The browsers' own config folder, not a test's XDG_CONFIG_HOME.
    home.join(".config")
}

/// Asks the extension to show `url`: a tab already on it comes forward
/// (unless `new_tab`). `Ok(true)` if a tab was reused; an error if the
/// extension is not there to ask.
pub fn open(url: &str, new_tab: bool) -> io::Result<bool> {
    let mut stream = UnixStream::connect(socket_path())?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let request = serde_json::json!({"open": url, "new": new_tab});
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut reply = String::new();
    BufReader::new(stream).read_line(&mut reply)?;
    let reply: serde_json::Value = serde_json::from_str(&reply).map_err(io::Error::other)?;
    if reply.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Err(io::Error::other(reply.get("error").and_then(|e| e.as_str()).unwrap_or("the browser did not answer").to_string()));
    }
    Ok(reply.get("reused").and_then(|v| v.as_bool()) == Some(true))
}

/// One native message: its length (native byte order), then JSON.
fn read_message(input: &mut impl Read) -> io::Result<serde_json::Value> {
    let mut len = [0u8; 4];
    input.read_exact(&mut len)?;
    let len = u32::from_ne_bytes(len) as usize;
    if len > 1 << 20 {
        return Err(io::Error::other("message too large"));
    }
    let mut buf = vec![0u8; len];
    input.read_exact(&mut buf)?;
    serde_json::from_slice(&buf).map_err(io::Error::other)
}

fn write_message(output: &mut impl Write, msg: &serde_json::Value) -> io::Result<()> {
    let bytes = msg.to_string().into_bytes();
    output.write_all(&(bytes.len() as u32).to_ne_bytes())?;
    output.write_all(&bytes)?;
    output.flush()
}

/// `shed browser-host`: what the browser starts for the extension. It
/// passes Shed's requests to the extension and the answers back, until the
/// browser closes it.
pub fn host() -> io::Result<()> {
    run_host(io::stdin(), io::stdout(), &socket_path())
}

fn run_host(input: impl Read + Send + 'static, output: impl Write + Send + 'static, socket: &Path) -> io::Result<()> {
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)?;
    let waiting: Arc<Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>> = Arc::default();
    let output = Arc::new(Mutex::new(output));

    // Answers from the extension go to whoever asked.
    let answers = waiting.clone();
    let socket_owned = socket.to_path_buf();
    std::thread::spawn(move || {
        let mut input = input;
        while let Ok(msg) = read_message(&mut input) {
            let id = msg.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            if let Some(tx) = answers.lock().unwrap().remove(&id) {
                let _ = tx.send(msg);
            }
        }
        // The browser closed the connection: so does the host.
        let _ = std::fs::remove_file(&socket_owned);
        std::process::exit(0);
    });

    let mut next = 0u64;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        next += 1;
        let id = next;
        let (waiting, output) = (waiting.clone(), output.clone());
        std::thread::spawn(move || {
            let mut line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut line).is_err() {
                return;
            }
            let Ok(mut request) = serde_json::from_str::<serde_json::Value>(&line) else { return };
            request["id"] = id.into();
            let (tx, rx) = mpsc::channel();
            waiting.lock().unwrap().insert(id, tx);
            if write_message(&mut *output.lock().unwrap(), &request).is_err() {
                return;
            }
            let reply = rx.recv_timeout(Duration::from_secs(3)).unwrap_or_else(|_| serde_json::json!({"ok": false, "error": "the browser did not answer"}));
            waiting.lock().unwrap().remove(&id);
            let mut stream = &stream;
            let _ = stream.write_all(format!("{reply}\n").as_bytes());
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipe of native messages, the way the browser talks to the host.
    struct Pipe(std::os::unix::net::UnixStream);

    impl Read for Pipe {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn requests_reach_the_extension_and_answers_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("browser.sock");
        // The "browser": what the host writes, it reads; what it writes, the host reads.
        let (to_host, host_in) = std::os::unix::net::UnixStream::pair().unwrap();
        let (host_out, from_host) = std::os::unix::net::UnixStream::pair().unwrap();
        let s = socket.clone();
        std::thread::spawn(move || run_host(Pipe(host_in), Pipe(host_out), &s));
        // A stand-in extension: says a tab was reused for any request.
        std::thread::spawn(move || {
            let (mut read, mut write) = (Pipe(from_host), Pipe(to_host));
            while let Ok(msg) = read_message(&mut read) {
                assert_eq!(msg["open"], "http://localhost:3000/");
                let reply = serde_json::json!({"id": msg["id"], "ok": true, "reused": msg["new"] == false});
                write_message(&mut write, &reply).unwrap();
            }
        });
        for _ in 0..50 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let ask = |new: bool| {
            let mut stream = UnixStream::connect(&socket).unwrap();
            stream.write_all(format!("{}\n", serde_json::json!({"open": "http://localhost:3000/", "new": new})).as_bytes()).unwrap();
            let mut reply = String::new();
            BufReader::new(stream).read_line(&mut reply).unwrap();
            serde_json::from_str::<serde_json::Value>(&reply).unwrap()
        };
        assert_eq!(ask(false)["reused"], true);
        assert_eq!(ask(true)["reused"], false);
    }
}
