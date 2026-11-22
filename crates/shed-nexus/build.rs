//! Builds the web app (`web/`) for the nexus to carry in its binary.
//!
//! Release builds (`cargo install`, `cargo build --release`) build it; debug
//! builds (`cargo build`, `cargo test`, clippy) do not, unless
//! `SHED_NEXUS_WEB=1`. `SHED_NEXUS_NO_WEB=1` leaves it out of any build: the
//! nexus then serves only the API (or `web_dir`).
//!
//! The sources are copied into `OUT_DIR`, which keeps its own
//! `node_modules` (`npm ci` again when the lockfile changes), and built
//! there: nothing is written to the source tree, which may be cargo's git
//! checkout. Needs Node 24.15+ (or 22.22.3+, as `web/package.json` says).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::{env, fs};

/// What the web app's build reads, relative to `web/`.
const INPUTS: &[&str] =
    &["package.json", "package-lock.json", "angular.json", "tsconfig.json", "tsconfig.app.json", "src", "public"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=SHED_NEXUS_NO_WEB");
    println!("cargo:rerun-if-env-changed=SHED_NEXUS_WEB");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let list = out.join("web_files.rs");
    let release = env::var("PROFILE").is_ok_and(|p| p == "release");
    if flag("SHED_NEXUS_NO_WEB") || !(release || flag("SHED_NEXUS_WEB")) {
        write(&list, "&[]\n");
        return;
    }

    let web = Path::new(&env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR")).join("../../web");
    let web = fs::canonicalize(&web).unwrap_or(web);
    if !web.join("package.json").is_file() {
        fail(&format!("the web app's sources are not at {}", web.display()));
    }
    for input in INPUTS {
        println!("cargo:rerun-if-changed={}", web.join(input).display());
    }
    check_node();

    let stage = out.join("web");
    copy_sources(&web, &stage);
    let lock = fs::read(web.join("package-lock.json")).unwrap_or_default();
    let stamp = stage.join(".lockfile-hash");
    let lock_hash = format!("{:016x}", fnv(&lock));
    if !stage.join("node_modules").is_dir() || fs::read_to_string(&stamp).ok().as_deref() != Some(&lock_hash) {
        npm(&stage, &["ci", "--no-audit", "--no-fund", "--loglevel=error"]);
        write(&stamp, &lock_hash);
    }
    npm(&stage, &["run", "build"]);

    let dist = stage.join("dist/shed-web");
    if !dist.join("index.html").is_file() {
        fail(&format!("the web app's build made no {}", dist.join("index.html").display()));
    }
    let mut files = Vec::new();
    collect(&dist, &dist, &mut files);
    files.sort();
    let mut code = String::from("&[\n");
    for (rel, full) in files {
        let bytes = fs::read(&full).unwrap_or_else(|e| fail(&format!("reading {}: {e}", full.display())));
        code += &format!(
            "    crate::web::WebFile {{ path: {rel:?}, bytes: include_bytes!({:?}), etag: \"\\\"{:016x}\\\"\" }},\n",
            full.display().to_string(),
            fnv(&bytes)
        );
    }
    code += "]\n";
    write(&list, &code);
}

/// Set, and not empty or `0`.
fn flag(name: &str) -> bool {
    env::var(name).is_ok_and(|v| !v.is_empty() && v != "0")
}

fn fail(message: &str) -> ! {
    eprintln!("\nshed-nexus: {message}");
    eprintln!("\nTo build without the web app (the nexus then serves only the API): SHED_NEXUS_NO_WEB=1 cargo …\n");
    std::process::exit(1);
}

/// Node or npm missing or too old.
fn fail_node(message: &str) -> ! {
    fail(&format!(
        "{message}.\n\nRelease builds of shed-nexus carry the web app, built from web/ with Node 24.15 or \
         newer (or 22.22.3+) and npm on PATH. With nvm:\n\n    \
         curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash\n    \
         . ~/.nvm/nvm.sh && nvm install 24"
    ))
}

fn check_node() {
    let version = Command::new("node")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| fail_node("node is not on PATH"));
    let parts: Vec<u32> =
        version.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).chain([0, 0, 0]).take(3).collect();
    let (major, minor, patch) = (parts[0], parts[1], parts[2]);
    let ok = major >= 26 || (major == 24 && minor >= 15) || (major == 22 && (minor, patch) >= (22, 3));
    if !ok {
        fail_node(&format!("Node {version} is too old for the web app's build"));
    }
    let npm = Command::new("npm").arg("--version").output();
    if !npm.is_ok_and(|o| o.status.success()) {
        fail_node("npm is not on PATH");
    }
}

/// Runs npm in `dir`, its output going to the build's log (stderr), not to
/// cargo (stdout).
fn npm(dir: &Path, args: &[&str]) {
    let status = Command::new("npm")
        .args(args)
        .current_dir(dir)
        .env("NG_CLI_ANALYTICS", "false")
        .stdin(Stdio::null())
        .stdout(Stdio::from(std::io::stderr()))
        .status()
        .unwrap_or_else(|e| fail(&format!("running npm: {e}")));
    if !status.success() {
        fail(&format!("`npm {}` failed in {} (its output is above)", args.join(" "), dir.display()));
    }
}

/// Fresh copies of the inputs in `stage`, keeping its `node_modules` and
/// Angular's cache.
fn copy_sources(web: &Path, stage: &Path) {
    fs::create_dir_all(stage).unwrap_or_else(|e| fail(&format!("making {}: {e}", stage.display())));
    for input in INPUTS {
        let to = stage.join(input);
        if to.is_dir() {
            let _ = fs::remove_dir_all(&to);
        }
        copy(&web.join(input), &to);
    }
}

fn copy(from: &Path, to: &Path) {
    if from.is_dir() {
        fs::create_dir_all(to).unwrap_or_else(|e| fail(&format!("making {}: {e}", to.display())));
        let entries = fs::read_dir(from).unwrap_or_else(|e| fail(&format!("reading {}: {e}", from.display())));
        for entry in entries.flatten() {
            copy(&entry.path(), &to.join(entry.file_name()));
        }
    } else {
        fs::copy(from, to).unwrap_or_else(|e| fail(&format!("copying {}: {e}", from.display())));
    }
}

/// Every file under `dir`, as (path relative to `root` with `/`, full path).
fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| fail(&format!("reading {}: {e}", dir.display())));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            files.push((rel, path));
        }
    }
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap_or_else(|e| fail(&format!("writing {}: {e}", path.display())));
}

/// FNV-1a: a stable hash, for ETags and the lockfile's stamp.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}
