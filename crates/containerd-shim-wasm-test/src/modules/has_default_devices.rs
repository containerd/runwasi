use std::path::Path;

fn main() {
    // Runtime must supply at least the following files regardless of OCI devices setting:
    let devices = [
        "/dev/null",
        "/dev/zero",
        "/dev/full",
        "/dev/random",
        "/dev/urandom",
        "/dev/tty",
    ];

    for device in devices.iter() {
        if Path::new(device).exists() {
            println!("{device} found");
        } else {
            panic!("{device} not found");
        }
    }
}
