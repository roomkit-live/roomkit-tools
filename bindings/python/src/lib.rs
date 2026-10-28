//! Optional `CPython` bindings; all filesystem behavior stays in roomkit-tools-fs.

use std::path::PathBuf;

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use roomkit_tools_core::Error;
use roomkit_tools_fs::{ReadInput, ReadLimits, ReadTool};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

pyo3::create_exception!(
    "roomkit_tools",
    ToolError,
    PyException,
    "Base tool failure."
);
pyo3::create_exception!(
    "roomkit_tools",
    InvalidInputError,
    ToolError,
    "Invalid tool input."
);
pyo3::create_exception!(
    "roomkit_tools",
    AccessRefusedError,
    ToolError,
    "Access was refused."
);
pyo3::create_exception!(
    "roomkit_tools",
    LimitExceededError,
    ToolError,
    "A host limit was exceeded."
);
pyo3::create_exception!(
    "roomkit_tools",
    UnsupportedResourceError,
    ToolError,
    "Unsupported resource."
);
pyo3::create_exception!(
    "roomkit_tools",
    ToolIOError,
    ToolError,
    "An I/O operation failed."
);
pyo3::import_exception!(asyncio, CancelledError);

fn tool_error(error: &Error) -> PyErr {
    let message = error.to_string();
    match error {
        Error::InvalidInput(_) => InvalidInputError::new_err(message),
        Error::Refused(_) => AccessRefusedError::new_err(message),
        Error::LimitExceeded(_) => LimitExceededError::new_err(message),
        Error::Unsupported(_) => UnsupportedResourceError::new_err(message),
        Error::Cancelled => CancelledError::new_err(message),
        Error::Io { .. } => ToolIOError::new_err(message),
        _ => ToolError::new_err(message),
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "match the Rust tool's public limit names"
)]
struct ReadOptions {
    max_file_bytes: Option<usize>,
    max_output_bytes: Option<usize>,
    max_lines: Option<usize>,
}

impl ReadOptions {
    fn limits(self) -> ReadLimits {
        let mut limits = ReadLimits::default();
        if let Some(value) = self.max_file_bytes {
            limits.max_file_bytes = value;
        }
        if let Some(value) = self.max_output_bytes {
            limits.max_output_bytes = value;
        }
        if let Some(value) = self.max_lines {
            limits.max_lines = value;
        }
        limits
    }
}

#[pyclass(name = "_Cancellation", frozen, module = "roomkit_tools._native")]
#[derive(Clone, Debug, Default)]
struct Cancellation {
    token: CancellationToken,
}

#[pymethods]
impl Cancellation {
    #[new]
    fn new() -> Self {
        Self::default()
    }

    fn cancel(&self) {
        self.token.cancel();
    }

    fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

#[pyclass(name = "_ReadTool", frozen, module = "roomkit_tools._native")]
#[derive(Clone, Debug)]
struct PythonReadTool {
    tool: ReadTool,
}

#[pymethods]
impl PythonReadTool {
    #[staticmethod]
    fn create<'py>(
        py: Python<'py>,
        root: PathBuf,
        options_json: String,
        cancellation: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let token = cancellation.token.clone();
        let guard = token.clone().drop_guard();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let _guard = guard;
            let options: ReadOptions = serde_json::from_str(&options_json)
                .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
            // The runtime bridge drops this future on Python cancellation.
            // Root canonicalization is async and performs no writes.
            if token.is_cancelled() {
                return Err(tool_error(&Error::Cancelled));
            }
            let tool = ReadTool::new(root)
                .await
                .and_then(|tool| tool.with_limits(options.limits()))
                .map_err(|error| tool_error(&error))?;
            if token.is_cancelled() {
                return Err(tool_error(&Error::Cancelled));
            }
            Ok(Self { tool })
        })
    }

    #[staticmethod]
    fn definition_json() -> PyResult<String> {
        serde_json::to_string(&ReadTool::definition())
            .map_err(|_| ToolError::new_err("Unable to serialize the read definition"))
    }

    fn read<'py>(
        &self,
        py: Python<'py>,
        arguments_json: String,
        cancellation: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let tool = self.tool.clone();
        let token = cancellation.token.clone();
        let guard = token.clone().drop_guard();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let _guard = guard;
            let input: ReadInput = serde_json::from_str(&arguments_json)
                .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
            let output = tool
                .read(input, &token)
                .await
                .map_err(|error| tool_error(&error))?;
            serde_json::to_string(&output)
                .map_err(|_| ToolError::new_err("Unable to serialize the read result"))
        })
    }
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Cancellation>()?;
    module.add_class::<PythonReadTool>()?;
    let py = module.py();
    module.add("ToolError", py.get_type::<ToolError>())?;
    module.add("InvalidInputError", py.get_type::<InvalidInputError>())?;
    module.add("AccessRefusedError", py.get_type::<AccessRefusedError>())?;
    module.add("LimitExceededError", py.get_type::<LimitExceededError>())?;
    module.add(
        "UnsupportedResourceError",
        py.get_type::<UnsupportedResourceError>(),
    )?;
    module.add("ToolIOError", py.get_type::<ToolIOError>())?;
    Ok(())
}
