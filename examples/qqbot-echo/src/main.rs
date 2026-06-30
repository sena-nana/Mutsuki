fn main() -> Result<(), Box<dyn std::error::Error>> {
    let report = qqbot_echo::run_smoke(qqbot_echo::EchoSmokeConfig::from_env())?;
    println!("{}", serde_json::to_string_pretty(&report.to_json())?);
    Ok(())
}
