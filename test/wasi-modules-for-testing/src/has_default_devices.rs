use std::path::Path;

fn main() {
    // Runtime must supply at least the following files regardless of OCI devices setting:
    let devices = vec![
        "/dev/null",
        "/dev/zero",
        "/dev/full",
        "/dev/random",
        "/dev/urandom",
        "/dev/tty",
    ];

    for device in devices.iter() {
        if Path::new(device).exists() {
            println!("{} found", device);
        } else {
            panic!("{} not found", device);
        }
    }
}
