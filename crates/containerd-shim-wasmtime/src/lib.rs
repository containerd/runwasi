use std::env;

use containerd_shim::parse;

pub mod error;
#[cfg_attr(unix, path = "instance/instance_linux.rs")]
#[cfg_attr(windows, path = "instance/instance_windows.rs")]
pub mod instance;
pub mod oci_wasmtime;

#[cfg(unix)]
pub mod executor;

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
