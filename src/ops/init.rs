//! Bootstrap a new lorri project

use crate::ops::{ok_msg, OpResult};
use std::fs::File;
use std::io;
use std::io::Write;
use std::path::Path;

fn create_if_missing(path: &Path, contents: &str, msg: &str) -> Result<(), io::Error> {
    if path.exists() {
        println!("- {} {}", msg, path.display());
        Ok(())
    } else {
        let mut f = File::create(path)?;
        f.write_all(contents.as_bytes())?;
        println!("- Writing {}", path.display());
        Ok(())
    }
}

/// See the documentation for lorri::cli::Command::Init for
/// more details
pub fn main(default_shell: &str, default_envrc: &str) -> OpResult {
    create_if_missing(
        Path::new("./shell.nix"),
        default_shell,
        "shell.nix exists, skipping. Make sure it is of a form that works with nix-shell.",
    )?;

    create_if_missing(
        Path::new("./.envrc"),
        default_envrc,
        ".envrc exists, skipping. Please add 'eval \"$(lorri direnv)\" to it to set up lorri support.",
    )?;

    ok_msg(String::from("\nSetup done."))
}
