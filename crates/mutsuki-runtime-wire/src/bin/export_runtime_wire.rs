fn main() {
    let artifact = std::env::args().nth(1).unwrap_or_else(|| "schema".into());
    match artifact.as_str() {
        "schema" => print!("{}", mutsuki_runtime_wire::generated_schema_json()),
        "fixtures" => print!("{}", mutsuki_runtime_wire::generated_fixtures_json()),
        other => panic!("unknown runtime wire artifact {other}"),
    }
}
