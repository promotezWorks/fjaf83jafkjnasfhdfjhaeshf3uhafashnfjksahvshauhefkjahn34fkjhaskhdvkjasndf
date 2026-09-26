// language: Rust, file: src/main.rs, target: Windows
// Standalone agent exe (testing / direct run). The injectable payload is the cdylib.
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    stoat_agent::run_from_exe().await
}
