//! Backend independence and shared wire boundaries.

use std::sync::{Arc, Mutex};

use roomkit_tools_agents::*;
use roomkit_tools_core::ToolError;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Default)]
struct Backend(Mutex<Vec<String>>);

fn receipt() -> TaskInfo {
    TaskInfo {
        task_id: "t1".into(),
        agent_id: "a1".into(),
        profile: "reader".into(),
        status: TaskState::Running,
        delivery: DeliveryState::Pending,
        result: None,
        result_truncated: false,
        agent_available: true,
    }
}

impl AgentBackend for Backend {
    fn delegate<'a>(
        &'a self,
        scope: &'a AgentScope,
        input: &'a DelegateInput,
        cancel: &'a CancellationToken,
    ) -> AgentFuture<'a, TaskInfo> {
        Box::pin(async move {
            if input.profile == "waits" {
                cancel.cancelled().await;
                return Err(ToolError::Cancelled);
            }
            self.0
                .lock()
                .map_err(|_| ToolError::AccessRefused("test backend poisoned".into()))?
                .push(format!("{}:{}", scope.room_id, input.profile));
            Ok(receipt())
        })
    }
    fn status<'a>(
        &'a self,
        _: &'a AgentScope,
        _: &'a StatusInput,
        _: &'a CancellationToken,
    ) -> AgentFuture<'a, TaskList> {
        Box::pin(async {
            Ok(TaskList {
                tasks: vec![receipt()],
                pending_launches: 0,
            })
        })
    }
    fn cancel<'a>(
        &'a self,
        _: &'a AgentScope,
        _: &'a CancelInput,
        _: &'a CancellationToken,
    ) -> AgentFuture<'a, CancelResult> {
        Box::pin(async {
            Ok(CancelResult {
                task: TaskInfo {
                    status: TaskState::Cancelled,
                    delivery: DeliveryState::Suppressed,
                    ..receipt()
                },
                cancelled: true,
            })
        })
    }
}

#[tokio::test]
async fn selected_tools_validate_before_backend_and_share_scope() {
    let backend = Arc::new(Backend::default());
    let shared: Arc<dyn AgentBackend> = Arc::<Backend>::clone(&backend);
    let tool = DelegateTaskTool::new(Arc::clone(&shared));
    let scope = AgentScope {
        room_id: "room".into(),
        caller_id: "assistant".into(),
    };
    let mut input = DelegateInput {
        profile: "reader".into(),
        task: " ".into(),
    };
    let cancel = CancellationToken::new();
    assert!(matches!(
        tool.call(&scope, input.clone(), &cancel).await,
        Err(ToolError::InvalidInput(_))
    ));
    assert!(backend.0.lock().unwrap().is_empty());
    input.task = "Read the source".into();
    assert_eq!(
        tool.call(&scope, input, &cancel)
            .await
            .unwrap()
            .data
            .task_id,
        "t1"
    );
    assert_eq!(backend.0.lock().unwrap().as_slice(), ["room:reader"]);
    assert_eq!(
        TaskStatusTool::new(Arc::clone(&shared))
            .call(&scope, StatusInput::default(), &cancel)
            .await
            .unwrap()
            .data
            .tasks
            .len(),
        1
    );
    assert!(
        CancelTaskTool::new(shared)
            .call(
                &scope,
                CancelInput {
                    task_id: "t1".into()
                },
                &cancel
            )
            .await
            .unwrap()
            .data
            .cancelled
    );
}

#[tokio::test]
async fn a_cancelled_call_refuses_before_reaching_the_backend() {
    let backend = Arc::new(Backend::default());
    let tool = DelegateTaskTool::new(Arc::<Backend>::clone(&backend));
    let scope = AgentScope {
        room_id: "room".into(),
        caller_id: "assistant".into(),
    };
    let cancel = CancellationToken::new();
    cancel.cancel();
    let input = DelegateInput {
        profile: "reader".into(),
        task: "Read the source".into(),
    };
    assert!(matches!(
        tool.call(&scope, input, &cancel).await,
        Err(ToolError::Cancelled)
    ));
    assert!(backend.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_backend_observes_the_callers_cancellation() {
    let tool = DelegateTaskTool::new(Arc::new(Backend::default()));
    let scope = AgentScope {
        room_id: "room".into(),
        caller_id: "assistant".into(),
    };
    let cancel = CancellationToken::new();
    let input = DelegateInput {
        profile: "waits".into(),
        task: "Read the source".into(),
    };
    // The call is polled first and waits on the token; cancelling it ends the call.
    let (result, ()) = tokio::join!(tool.call(&scope, input, &cancel), async {
        cancel.cancel();
    });
    assert!(matches!(result, Err(ToolError::Cancelled)));
}

#[test]
fn json_rejects_host_scope_and_limits_utf8_bytes() {
    assert!(
        decode::<DelegateInput>(r#"{"profile":"reader","task":"read","room_id":"other"}"#).is_err()
    );
    let input = DelegateInput {
        profile: "reader".into(),
        task: "é".repeat(8193),
    };
    assert!(matches!(input.validate(), Err(ToolError::LimitExceeded(_))));
    let mut output = receipt();
    output.result = Some("invented while running".into());
    assert!(render(output).is_err());
    assert!(
        render(TaskList {
            tasks: vec![receipt(), receipt()],
            pending_launches: 0,
        })
        .is_err()
    );
    assert!(
        render(CancelResult {
            task: receipt(),
            cancelled: true
        })
        .is_err()
    );
    let schema = DelegateTaskTool::definition().input_schema;
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["properties"].as_object().unwrap().len(), 2);
}

#[tokio::test]
async fn extensions_fail_explicitly_for_older_backends() {
    let backend: Arc<dyn AgentBackend> = Arc::new(Backend::default());
    let scope = AgentScope {
        room_id: "room".into(),
        caller_id: "assistant".into(),
    };
    assert!(matches!(
        AgentFollowupTool::new(Arc::clone(&backend))
            .call(
                &scope,
                FollowupInput {
                    agent_id: "a1".into(),
                    task: "Continue".into()
                },
                &CancellationToken::new()
            )
            .await,
        Err(ToolError::UnsupportedResource(_))
    ));
    assert!(matches!(
        AgentSendTool::new(backend)
            .call(
                &scope,
                SendInput {
                    agent_id: "a1".into(),
                    message: "Clarify".into()
                },
                &CancellationToken::new()
            )
            .await,
        Err(ToolError::UnsupportedResource(_))
    ));
    assert!(
        decode::<FollowupInput>(r#"{"agent_id":"a1","task":"Continue","profile":"other"}"#)
            .is_err()
    );
    assert!(matches!(
        SendInput {
            agent_id: "a1".into(),
            message: "é".repeat(4097)
        }
        .validate(),
        Err(ToolError::LimitExceeded(_))
    ));
    let mut sent = SendResult {
        agent_id: "a1".into(),
        task_id: Some("t1".into()),
        message_id: None,
        status: SendState::Queued,
        history_recorded: false,
    };
    assert!(sent.validate().is_err());
    sent.message_id = Some("m1".into());
    assert!(sent.validate().is_ok());
    sent.status = SendState::NotRunning;
    assert!(sent.validate().is_err());
    sent.message_id = None;
    assert!(sent.validate().is_ok());
}
