//! Host-owned output storage; all capture, paging and cleanup remain in Rust.

use std::path::PathBuf;
use std::sync::Arc;

use pyo3::prelude::*;
use roomkit_tools_core::Error;
use roomkit_tools_process::{
    OutputForgetInput, OutputLimits, OutputReadInput, OutputStore, ShellInput, ShellTool,
};
use serde::Deserialize;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use super::{Cancellation, InvalidInputError, ShellOptions, ToolError, tool_error};

// Python hosts supply a separate OutputStore per isolated conversation/agent.
pub(super) const OWNER: &str = "default";

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    max_archives: Option<usize>,
    max_archives_per_owner: Option<usize>,
    max_stream_bytes: Option<u64>,
    max_total_bytes: Option<u64>,
    max_read_bytes: Option<usize>,
    write_timeout_ms: Option<u64>,
}

impl Options {
    fn limits(self) -> OutputLimits {
        let mut limits = OutputLimits::default();
        macro_rules! set { ($($field:ident),+) => {$(if let Some(value) = self.$field { limits.$field = value; })+}; }
        set!(
            max_archives,
            max_archives_per_owner,
            max_stream_bytes,
            max_total_bytes,
            max_read_bytes,
            write_timeout_ms
        );
        limits
    }
}

#[derive(Debug)]
pub(super) struct State {
    parent: PathBuf,
    limits: OutputLimits,
    store: OnceCell<OutputStore>,
    closed: CancellationToken,
}

impl State {
    pub(super) async fn resolve(&self) -> Result<OutputStore, Error> {
        if self.closed.is_cancelled() {
            return Err(Error::Refused("output store is closed".into()));
        }
        let store = self
            .store
            .get_or_try_init(|| OutputStore::new(&self.parent, self.limits))
            .await?;
        if self.closed.is_cancelled() {
            store.close().await;
            return Err(Error::Refused("output store is closed".into()));
        }
        Ok(store.clone())
    }
}

#[pyclass(name = "_OutputStore", frozen, module = "roomkit_tools._native")]
#[derive(Debug)]
pub(super) struct PythonOutputStore {
    pub(super) state: Arc<State>,
}

impl Drop for PythonOutputStore {
    fn drop(&mut self) {
        self.state.closed.cancel();
        if let Some(store) = self.state.store.get().cloned() {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                store.close().await;
            });
        }
    }
}

#[pymethods]
impl PythonOutputStore {
    #[new]
    fn new(parent: PathBuf, options_json: &str) -> PyResult<Self> {
        let options: Options = serde_json::from_str(options_json)
            .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
        Ok(Self {
            state: Arc::new(State {
                parent,
                limits: options.limits(),
                store: OnceCell::new(),
                closed: CancellationToken::new(),
            }),
        })
    }

    fn is_closed(&self) -> bool {
        self.state.closed.is_cancelled()
    }

    #[staticmethod]
    fn definition_json(name: &str) -> PyResult<String> {
        let definition = match name {
            "output_read" => OutputStore::read_definition(),
            "output_forget" => OutputStore::forget_definition(),
            _ => return Err(InvalidInputError::new_err("unknown output tool")),
        };
        serde_json::to_string(&definition)
            .map_err(|_| ToolError::new_err("Unable to serialize output definition"))
    }

    fn call<'py>(
        &self,
        py: Python<'py>,
        name: String,
        arguments_json: String,
        cancellation: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let state = Arc::clone(&self.state);
        let token = cancellation.token.clone();
        let guard = token.clone().drop_guard();
        crate::runtime::future_into_py(py, async move {
            let _guard = guard;
            if token.is_cancelled() {
                return Err(tool_error(&Error::Cancelled));
            }
            macro_rules! invoke {
                ($method:ident, $input:ty) => {{
                    let input: $input = serde_json::from_str(&arguments_json)
                        .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
                    let store = state.resolve().await.map_err(|e| tool_error(&e))?;
                    let result = store
                        .$method(OWNER, input, &token)
                        .await
                        .map_err(|e| tool_error(&e))?;
                    serde_json::to_string(&result)
                        .map_err(|_| ToolError::new_err("Unable to serialize output result"))
                }};
            }
            match name.as_str() {
                "output_read" => invoke!(read, OutputReadInput),
                "output_forget" => invoke!(forget, OutputForgetInput),
                "initialize" | "list" => {
                    let store = state.resolve().await.map_err(|e| tool_error(&e))?;
                    let entries = store.list(OWNER, &token).map_err(|e| tool_error(&e))?;
                    serde_json::to_string(&entries)
                        .map_err(|_| ToolError::new_err("Unable to serialize output list"))
                }
                _ => Err(InvalidInputError::new_err("unknown output operation")),
            }
        })
    }

    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.state.closed.cancel();
        let worker = self.state.store.get().cloned().map(|store| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move { store.close().await })
        });
        crate::runtime::future_into_py(py, async move {
            match worker {
                Some(worker) => worker
                    .await
                    .map_err(|_| ToolError::new_err("Unable to join output cleanup")),
                None => Ok(true),
            }
        })
    }
}

#[pyclass(name = "_ShellTool", frozen, module = "roomkit_tools._native")]
#[derive(Clone, Debug)]
pub(super) struct PythonShellTool {
    tool: ShellTool,
}

#[pymethods]
impl PythonShellTool {
    #[staticmethod]
    #[pyo3(signature = (root, options_json, cancellation, outputs=None))]
    fn create<'py>(
        py: Python<'py>,
        root: PathBuf,
        options_json: String,
        cancellation: &Cancellation,
        outputs: Option<&PythonOutputStore>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let state = outputs.map(|outputs| Arc::clone(&outputs.state));
        let token = cancellation.token.clone();
        let guard = token.clone().drop_guard();
        crate::runtime::future_into_py(py, async move {
            let _guard = guard;
            if token.is_cancelled() {
                return Err(tool_error(&Error::Cancelled));
            }
            let mut options: ShellOptions = serde_json::from_str(&options_json)
                .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
            let env = super::process::command_env(options.inherit_env.take(), options.env.take())?;
            let mut tool = ShellTool::new(root)
                .await
                .and_then(|tool| tool.with_limits(options.limits()))
                .and_then(|tool| tool.with_env(env))
                .map_err(|e| tool_error(&e))?;
            if let Some(state) = state {
                tool = tool.with_output_store(state.resolve().await.map_err(|e| tool_error(&e))?);
            }
            if token.is_cancelled() {
                return Err(tool_error(&Error::Cancelled));
            }
            Ok(Self { tool })
        })
    }

    #[staticmethod]
    fn definition_json() -> PyResult<String> {
        serde_json::to_string(&ShellTool::definition())
            .map_err(|_| ToolError::new_err("Unable to serialize tool definition"))
    }

    fn call<'py>(
        &self,
        py: Python<'py>,
        arguments_json: String,
        cancellation: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let tool = self.tool.clone();
        let token = cancellation.token.clone();
        let guard = token.clone().drop_guard();
        crate::runtime::future_into_py(py, async move {
            let _guard = guard;
            let input: ShellInput = serde_json::from_str(&arguments_json)
                .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
            let result = tool
                .shell_owned(OWNER, input, &token)
                .await
                .map_err(|e| tool_error(&e))?;
            serde_json::to_string(&result)
                .map_err(|_| ToolError::new_err("Unable to serialize tool result"))
        })
    }
}
