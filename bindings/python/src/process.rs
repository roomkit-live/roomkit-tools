//! A Python-owned process session; process behavior remains in the Rust manager.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use pyo3::prelude::*;
use roomkit_tools_core::Error;
use roomkit_tools_process::{
    CommandEnv, DEFAULT_INHERITED_ENV, ProcessLimits, ProcessManager, ProcessReadInput,
    ProcessStartInput, ProcessStatusInput, ProcessStopInput, ProcessWriteInput,
};
use serde::Deserialize;
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

use super::{Cancellation, InvalidInputError, ToolError, fork, tool_error};

// Calls that name no owner use this one; hosts and RoomKit handlers pass a
// trusted owner. Neither the manager nor an owner is selected by model arguments.
const OWNER: &str = super::output::OWNER;

/// Add the default inherited variable names and the environment check.
pub(super) fn install(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("DEFAULT_INHERITED_ENV", DEFAULT_INHERITED_ENV.to_vec())?;
    module.add_function(wrap_pyfunction!(check_command_env, module)?)
}

/// The host-selected command environment; omitted parts keep Rust defaults.
pub(super) fn command_env(
    inherit_env: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
) -> PyResult<CommandEnv> {
    let mut command_env = CommandEnv::default();
    if let Some(names) = inherit_env {
        command_env.inherit_env = names;
    }
    if let Some(values) = env {
        command_env.env = values;
    }
    command_env.validate().map_err(|error| tool_error(&error))?;
    Ok(command_env)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvOptions {
    inherit_env: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
}

/// Validate environment options when a host tool is configured, before first use.
#[pyfunction]
#[pyo3(name = "_check_command_env")]
fn check_command_env(options_json: &str) -> PyResult<()> {
    let options: EnvOptions = serde_json::from_str(options_json)
        .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
    command_env(options.inherit_env, options.env).map(drop)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessOptions {
    max_processes: Option<usize>,
    max_processes_per_owner: Option<usize>,
    max_command_bytes: Option<usize>,
    max_output_bytes: Option<usize>,
    max_read_bytes: Option<usize>,
    max_write_bytes: Option<usize>,
    write_timeout_ms: Option<u64>,
    default_timeout_ms: Option<u64>,
    max_timeout_ms: Option<u64>,
    inherit_env: Option<Vec<String>>,
    env: Option<BTreeMap<String, String>>,
}

impl ProcessOptions {
    fn limits(self) -> ProcessLimits {
        let mut limits = ProcessLimits::default();
        macro_rules! set {
            ($($field:ident),+) => {$(if let Some(value) = self.$field {
                limits.$field = value;
            })+};
        }
        set!(
            max_processes,
            max_processes_per_owner,
            max_command_bytes,
            max_output_bytes,
            max_read_bytes,
            max_write_bytes,
            write_timeout_ms,
            default_timeout_ms,
            max_timeout_ms
        );
        limits
    }
}

#[derive(Debug)]
struct SessionState {
    root: PathBuf,
    limits: ProcessLimits,
    env: CommandEnv,
    manager: OnceCell<ProcessManager>,
    closed: CancellationToken,
    outputs: Option<Arc<super::output::State>>,
}

impl SessionState {
    async fn resolve(&self) -> Result<ProcessManager, Error> {
        if self.closed.is_cancelled() {
            return Err(Error::Refused("process session is closed".into()));
        }
        let manager = self
            .manager
            .get_or_try_init(|| async {
                let mut manager = ProcessManager::new(&self.root)
                    .await?
                    .with_limits(self.limits)?
                    .with_env(self.env.clone())?;
                if let Some(outputs) = &self.outputs {
                    manager = manager.with_output_store(outputs.resolve().await?)?;
                }
                Ok::<_, Error>(manager)
            })
            .await?;
        if self.closed.is_cancelled() {
            // Closing before/during lazy initialization must never reopen the
            // scope or allow a first process to escape its shutdown boundary.
            manager.close().await;
            return Err(Error::Refused("process session is closed".into()));
        }
        Ok(manager.clone())
    }
}

#[pyclass(name = "_ProcessSession", frozen, module = "roomkit_tools._native")]
#[derive(Debug)]
pub(super) struct PythonProcessSession {
    state: Arc<SessionState>,
}

impl Drop for PythonProcessSession {
    fn drop(&mut self) {
        // A forked child's copied runtime has no thread to run cleanup, and
        // its locks may be held by threads the child lacks.
        if fork::is_forked() {
            return;
        }
        self.state.closed.cancel();
        if let Some(manager) = self.state.manager.get().cloned() {
            // No Python calls in this worker. Prefer explicit close: interpreter
            // or host termination does not wait for asynchronous cleanup.
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move {
                manager.close().await;
            });
        }
    }
}

#[pymethods]
impl PythonProcessSession {
    #[new]
    #[pyo3(signature = (root, options_json, outputs=None))]
    fn new(
        root: PathBuf,
        options_json: &str,
        outputs: Option<&super::output::PythonOutputStore>,
    ) -> PyResult<Self> {
        let mut options: ProcessOptions = serde_json::from_str(options_json)
            .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
        let env = command_env(options.inherit_env.take(), options.env.take())?;
        Ok(Self {
            state: Arc::new(SessionState {
                root,
                limits: options.limits(),
                env,
                manager: OnceCell::new(),
                closed: CancellationToken::new(),
                outputs: outputs.map(|outputs| Arc::clone(&outputs.state)),
            }),
        })
    }

    fn is_closed(&self) -> bool {
        self.state.closed.is_cancelled()
    }

    fn initialize<'py>(
        &self,
        py: Python<'py>,
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
            state.resolve().await.map_err(|error| tool_error(&error))?;
            if token.is_cancelled() {
                return Err(tool_error(&Error::Cancelled));
            }
            Ok(())
        })
    }

    #[staticmethod]
    fn definition_json(name: &str) -> PyResult<String> {
        let definition = match name {
            "process_start" => ProcessManager::start_definition(),
            "process_status" => ProcessManager::status_definition(),
            "process_read" => ProcessManager::read_definition(),
            "process_write" => ProcessManager::write_definition(),
            "process_stop" => ProcessManager::stop_definition(),
            _ => return Err(InvalidInputError::new_err("unknown process tool")),
        };
        serde_json::to_string(&definition)
            .map_err(|_| ToolError::new_err("Unable to serialize the tool definition"))
    }

    #[pyo3(signature = (name, arguments_json, cancellation, owner=None))]
    fn call<'py>(
        &self,
        py: Python<'py>,
        name: String,
        arguments_json: String,
        cancellation: &Cancellation,
        owner: Option<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let state = Arc::clone(&self.state);
        let owner = owner.unwrap_or_else(|| OWNER.to_owned());
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
                        .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
                    let manager = state.resolve().await.map_err(|error| tool_error(&error))?;
                    let result = manager
                        .$method(&owner, input, &token)
                        .await
                        .map_err(|error| tool_error(&error))?;
                    serde_json::to_string(&result)
                        .map_err(|_| ToolError::new_err("Unable to serialize the tool result"))
                }};
            }
            match name.as_str() {
                "process_start" => invoke!(start, ProcessStartInput),
                "process_status" => invoke!(status, ProcessStatusInput),
                "process_read" => invoke!(read, ProcessReadInput),
                "process_write" => invoke!(write, ProcessWriteInput),
                "process_stop" => invoke!(stop, ProcessStopInput),
                _ => Err(InvalidInputError::new_err("unknown process tool")),
            }
        })
    }

    fn close_owner<'py>(&self, py: Python<'py>, owner: String) -> PyResult<Bound<'py, PyAny>> {
        fork::ensure_unforked()?;
        // Start the worker synchronously, as close does. A manager not yet
        // created holds no process for any owner.
        let worker = self.state.manager.get().cloned().map(|manager| {
            pyo3_async_runtimes::tokio::get_runtime()
                .spawn(async move { manager.close_owner(&owner).await })
        });
        crate::runtime::future_into_py(py, async move {
            match worker {
                Some(worker) => worker
                    .await
                    .map_err(|_| ToolError::new_err("Unable to join process owner cleanup")),
                None => Ok(true),
            }
        })
    }

    fn close<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        // A forked child refuses before it changes any state.
        fork::ensure_unforked()?;
        // Mark closed and start the worker synchronously: cancellation or never
        // awaiting the returned Python future cannot abandon shutdown.
        self.state.closed.cancel();
        let worker = self.state.manager.get().cloned().map(|manager| {
            pyo3_async_runtimes::tokio::get_runtime().spawn(async move { manager.close().await })
        });
        crate::runtime::future_into_py(py, async move {
            match worker {
                Some(worker) => worker
                    .await
                    .map_err(|_| ToolError::new_err("Unable to join process session cleanup")),
                // Any initializer still in flight checks closed before exposing
                // its manager. There can be no owned process in that case.
                None => Ok(true),
            }
        })
    }
}
