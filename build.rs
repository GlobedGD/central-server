fn main() {
    capnpc::CompilerCommand::new()
        .src_prefix("schema")
        .file("./schema/main.capnp")
        .output_path("schema/generated")
        .run()
        .expect("capnpc failed");
}
