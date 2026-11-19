//! `cargo run -p shed-codegen`: regenerates the nexus API's TypeScript (web
//! app) and Kotlin (Android app) types from `crates/shed-api`.

fn main() -> anyhow::Result<()> {
    let root = shed_codegen::workspace_root();
    let changed = shed_codegen::write_all(&root)?;
    if changed.is_empty() {
        println!("API types are up to date");
    }
    for file in changed {
        println!("wrote {file}");
    }
    Ok(())
}
