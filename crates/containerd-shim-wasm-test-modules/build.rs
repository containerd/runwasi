use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use lazy_static::lazy_static;

fn env_path(key: impl AsRef<str>) -> Result<PathBuf> {
    std::env::var_os(key.as_ref())
        .map(Into::into)
        .with_context(|| format!("failed to read env-var {}", key.as_ref()))
}

lazy_static! {
    static ref OUT_DIR: PathBuf = env_path("OUT_DIR").unwrap();
    static ref PKG_DIR: PathBuf = env_path("CARGO_MANIFEST_DIR").unwrap();
}

fn main() -> Result<()> {
    let modules_file = OUT_DIR.join("modules.rs");
    let modules_dir = PKG_DIR.join("src").join("modules");

    let mut writer = std::fs::File::create(modules_file)?;

    let paths = std::fs::read_dir(modules_dir)?;
    for entry in paths.flatten() {
        let src = entry.path();
        let name = path_to_ident(&src)?.to_ascii_uppercase();

        println!("cargo:rerun-if-changed={}", src.to_string_lossy());

        writeln!(writer, "pub const {name}: TestModule = TestModule {{")?;
        let dst = match src
            .extension()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default()
        {
            "rs" => {
                writeln!(writer, "    source: Some(include_str!({src:?})),")?;
                compile_rust(&src)?
            }
            "wat" => {
                writeln!(writer, "    source: Some(include_str!({src:?})),")?;
                compile_wat(&src)?
            }
            "wasm" => {
                writeln!(writer, "    source: None,")?;
                move_wasm(&src)?
            }
            _ => bail!("unrecognized file format for source file {src:?}"),
        };

        writeln!(writer, "    bytes: include_bytes!({dst:?}),")?;
        writeln!(writer, "}};")?;
    }

    Ok(())
}

fn compile_rust(src: impl AsRef<Path>) -> Result<PathBuf> {
    let rustc = std::env::var_os("RUSTC").context("reading RUSTC")?;
    let src = src.as_ref();
    let dst = output_for(src)?;

    Command::new(rustc)
        .arg("--target=wasm32-wasip1")
        .arg("-Copt-level=z")
        .arg("-Cstrip=symbols")
        .arg("-o")
        .arg(&dst)
        .arg(src)
        .spawn()?
        .wait()?
        .success()
        .then_some(dst)
        .context("running rustc")
}

fn compile_wat(src: impl AsRef<Path>) -> Result<PathBuf> {
    let src = src.as_ref();
    let dst = output_for(src)?;

    let bytes = wat::parse_file(src)?;
    std::fs::write(&dst, bytes)?;

    Ok(dst)
}

fn move_wasm(src: impl AsRef<Path>) -> Result<PathBuf> {
    let src = src.as_ref();
    let dst = output_for(src)?;

    std::fs::copy(src, &dst)?;

    Ok(dst)
}

fn output_for(src: impl AsRef<Path>) -> Result<PathBuf> {
    let src = src.as_ref();
    let filename = src
        .file_name()
        .with_context(|| format!("getting filename of {src:?}"))?;
    Ok(OUT_DIR.join(filename).with_extension("wasm"))
}

fn path_to_ident(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    let ident: String = path
        .file_stem()
        .with_context(|| format!("getting filename of {path:?}"))?
        .to_str()
        .context("converting filename to string")?
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' => c,
            _ => '_',
        })
        .collect();

    if !ident.starts_with(char::is_alphabetic) {
        bail!("please start the filename with [a-zA-Z]")
    }

    Ok(ident)
}
