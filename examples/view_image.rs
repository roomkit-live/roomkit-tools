//! Prepare a local image; the payload must be delivered through a vision channel.
#![forbid(unsafe_code)]
#![expect(clippy::print_stdout, reason = "an executable example prints metadata")]

use base64::Engine;
use roomkit_tools_image::{ViewImageInput, ViewImageTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Keep a generic PNG fixture for the argument-free executable check.
    let root = tempfile::tempdir()?;
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/image.json"))?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(
        fixture
            .get("files")
            .and_then(|files| files.get("colors.png"))
            .and_then(serde_json::Value::as_str)
            .ok_or("fixture missing")?,
    )?;
    std::fs::write(root.path().join("image.png"), bytes)?;
    let tool = ViewImageTool::new(root.path()).await?;
    let result = tool
        .view(
            ViewImageInput {
                path: "image.png".into(),
                ..Default::default()
            },
            &CancellationToken::new(),
        )
        .await?;
    assert_eq!((result.data.width, result.data.height), (2, 2));
    assert_ne!(result.data.image.base64, "");
    print!("{}", result.text);
    Ok(())
}
