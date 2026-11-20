//! Backend independence and shared wire boundaries.

use std::sync::{Arc, Mutex};
use std::time::Duration;

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
        input: &'a DelegateTaskInput,
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
        _: &'a TaskStatusInput,
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
        _: &'a CancelTaskInput,
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
    let mut input = DelegateTaskInput {
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
            .call(&scope, TaskStatusInput::default(), &cancel)
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
                CancelTaskInput {
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
    let input = DelegateTaskInput {
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
    let input = DelegateTaskInput {
        profile: "waits".into(),
        task: "Read the source".into(),
    };
    // The call is polled first and waits on the token; cancelling it ends the
    // call. A backend handed another token would wait forever: the bound
    // turns that into a failure.
    let call = tokio::time::timeout(Duration::from_secs(10), tool.call(&scope, input, &cancel));
    let (result, ()) = tokio::join!(call, async {
        cancel.cancel();
    });
    let result = result.expect("the backend did not observe the caller's cancellation");
    assert!(matches!(result, Err(ToolError::Cancelled)));
}

#[test]
fn json_rejects_host_scope_and_limits_utf8_bytes() {
    assert!(
        decode::<DelegateTaskInput>(r#"{"profile":"reader","task":"read","room_id":"other"}"#)
            .is_err()
    );
    let input = DelegateTaskInput {
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
                AgentFollowupInput {
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
                AgentSendInput {
                    agent_id: "a1".into(),
                    message: "Clarify".into()
                },
                &CancellationToken::new()
            )
            .await,
        Err(ToolError::UnsupportedResource(_))
    ));
    assert!(
        decode::<AgentFollowupInput>(r#"{"agent_id":"a1","task":"Continue","profile":"other"}"#)
            .is_err()
    );
    assert!(matches!(
        AgentSendInput {
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

/// What a model reads of each agent tool: lines naming the IDs to copy and
/// the states, and a task's result in its own lines, not the JSON of data.
#[test]
fn each_result_reads_as_lines() {
    let report = "Found the bug.\nshipping.py:3 uses > where >= is meant.";
    let done = TaskInfo {
        status: TaskState::Completed,
        delivery: DeliveryState::Delivered,
        result: Some(report.into()),
        ..receipt()
    };
    assert_eq!(
        render(receipt()).unwrap().text,
        "task \"t1\": running, delivery pending; agent \"a1\" (profile \"reader\"), available\n"
    );
    let list = TaskList {
        tasks: vec![
            done.clone(),
            TaskInfo {
                task_id: "t2".into(),
                agent_available: false,
                ..receipt()
            },
        ],
        pending_launches: 1,
    };
    assert_eq!(
        render(list).unwrap().text,
        "2 tasks, 1 launch pending\n\
         1. task \"t1\": completed, delivery delivered; agent \"a1\" (profile \"reader\"), available\n   \
         result:\n   Found the bug.\n   shipping.py:3 uses > where >= is meant.\n\
         2. task \"t2\": running, delivery pending; agent \"a1\" (profile \"reader\"), not available\n"
    );
    let empty = TaskList {
        tasks: vec![],
        pending_launches: 0,
    };
    assert_eq!(render(empty).unwrap().text, "0 tasks\n");
    let ended = CancelResult {
        task: TaskInfo {
            result_truncated: true,
            ..done
        },
        cancelled: false,
    };
    assert_eq!(
        render(ended).unwrap().text,
        "not cancelled by this call\n\
         task \"t1\": completed, delivery delivered; agent \"a1\" (profile \"reader\"), available\n   \
         result (truncated):\n   Found the bug.\n   shipping.py:3 uses > where >= is meant.\n"
    );
    let queued = SendResult {
        agent_id: "a1".into(),
        task_id: Some("t1".into()),
        message_id: Some("m1".into()),
        status: SendState::Queued,
        history_recorded: true,
    };
    assert_eq!(
        render(queued).unwrap().text,
        "agent \"a1\": queued, task \"t1\", message \"m1\"; history recorded\n"
    );
    let idle = SendResult {
        agent_id: "a1".into(),
        task_id: None,
        message_id: None,
        status: SendState::NotRunning,
        history_recorded: false,
    };
    assert_eq!(
        render(idle).unwrap().text,
        "agent \"a1\": not_running; history not recorded\n"
    );
}

/// A result of 8 KiB, the bound, stays lines a model reads, not one escaped
/// string.
#[test]
fn a_long_result_keeps_its_lines() {
    let line = "x".repeat(63);
    let report = vec![line.as_str(); 128].join("\n");
    assert_eq!(report.len(), 8191);
    let text = render(TaskInfo {
        status: TaskState::Completed,
        delivery: DeliveryState::Delivered,
        result: Some(report),
        ..receipt()
    })
    .unwrap()
    .text;
    assert_eq!(text.lines().count(), 2 + 128);
    assert!(text.lines().skip(2).all(|l| l == format!("   {line}")));
    assert!(!text.contains("\\n"));
}

/// A lone carriage return, U+2028 and U+2029 break a line for whoever reads
/// it, so each starts an indented line, as `\n` and `\r\n` do: none leaves
/// what follows on the line of a task or an answer.
#[test]
fn every_line_break_starts_an_indented_line() {
    let breaks = "one\rtwo\u{2028}three\u{2029}four\r\nfive\nsix";
    let text = render(TaskInfo {
        status: TaskState::Completed,
        delivery: DeliveryState::Delivered,
        result: Some(breaks.into()),
        ..receipt()
    })
    .unwrap()
    .text;
    let result: Vec<&str> = text.split('\n').skip(2).collect();
    assert_eq!(
        result,
        [
            "   one", "   two", "   three", "   four", "   five", "   six", ""
        ]
    );
    let answers = render(AskUserResult {
        answers: vec![AskUserAnswer {
            selected: vec![],
            other: Some("Yes\rbut later\u{2028}today".into()),
        }],
    })
    .unwrap()
    .text;
    assert_eq!(answers, "1. wrote: Yes\n   but later\n   today\n");
}
