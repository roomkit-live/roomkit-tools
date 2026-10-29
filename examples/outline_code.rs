//! Locate a method using its source outline, then read only that method.
#![expect(clippy::print_stdout, reason = "executable example output")]

use roomkit_tools_code::{CodeOutlineInput, CodeOutlineTool};
use roomkit_tools_fs::{ReadInput, ReadTool};
use tokio_util::sync::CancellationToken;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    std::fs::write(
        root.path().join("shipping.py"),
        "class ShippingService:\n    def calculate_fee(self, subtotal):\n        return 0 if subtotal >= 50 else 5\n",
    )?;
    let token = CancellationToken::new();
    let tool = CodeOutlineTool::new(root.path()).await?;
    let result = tool
        .outline(CodeOutlineInput::new("shipping.py"), &token)
        .await?;
    assert!(!result.data.syntax_errors);
    let method = result
        .data
        .symbols
        .iter()
        .find(|symbol| symbol.name == "calculate_fee")
        .ok_or("method not found")?;
    let mut input = ReadInput::new("shipping.py");
    input.offset = method.start_line.try_into()?;
    input.limit = Some((method.end_line - method.start_line + 1).try_into()?);
    let read = ReadTool::new(root.path())
        .await?
        .read(input, &token)
        .await?;
    assert_eq!(read.data.revision, result.data.revision);
    assert_eq!(read.data.lines.len(), 2);
    println!("{}\n{}", result.text, read.text);
    Ok(())
}
