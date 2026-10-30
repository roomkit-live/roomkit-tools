//! Backend independence and shared wire boundaries.

use std::sync::{Arc, Mutex};

use roomkit_tools_agents::*;
use roomkit_tools_core::Error;

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
    ) -> AgentFuture<'a, TaskInfo> {
        Box::pin(async move {
            self.0
                .lock()
                .map_err(|_| Error::Refused("test backend poisoned".into()))?
                .push(format!("{}:{}", scope.room_id, input.profile));
            Ok(receipt())
        })
    }
    fn status<'a>(&'a self, _: &'a AgentScope, _: &'a StatusInput) -> AgentFuture<'a, TaskList> {
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
    assert!(matches!(
        tool.call(&scope, &input).await,
        Err(Error::InvalidInput(_))
    ));
    assert!(backend.0.lock().unwrap().is_empty());
    input.task = "Read the source".into();
    assert_eq!(tool.call(&scope, &input).await.unwrap().data.task_id, "t1");
    assert_eq!(backend.0.lock().unwrap().as_slice(), ["room:reader"]);
    assert_eq!(
        TaskStatusTool::new(Arc::clone(&shared))
            .call(&scope, &StatusInput::default())
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
                &CancelInput {
                    task_id: "t1".into()
                }
            )
            .await
            .unwrap()
            .data
            .cancelled
    );
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
    assert!(matches!(input.validate(), Err(Error::LimitExceeded(_))));
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
                &FollowupInput {
                    agent_id: "a1".into(),
                    task: "Continue".into()
                }
            )
            .await,
        Err(Error::Unsupported(_))
    ));
    assert!(matches!(
        AgentSendTool::new(backend)
            .call(
                &scope,
                &SendInput {
                    agent_id: "a1".into(),
                    message: "Clarify".into()
                }
            )
            .await,
        Err(Error::Unsupported(_))
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
        Err(Error::LimitExceeded(_))
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
