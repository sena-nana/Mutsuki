fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => print!("{}", generated("schema")),
        [artifact] => print!("{}", generated(artifact)),
        [command, root] if command == "write" => {
            let root = std::path::Path::new(root);
            std::fs::write(root.join("runtime-wire-v1.json"), generated("schema"))
                .expect("write runtime wire schema");
            std::fs::write(
                root.join("runtime-wire-fixtures-v1.json"),
                generated("fixtures"),
            )
            .expect("write runtime wire fixtures");
            std::fs::write(
                root.join("runtime-wire-binary-golden-v1.json"),
                generated("binary-golden"),
            )
            .expect("write runtime wire binary golden vectors");
        }
        other => panic!("usage: export_runtime_wire <schema|fixtures|write DIR>, got {other:?}"),
    }
}

fn generated(artifact: &str) -> String {
    match artifact {
        "schema" => mutsuki_runtime_wire::generated_schema_json(),
        "fixtures" => mutsuki_runtime_wire::generated_fixtures_json(),
        "binary-golden" => mutsuki_runtime_wire::generated_binary_golden_json(),
        other => panic!("unknown runtime wire artifact {other}"),
    }
}
