fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = qqbot_echo::run_default_smoke()?;
    println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    Ok(())
}
