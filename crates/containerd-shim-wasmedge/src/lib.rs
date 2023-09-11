use std::env;

use containerd_shim::parse;

#[cfg_attr(unix, path = "instance_linux.rs")]
#[cfg_attr(windows, path = "instance_windows.rs")]
pub mod instance;

pub use instance::WasmEdgeInstance;

pub fn parse_version() {
    let os_args: Vec<_> = env::args_os().collect();
    let flags = parse(&os_args[1..]).unwrap();
    if flags.version {
        println!("{}:", os_args[0].to_string_lossy());
        println!("  Version: {}", env!("CARGO_PKG_VERSION"));
        println!("  Revision: {}", env!("CARGO_GIT_HASH"));
        println!();

        std::process::exit(0);
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests;
