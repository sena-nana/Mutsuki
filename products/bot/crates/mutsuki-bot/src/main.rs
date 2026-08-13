use mutsuki_bot::{load_single_instance_product, run_single_instance_product};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    reject_arguments(std::env::args_os().skip(1))?;
    let product = load_single_instance_product().await?;
    run_single_instance_product(product).await?;
    Ok(())
}

fn reject_arguments(arguments: impl IntoIterator<Item = std::ffi::OsString>) -> Result<(), String> {
    if arguments.into_iter().next().is_some() {
        return Err("single_instance.arguments_unsupported: mutsuki-bot accepts no configuration path or profile".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_entry_rejects_configuration_arguments() {
        assert!(reject_arguments(Vec::new()).is_ok());
        assert!(reject_arguments(["local.toml".into()]).is_err());
    }
}
