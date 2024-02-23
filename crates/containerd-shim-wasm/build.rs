use std::env::var_os;
use std::path::Path;

use ttrpc_codegen::{Codegen, ProtobufCustomize};

fn main() {
    let protos = ["protos/sandbox.proto"];
    println!("cargo:rerun-if-changed=protos/sandbox.proto");

    let out_dir = var_os("OUT_DIR").unwrap();
    let out_dir = Path::new(&out_dir);

    Codegen::new()
        .out_dir(out_dir)
        .inputs(protos)
        .include("protos")
        .rust_protobuf()
        .rust_protobuf_customize(ProtobufCustomize::default().gen_mod_rs(false))
        .run()
        .expect("failed to generate code");

    let sanbox_rs = out_dir.join("sandbox.rs");
    let sanbox_ttrpc_rs = out_dir.join("sandbox_ttrpc.rs");

    std::fs::write(
        out_dir.join("mod.rs"),
        format!(
            r#"
#[path = {sanbox_rs:?}] pub mod sandbox;
#[path = {sanbox_ttrpc_rs:?}] pub mod sandbox_ttrpc;
"#,
        ),
    )
    .expect("failed to generate module");
}
