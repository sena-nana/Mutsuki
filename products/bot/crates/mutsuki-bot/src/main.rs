#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    mutsuki_bot::run_single_instance_product_entry().await?;
    Ok(())
}
