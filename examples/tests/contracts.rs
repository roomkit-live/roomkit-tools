//! Common fixtures check the native API and supply installed-Python comparisons.
#![cfg(unix)]
#![expect(clippy::unwrap_used, reason = "invalid fixtures must fail the test")]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    DiffTool, EditTool, GlobTool, GrepTool, ListTool, MutationWorkspace, PatchTool, ReadTool,
    WriteTool,
};
use roomkit_tools_process::{
    ProcessLimits, ProcessManager, ProcessStatusInput, ShellLimits, ShellTool,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

const FIXTURE: &str = include_str!("../../tests/fixtures/contracts.json");
const OWNER: &str = "contract-host";

fn input<T: DeserializeOwned>(arguments: &Value) -> Result<T, Error> {
    serde_json::from_value(arguments.clone())
        .map_err(|error| Error::InvalidInput(error.to_string()))
}

async fn invoke(
    root: &Path,
    workspace: &MutationWorkspace,
    manager: &ProcessManager,
    step: &Value,
    token: &CancellationToken,
) -> Result<Value, Error> {
    let arguments = &step["input"];
    macro_rules! call {
        ($tool:expr, $method:ident) => {
            Ok(serde_json::to_value($tool.$method(input(arguments)?, token).await?).unwrap())
        };
    }
    macro_rules! process {
        ($method:ident) => {
            Ok(
                serde_json::to_value(manager.$method(OWNER, input(arguments)?, token).await?)
                    .unwrap(),
            )
        };
    }
    match step["tool"].as_str().unwrap() {
        "read" => call!(ReadTool::new(root).await?, read),
        "list" => call!(ListTool::new(root).await?, list),
        "glob" => call!(GlobTool::new(root).await?, glob),
        "grep" => call!(GrepTool::new(root).await?, grep),
        "diff" => call!(DiffTool::new(root).await?, diff),
        "write" => call!(WriteTool::from_workspace(workspace.clone()), write),
        "edit" => call!(EditTool::from_workspace(workspace.clone()), edit),
        "patch" => call!(PatchTool::from_workspace(workspace.clone()), patch),
        "shell" => {
            let mut limits = ShellLimits::default();
            limits.max_output_bytes = 8;
            call!(ShellTool::new(root).await?.with_limits(limits)?, shell)
        }
        "process_start" => process!(start),
        "process_status" => process!(status),
        "process_read" => process!(read),
        "process_write" => process!(write),
        "process_stop" => process!(stop),
        name => Err(Error::InvalidInput(format!("unknown fixture tool: {name}"))),
    }
}

fn error_name(error: &Error) -> &str {
    match error {
        Error::InvalidInput(_) => "invalid_input",
        Error::Refused(_) => "refused",
        Error::LimitExceeded(_) => "limit",
        Error::Unsupported(_) => "unsupported",
        Error::Cancelled => "cancelled",
        Error::Io { .. } => "io",
        _ => "unknown",
    }
}

fn normalize(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace(root, "$ROOT"),
        Value::Array(values) => values.iter_mut().for_each(|v| normalize(v, root)),
        Value::Object(values) => values.values_mut().for_each(|v| normalize(v, root)),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn bytes(value: &Value) -> Vec<u8> {
    match value.as_str() {
        Some(text) => text.as_bytes().to_vec(),
        None => serde_json::from_value(value.clone()).unwrap(),
    }
}

fn snapshot(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            snapshot(root, &path, files);
        } else {
            files.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned(),
                std::fs::read(path).unwrap(),
            );
        }
    }
}

async fn wait(manager: &ProcessManager, expected: &Value) {
    // Synchronize on observed output/state, never on a guessed process delay.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let observed = manager
                .status(
                    OWNER,
                    ProcessStatusInput {
                        process_id: Some("p1".into()),
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            let observed = serde_json::to_value(observed).unwrap();
            if expected
                .as_object()
                .unwrap()
                .iter()
                .all(|(key, value)| observed.pointer(key).is_some_and(|actual| actual == value))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn shared_contracts() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let mut reports = BTreeMap::new();
    for case in fixture["cases"].as_array().unwrap() {
        let directory = tempfile::Builder::new()
            .prefix("tool contracts ")
            .tempdir()
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        run_case(&root, case, &mut reports).await;
    }
    if let Ok(path) = std::env::var("ROOMKIT_CONTRACT_RESULTS") {
        std::fs::write(
            path,
            serde_json::to_vec(&json!({"fixture": fixture, "reports": reports})).unwrap(),
        )
        .unwrap();
    }
}

async fn run_case(root: &Path, case: &Value, reports: &mut BTreeMap<String, Value>) {
    let name = case["name"].as_str().unwrap();
    for (path, content) in case["files"].as_object().unwrap() {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes(content)).unwrap();
    }
    let workspace = MutationWorkspace::new(root).await.unwrap();
    let mut limits = ProcessLimits::default();
    limits.max_output_bytes = 8;
    limits.max_read_bytes = 3;
    let manager = ProcessManager::new(root)
        .await
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let mut results = Vec::new();
    for step in case["steps"].as_array().unwrap() {
        if let Some(expected) = step.get("wait") {
            wait(&manager, expected).await;
        }
        let token = CancellationToken::new();
        if step["cancelled"] == true {
            token.cancel();
        }
        let mut result = match invoke(root, &workspace, &manager, step, &token).await {
            Ok(output) => output,
            Err(error) => json!({"error": error_name(&error)}),
        };
        normalize(&mut result, root.to_str().unwrap());
        for (pointer, expected) in step["expect"].as_object().unwrap() {
            assert_eq!(result.pointer(pointer), Some(expected), "{name}: {step}");
        }
        results.push(result);
    }
    assert!(manager.close().await);
    let mut files = BTreeMap::new();
    snapshot(root, root, &mut files);
    let expected = case
        .get("files_after")
        .unwrap_or(&case["files"])
        .as_object()
        .unwrap()
        .iter()
        .map(|(path, value)| (path.clone(), bytes(value)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(files, expected, "{name}: final file bytes");
    reports.insert(name.into(), json!({"results": results, "files": files}));
}
