use std::env;

use containerd_shim::parse;

pub mod error;

#[cfg_attr(unix, path = "instance/instance_linux.rs")]
#[cfg_attr(windows, path = "instance/instance_windows.rs")]
pub mod instance;
pub mod oci_utils;

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

#[cfg(unix)]
#[cfg(test)]
mod test {
    use std::os::unix::prelude::OsStrExt;

    // Get the path to binary where the `WasmEdge_VersionGet` C ffi symbol is defined.
    // If wasmedge is dynamically linked, this will be the path to the `.so`.
    // If wasmedge is statically linked, this will be the path to the current executable.
    fn get_wasmedge_binary_path() -> Option<std::path::PathBuf> {
        let f = wasmedge_sys::ffi::WasmEdge_VersionGet;
        let mut info = unsafe { std::mem::zeroed() };
        if unsafe { libc::dladdr(f as *const libc::c_void, &mut info) } == 0 {
            None
        } else {
            let fname = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
            let fname = std::ffi::OsStr::from_bytes(fname.to_bytes());
            Some(std::path::PathBuf::from(fname))
        }
    }

    #[cfg(feature = "static")]
    #[test]
    fn check_static_linking() {
        let wasmedge_path = get_wasmedge_binary_path().unwrap().canonicalize().unwrap();
        let current_exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert!(wasmedge_path == current_exe);
    }

    #[cfg(not(feature = "static"))]
    #[test]
    fn check_dynamic_linking() {
        let wasmedge_path = get_wasmedge_binary_path().unwrap().canonicalize().unwrap();
        let current_exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert!(wasmedge_path != current_exe);
    }
}
