//! Unsaved edits kept aside, in `$XDG_STATE_HOME/shed/recovery/`, so they
//! come back after a crash or a closed bench. Each file there is named by a
//! hash of the edited file's path and starts with that path.

use std::path::{Path, PathBuf};

/// Where a file's unsaved edits are kept aside.
fn recovery_file(path: &Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    shed_model::paths::state_dir().join("recovery").join(format!("{:016x}", h.finish()))
}

/// Keeps `text` aside as `path`'s unsaved edits.
pub fn write(path: &Path, text: &str) {
    let file = recovery_file(path);
    let _ = std::fs::create_dir_all(file.parent().unwrap());
    let _ = std::fs::write(file, format!("{}\n{text}", path.display()));
}

/// `path`'s unsaved edits, if some were kept.
pub fn read(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(recovery_file(path)).ok()?;
    let (first, rest) = text.split_once('\n')?;
    (Path::new(first) == path).then(|| rest.to_string())
}

/// Forgets `path`'s unsaved edits.
pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(recovery_file(path));
}
