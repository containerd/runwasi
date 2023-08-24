use std::process::Command;
use std::str::from_utf8;

fn main() {
    let output = match Command::new("git").arg("rev-parse").arg("HEAD").output() {
        Ok(output) => output,
        Err(_) => {
            return;
        }
    };
    let mut hash = from_utf8(&output.stdout).unwrap().trim().to_string();

    let output_dirty = match Command::new("git").arg("diff").arg("--exit-code").output() {
        Ok(output) => output,
        Err(_) => {
            return;
        }
    };

    if !output_dirty.status.success() {
        hash.push_str(".m");
    }
    println!("cargo:rustc-env=CARGO_GIT_HASH={}", hash);
}
