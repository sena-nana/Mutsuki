fn main() -> Result<(), Box<dyn std::error::Error>> {
    let validation = qqbot_echo::validate_example_flow()?;
    if !validation.valid {
        return Err(format!("example flow is invalid: {:?}", validation.issues).into());
    }
    println!(
        "Bot Flow draft for the ServiceHost/Web Console:\n{}",
        serde_json::to_string_pretty(&qqbot_echo::example_flow_config_json())?
    );
    Ok(())
}
