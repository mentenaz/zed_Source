//! Prints what the manager would show for a crate — a way to try the
//! backend from a terminal, with no editor involved.
//!
//!     cargo run -p cargo_backend --example list -- <directory> [crate-name]

use cargo_backend::{direct_dependencies, has_lockfile, load_workspace, read_lockfile};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let directory = args.next().unwrap_or_else(|| ".".to_string());
    let crate_name = args.next();

    println!(
        "rustc {}  cargo {}",
        cargo_backend::query_rustc()?,
        cargo_backend::query_cargo()?
    );

    let workspace = load_workspace(&directory)?;
    println!("workspace: {} ({} crates)", workspace.root, workspace.crates.len());

    let krate = match &crate_name {
        Some(name) => workspace
            .find(name)
            .ok_or_else(|| format!("no crate named {name:?} in this workspace"))?,
        None => workspace
            .default_crate()
            .ok_or_else(|| "the workspace has no crates".to_string())?,
    };
    println!("crate: {} {}", krate.name, krate.version);

    let lockfile = if has_lockfile(&workspace.root) {
        Some(read_lockfile(&workspace.root)?)
    } else {
        println!("(no Cargo.lock: installed versions are unknown)");
        None
    };

    let list = direct_dependencies(krate, lockfile.as_ref());
    for row in &list.listed {
        let mut notes = Vec::new();
        if row.kind != cargo_backend::DependencyKind::Normal {
            notes.push(row.kind.label().to_string());
        }
        if row.optional {
            notes.push("optional".to_string());
        }
        if let Some(target) = &row.target {
            notes.push(target.clone());
        }
        if let Some(rename) = &row.rename {
            notes.push(format!("imported as {rename}"));
        }
        println!(
            "  {:<28} {:<12} {:<12} {}",
            row.name,
            row.requirement,
            row.locked_version.as_deref().unwrap_or("unknown"),
            notes.join(", ")
        );
    }
    println!("{} listed", list.listed.len());
    if list.hidden > 0 {
        println!("{} local and git dependencies not shown", list.hidden);
    }

    if let Some(lockfile) = &lockfile {
        let reachable = lockfile.reachable_crates_io_packages(&krate.name, &krate.version);
        println!("{} crates.io packages reachable (the set to check for advisories)", reachable.len());
    }
    Ok(())
}
