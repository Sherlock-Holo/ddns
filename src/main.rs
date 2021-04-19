use anyhow::Result;
use ddns::run;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    run().await
}
