fn main() {
    // Add current working dir request so that we have some known system call to
    // test seccomp with.
    let cwd = std::env::current_dir().unwrap();

    println!(
        "hello world, current working dir: {}",
        cwd.to_string_lossy()
    );
}
