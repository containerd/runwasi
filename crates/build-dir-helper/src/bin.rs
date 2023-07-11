fn main() {
    let build_dir = std::env::current_exe()
        .unwrap()
        .canonicalize()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    println!("{}", build_dir.display());
}
