//! Prints, for each of a crate's dependencies, how it is declared and what
//! removing it would do. With a dependency name, also asks Cargo what
//! updating that one dependency would change.
//!
//! Nothing is changed: no `cargo add` or `cargo remove` is run, and the
//! update is a dry run (which may refresh the registry index).
//!
//!     cargo run -p cargo_backend --example changes -- <directory> <crate-name> [dependency]

use cargo_backend::{
    AddEffect, Declaration, UpdateSpec, add_effect, command_line, declaration,
    direct_dependencies, has_lockfile, load_workspace, read_crate_manifest, read_lockfile,
    read_member_manifests, read_root_manifest, remove_args, remove_effect, update_args,
    update_dry_run,
};

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let directory = args.next().unwrap_or_else(|| ".".to_string());
    let crate_name = args.next().ok_or("usage: changes <directory> <crate-name> [dependency]")?;
    let to_update = args.next();

    let workspace = load_workspace(&directory)?;
    let krate = workspace
        .find(&crate_name)
        .ok_or_else(|| format!("no crate named {crate_name:?} in this workspace"))?;
    println!("crate: {} {}", krate.name, krate.version);

    let root = read_root_manifest(&workspace)?;
    let manifest = read_crate_manifest(krate)?;
    let members = read_member_manifests(&workspace)?;
    println!("{} member manifests read", members.len());

    let lockfile = if has_lockfile(&workspace.root) {
        Some(read_lockfile(&workspace.root)?)
    } else {
        None
    };
    let list = direct_dependencies(krate, lockfile.as_ref());

    for row in &list.listed {
        let target = row.target.as_deref();
        let declared = match declaration(&root, &manifest, &row.name, row.kind, target) {
            Some(Declaration::Inherited) => "workspace",
            Some(Declaration::Literal) => "literal",
            None => "not found",
        };
        println!("  {:<28} {:<10} {}", row.name, declared, row.kind.label());
        match remove_args(&row.name, &krate.name, row.kind, target) {
            Ok(args) => {
                let effect = remove_effect(&root, &members, &krate.name, &row.name, row.kind, target);
                println!(
                    "      {}{}",
                    command_line(&args),
                    if effect.removes_workspace_entry {
                        "   (also removes the entry from the root Cargo.toml)"
                    } else {
                        ""
                    }
                );
            }
            Err(error) => println!("      {error}"),
        }
    }

    for name in ["serde", "a-crate-nobody-has"] {
        let effect = match add_effect(&root, name) {
            AddEffect::Inherits => "written as workspace = true",
            AddEffect::LiteralOutsideWorkspaceTable => {
                "a literal version in this crate, outside [workspace.dependencies]: warn first"
            }
            AddEffect::Literal => "a literal version in this crate",
        };
        println!("adding {name}: {effect}");
    }

    if let Some(name) = to_update {
        let row = list
            .listed
            .iter()
            .find(|row| row.name == name)
            .ok_or_else(|| format!("{name} is not a listed dependency of {crate_name}"))?;
        let specs = [UpdateSpec::new(&row.name, row.locked_version.as_deref())];
        if let Some(args) = update_args(&specs, false)? {
            println!("{} would change:", command_line(&args));
        }
        let changes = update_dry_run(&workspace.root, &specs)?;
        if changes.is_empty() {
            println!("  nothing");
        }
        for change in changes {
            println!(
                "  {:?} {} {} -> {}",
                change.kind,
                change.name,
                change.from.as_deref().unwrap_or("-"),
                change.to.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(())
}
