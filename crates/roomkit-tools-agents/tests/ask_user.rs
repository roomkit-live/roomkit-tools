//! `ask_user`: the questions a call may ask, the answers a backend returns.
#![expect(
    clippy::unwrap_used,
    clippy::panic,
    reason = "fixture errors must fail tests"
)]

use std::sync::{Arc, Mutex};

use roomkit_tools_agents::*;
use roomkit_tools_core::ToolError;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

/// A backend whose person picks the first option of each question.
#[derive(Debug, Default)]
struct Person(Mutex<Vec<String>>);

impl AgentBackend for Person {
    fn delegate<'a>(
        &'a self,
        _: &'a AgentScope,
        _: &'a DelegateTaskInput,
        _: &'a CancellationToken,
    ) -> AgentFuture<'a, TaskInfo> {
        Box::pin(async { Err(ToolError::UnsupportedResource("no tasks".into())) })
    }
    fn status<'a>(
        &'a self,
        _: &'a AgentScope,
        _: &'a TaskStatusInput,
        _: &'a CancellationToken,
    ) -> AgentFuture<'a, TaskList> {
        Box::pin(async { Err(ToolError::UnsupportedResource("no tasks".into())) })
    }
    fn cancel<'a>(
        &'a self,
        _: &'a AgentScope,
        _: &'a CancelTaskInput,
        _: &'a CancellationToken,
    ) -> AgentFuture<'a, CancelResult> {
        Box::pin(async { Err(ToolError::UnsupportedResource("no tasks".into())) })
    }
    fn ask<'a>(
        &'a self,
        scope: &'a AgentScope,
        input: &'a AskUserInput,
        _: &'a CancellationToken,
    ) -> AgentFuture<'a, AskUserResult> {
        Box::pin(async move {
            let mut asked = self.0.lock().unwrap();
            asked.extend(input.questions.iter().map(|q| q.question.clone()));
            asked.push(scope.caller_id.clone());
            let answers = input
                .questions
                .iter()
                .map(|q| AskUserAnswer {
                    selected: q.options.iter().take(1).map(|o| o.label.clone()).collect(),
                    other: q.options.is_empty().then(|| "Blue".to_owned()),
                })
                .collect();
            Ok(AskUserResult { answers })
        })
    }
}

fn arguments(value: Value) -> Map<String, Value> {
    let Value::Object(map) = value else {
        panic!("not an object: {value}")
    };
    map
}

fn question(text: &str, labels: &[&str]) -> Value {
    let options: Vec<Value> = labels.iter().map(|l| json!({"label": l})).collect();
    json!({"question": text, "options": options})
}

fn refusal(value: Value) -> String {
    match AskUserTool::read(&arguments(value)) {
        Err(ToolError::InvalidInput(message) | ToolError::LimitExceeded(message)) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_call_reads_with_its_defaults() {
    let input = AskUserTool::read(&arguments(json!({"questions": [
        {"question": "Which database?", "header": "Database", "multi_select": true,
         "options": [{"label": "SQLite", "description": "One file"}, {"label": "Postgres"}]},
        {"question": "Your name?"}
    ]})))
    .unwrap();
    let [first, second] = input.questions.as_slice() else {
        panic!("two questions")
    };
    assert_eq!(first.header, "Database");
    assert!(first.multi_select);
    assert_eq!(first.options[0].description, "One file");
    assert_eq!(first.options[1].description, "");
    assert_eq!(second.header, "");
    assert!(second.options.is_empty());
    assert!(!second.multi_select);
}

#[test]
fn a_call_is_refused_with_what_to_fix() {
    let five: Vec<Value> = (1..=5).map(|n| question(&format!("Q{n}?"), &[])).collect();
    assert_eq!(
        refusal(json!({"questions": five})),
        "ask_user takes one to 4 questions; 5 were given."
    );
    assert_eq!(
        refusal(json!({"questions": []})),
        "ask_user takes one to 4 questions; 0 were given."
    );
    assert!(refusal(json!({})).contains("missing field `questions`"));
    assert!(refusal(json!({"questions": [{"question": "Q?", "kind": "x"}]})).contains("kind"));
    assert_eq!(
        refusal(json!({"questions": [question("A?", &[]), question("  ", &[])]})),
        "Question 2 has no text."
    );
    assert_eq!(
        refusal(json!({"questions": [question("A?", &["a", "b", "c", "d", "e"])]})),
        "Question 1 offers 5 options; at most 4."
    );
    assert_eq!(
        refusal(json!({"questions": [question("A?", &["Yes", " Yes "])]})),
        "Question 1 needs distinct, non-empty option labels."
    );
    assert_eq!(
        refusal(json!({"questions": [question("A?", &["Yes", ""])]})),
        "Question 1 needs distinct, non-empty option labels."
    );
    assert_eq!(
        refusal(json!({"questions": [question(&"é".repeat(513), &[])]})),
        "Question 1's text exceeds 1024 bytes."
    );
}

#[test]
fn the_definition_bounds_questions_and_options() {
    let definition = AskUserTool::definition();
    assert_eq!(definition.name, "ask_user");
    let schema = &definition.input_schema;
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["required"], json!(["questions"]));
    let questions = &schema["properties"]["questions"];
    assert_eq!(
        (&questions["minItems"], &questions["maxItems"]),
        (&json!(1), &json!(4))
    );
    let question = &schema["$defs"]["AskUserQuestion"];
    assert_eq!(question["required"], json!(["question"]));
    assert_eq!(question["properties"]["options"]["maxItems"], 4);
    assert_eq!(
        schema["$defs"]["AskUserOption"]["required"],
        json!(["label"])
    );
}

#[tokio::test]
async fn the_backend_answers_checked_questions() {
    let person = Arc::new(Person::default());
    let tool = AskUserTool::new(Arc::<Person>::clone(&person));
    let scope = AgentScope {
        room_id: "room".into(),
        caller_id: "assistant".into(),
    };
    let input = AskUserTool::read(&arguments(json!({"questions": [
        question("Which database?", &["SQLite", "Postgres"]),
        question("Favourite colour?", &[])
    ]})))
    .unwrap();
    let output = tool
        .call(&scope, input, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(output.data.answers[0].selected, ["SQLite"]);
    assert_eq!(output.data.answers[1].other.as_deref(), Some("Blue"));
    assert_eq!(
        serde_json::from_str::<Value>(&output.text).unwrap(),
        json!({"answers": [
            {"selected": ["SQLite"], "other": null},
            {"selected": [], "other": "Blue"}
        ]})
    );
    assert_eq!(
        *person.0.lock().unwrap(),
        ["Which database?", "Favourite colour?", "assistant"]
    );

    let empty = AskUserInput { questions: vec![] };
    let refused = tool.call(&scope, empty, &CancellationToken::new()).await;
    assert!(matches!(refused, Err(ToolError::InvalidInput(_))));
    assert_eq!(
        person.0.lock().unwrap().len(),
        3,
        "never reached the backend"
    );
}

#[test]
fn answers_are_bounded() {
    let answer = |selected: usize, other: usize| AskUserResult {
        answers: vec![AskUserAnswer {
            selected: (0..selected).map(|n| n.to_string()).collect(),
            other: Some("a".repeat(other)),
        }],
    };
    assert!(render(answer(4, 4096)).is_ok());
    assert!(matches!(
        render(answer(5, 0)),
        Err(ToolError::LimitExceeded(_))
    ));
    assert!(matches!(
        render(answer(0, 4097)),
        Err(ToolError::LimitExceeded(_))
    ));
    let five = AskUserResult {
        answers: vec![AskUserAnswer::default(); 5],
    };
    assert!(matches!(render(five), Err(ToolError::LimitExceeded(_))));
}

#[tokio::test]
async fn older_backends_refuse_explicitly() {
    #[derive(Debug)]
    struct Tasks;
    impl AgentBackend for Tasks {
        fn delegate<'a>(
            &'a self,
            _: &'a AgentScope,
            _: &'a DelegateTaskInput,
            _: &'a CancellationToken,
        ) -> AgentFuture<'a, TaskInfo> {
            Box::pin(async { Err(ToolError::UnsupportedResource("no".into())) })
        }
        fn status<'a>(
            &'a self,
            _: &'a AgentScope,
            _: &'a TaskStatusInput,
            _: &'a CancellationToken,
        ) -> AgentFuture<'a, TaskList> {
            Box::pin(async { Err(ToolError::UnsupportedResource("no".into())) })
        }
        fn cancel<'a>(
            &'a self,
            _: &'a AgentScope,
            _: &'a CancelTaskInput,
            _: &'a CancellationToken,
        ) -> AgentFuture<'a, CancelResult> {
            Box::pin(async { Err(ToolError::UnsupportedResource("no".into())) })
        }
    }
    let scope = AgentScope {
        room_id: "room".into(),
        caller_id: "assistant".into(),
    };
    let input = AskUserTool::read(&arguments(json!({"questions": [question("Q?", &[])]}))).unwrap();
    let result = AskUserTool::new(Arc::new(Tasks))
        .call(&scope, input, &CancellationToken::new())
        .await;
    assert!(matches!(result, Err(ToolError::UnsupportedResource(_))));
}
