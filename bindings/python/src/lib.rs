//! Optional `CPython` bindings; all filesystem behavior stays in roomkit-tools-fs.

use std::path::PathBuf;

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    GlobInput, GlobLimits, GlobTool, GrepInput, GrepLimits, GrepTool, ListInput, ListLimits,
    ListTool, ReadInput, ReadLimits, ReadTool,
};
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

// Only supplied host bounds override Rust defaults. Validation remains in each
// tool's with_limits method; model input schemas still come from its input type.
macro_rules! host_options {
    ($options:ident, $limits:ty, $($field:ident),+ $(,)?) => {
        #[derive(Default, Deserialize)]
        #[serde(deny_unknown_fields)]
        #[expect(clippy::struct_field_names, reason = "match the Rust tool's public limit names")]
        struct $options {
            $($field: Option<usize>,)+
        }

        impl $options {
            fn limits(self) -> $limits {
                let mut limits = <$limits>::default();
                $(if let Some(value) = self.$field {
                    limits.$field = value;
                })+
                limits
            }
        }
    };
}

host_options!(
    ReadOptions,
    ReadLimits,
    max_file_bytes,
    max_output_bytes,
    max_lines
);
host_options!(
    ListOptions,
    ListLimits,
    max_entries,
    max_output_bytes,
    max_scan_entries
);
host_options!(
    GlobOptions,
    GlobLimits,
    max_matches,
    max_output_bytes,
    max_scan_entries,
    max_depth
);
host_options!(
    GrepOptions,
    GrepLimits,
    max_matches,
    max_output_bytes,
    max_scan_entries,
    max_depth,
    max_file_bytes,
    max_total_bytes,
    max_context_lines
);

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

// All read-only bindings own one configured tool and one cancellation token per
// invocation. There is no registry or Python implementation of filesystem I/O.
macro_rules! filesystem_binding {
    ($binding:ident, $name:literal, $tool:ty, $options:ty, $input:ty, $method:ident, $operation:ident) => {
        #[pyclass(name = $name, frozen, module = "roomkit_tools._native")]
        #[derive(Clone, Debug)]
        struct $binding {
            tool: $tool,
        }

        #[pymethods]
        impl $binding {
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
                    let options: $options = serde_json::from_str(&options_json)
                        .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
                    // Root resolution is async; dropping the bridge future
                    // cancels it. These tools perform no writes.
                    if token.is_cancelled() {
                        return Err(tool_error(&Error::Cancelled));
                    }
                    let tool = <$tool>::new(root)
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
                serde_json::to_string(&<$tool>::definition())
                    .map_err(|_| ToolError::new_err("Unable to serialize the tool definition"))
            }

            fn $method<'py>(
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
                    let input: $input = serde_json::from_str(&arguments_json)
                        .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
                    let output = tool
                        .$operation(input, &token)
                        .await
                        .map_err(|error| tool_error(&error))?;
                    serde_json::to_string(&output)
                        .map_err(|_| ToolError::new_err("Unable to serialize the tool result"))
                })
            }
        }
    };
}

filesystem_binding!(
    PythonReadTool,
    "_ReadTool",
    ReadTool,
    ReadOptions,
    ReadInput,
    read,
    read
);
filesystem_binding!(
    PythonListTool,
    "_ListTool",
    ListTool,
    ListOptions,
    ListInput,
    call,
    list
);
filesystem_binding!(
    PythonGlobTool,
    "_GlobTool",
    GlobTool,
    GlobOptions,
    GlobInput,
    call,
    glob
);
filesystem_binding!(
    PythonGrepTool,
    "_GrepTool",
    GrepTool,
    GrepOptions,
    GrepInput,
    call,
    grep
);

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<Cancellation>()?;
    module.add_class::<PythonReadTool>()?;
    module.add_class::<PythonListTool>()?;
    module.add_class::<PythonGlobTool>()?;
    module.add_class::<PythonGrepTool>()?;
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
