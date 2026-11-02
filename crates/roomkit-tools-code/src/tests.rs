use super::*;

#[tokio::test]
async fn queued_calls_obey_deadlines_and_cancellation_without_losing_slots() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("source.rs"), "fn ready() {}\n").unwrap();
    let tool = CodeOutlineTool::new(root.path()).await.unwrap();
    let held = Arc::clone(&tool.workers)
        .acquire_many_owned(2)
        .await
        .unwrap();
    let token = CancellationToken::new();
    let mut input = CodeOutlineInput::new("source.rs");
    input.timeout_ms = Some(1);
    assert!(matches!(
        tool.outline(input, &token).await,
        Err(ToolError::LimitExceeded(_))
    ));
    let pending = tool.outline(CodeOutlineInput::new("source.rs"), &token);
    let stop = async {
        tokio::task::yield_now().await;
        token.cancel();
    };
    let (result, ()) = tokio::join!(pending, stop);
    assert!(matches!(result, Err(ToolError::Cancelled)));
    drop(held);
    let result = tool
        .outline(
            CodeOutlineInput::new("source.rs"),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.data.symbols[0].name, "ready");
    assert_eq!(tool.workers.available_permits(), 2);
}

#[test]
fn parser_work_checks_cooperative_deadline_and_cancellation() {
    let token = CancellationToken::new();
    let mut control = extract::Control {
        token: token.clone(),
        deadline: Instant::now(),
    };
    let run = |control: &extract::Control| {
        extract::outline(
            "fn example() {}",
            CodeLanguage::Rust,
            &CodeOutlineInput::new("source.rs"),
            CodeOutlineLimits::default(),
            control,
        )
    };
    assert!(matches!(run(&control), Err(ToolError::LimitExceeded(_))));
    control.deadline = Instant::now() + Duration::from_secs(5);
    token.cancel();
    assert!(matches!(run(&control), Err(ToolError::Cancelled)));
}
