#[cfg(feature = "generate_bindings")]
use std::fs;

#[cfg(feature = "generate_bindings")]
use ttrpc_codegen::{Codegen, ProtobufCustomize};

#[cfg(not(feature = "generate_bindings"))]
fn main() {}

#[cfg(feature = "generate_bindings")]
fn main() {
    println!("cargo:rerun-if-changed=protos");
    fs::metadata("src/services/sandbox.rs").unwrap_or_else(|_| {
        fs::create_dir_all("src/services").unwrap();
        // always rerun if the directory doesn't exist
        println!("cargo:rerun-if-changed=src/services");
        fs::metadata("src/services").unwrap()
    });
    fs::metadata("src/services/sandbox_ttrpc.rs").unwrap_or_else(|_| {
        fs::create_dir_all("src/services").unwrap();
        // always rerun if the directory doesn't exist
        println!("cargo:rerun-if-changed=src/services/sandbox_ttrpc.rs");
        fs::metadata("src/services").unwrap()
    });

    let protos = vec!["protos/sandbox.proto"];

    Codegen::new()
        .out_dir("src/services")
        .inputs(&protos)
        .include("protos")
        .rust_protobuf()
        .rust_protobuf_customize(ProtobufCustomize {
            gen_mod_rs: Some(false),
            ..ProtobufCustomize::default()
        })
        .run()
        .expect("failed to generate code");
}
