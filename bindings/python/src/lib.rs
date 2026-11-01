//! Optional `CPython` bindings; tool behavior stays in the Rust tool crates.

use std::path::PathBuf;

use pyo3::exceptions::PyException;
use pyo3::prelude::*;
use roomkit_tools_code::{CodeOutlineInput, CodeOutlineLimits, CodeOutlineTool};
use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    DiffInput, DiffLimits, DiffTool, GlobInput, GlobLimits, GlobTool, GrepInput, GrepLimits,
    GrepTool, ListInput, ListLimits, ListTool, ReadInput, ReadLimits, ReadTool,
};
use roomkit_tools_image::{ViewImageInput, ViewImageLimits, ViewImageTool};
use roomkit_tools_process::ShellLimits;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

mod agents;
mod docs;
mod fork;
mod ocr;
mod plans;

pyo3::create_exception!(roomkit_tools, ToolError, PyException, "Base tool failure.");
pyo3::create_exception!(
    roomkit_tools,
    InvalidInputError,
    ToolError,
    "Invalid tool input."
);
pyo3::create_exception!(
    roomkit_tools,
    AccessRefusedError,
    ToolError,
    "Access was refused."
);
pyo3::create_exception!(
    roomkit_tools,
    LimitExceededError,
    ToolError,
    "A host limit was exceeded."
);
pyo3::create_exception!(
    roomkit_tools,
    UnsupportedResourceError,
    ToolError,
    "Unsupported resource."
);
pyo3::create_exception!(
    roomkit_tools,
    ToolIOError,
    ToolError,
    "An I/O operation failed."
);
pyo3::create_exception!(
    roomkit_tools,
    ForkedProcessError,
    ToolError,
    "Native tools are unusable in a process forked after its parent's first native call."
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
        #[allow(clippy::struct_field_names, reason = "preserve public Rust limit names, also for single-field options")]
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
    ViewImageOptions,
    ViewImageLimits,
    max_file_bytes,
    max_dimension,
    max_pixels,
    max_decode_bytes,
    max_edge,
    max_image_bytes,
    default_timeout_ms,
    max_timeout_ms
);
host_options!(
    CodeOutlineOptions,
    CodeOutlineLimits,
    max_file_bytes,
    max_output_bytes,
    max_symbols,
    max_scan_symbols,
    max_scan_nodes,
    max_depth,
    max_signature_bytes,
    default_timeout_ms,
    max_timeout_ms
);
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

host_options!(
    DiffOptions,
    DiffLimits,
    max_file_bytes,
    max_lines,
    max_comparison_cells,
    max_output_bytes,
    max_context_lines
);

mod mutation;
mod output;
mod process;
mod runtime;
mod search;
mod web;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShellOptions {
    max_command_bytes: Option<usize>,
    max_output_bytes: Option<usize>,
    default_timeout_ms: Option<u64>,
    max_timeout_ms: Option<u64>,
    inherit_env: Option<Vec<String>>,
    env: Option<std::collections::BTreeMap<String, String>>,
}

impl ShellOptions {
    fn limits(self) -> ShellLimits {
        let mut limits = ShellLimits::default();
        if let Some(value) = self.max_command_bytes {
            limits.max_command_bytes = value;
        }
        if let Some(value) = self.max_output_bytes {
            limits.max_output_bytes = value;
        }
        if let Some(value) = self.default_timeout_ms {
            limits.default_timeout_ms = value;
        }
        if let Some(value) = self.max_timeout_ms {
            limits.max_timeout_ms = value;
        }
        limits
    }
}

#[pyclass(name = "_Cancellation", frozen, module = "roomkit_tools._native")]
#[derive(Clone, Debug)]
struct Cancellation {
    token: CancellationToken,
}

impl Default for Cancellation {
    fn default() -> Self {
        Self {
            token: runtime::cancellation_token(),
        }
    }
}

#[pymethods]
impl Cancellation {
    #[new]
    fn new() -> PyResult<Self> {
        // Every wrapper creates a token first: a forked child fails here.
        fork::ensure_unforked()?;
        Ok(Self::default())
    }

    fn cancel(&self) {
        self.token.cancel();
    }

    fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

// Root-configured bindings own one tool and one cancellation token per call.
// Execution and cleanup remain in Rust; there is no global tool registry.
macro_rules! rooted_binding {
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
                crate::runtime::future_into_py(py, async move {
                    let _guard = guard;
                    let options: $options = serde_json::from_str(&options_json)
                        .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
                    // Root resolution is async; dropping the bridge future
                    // cancels it. Construction does not execute a tool call.
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
                crate::runtime::future_into_py(py, async move {
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

rooted_binding!(
    PythonViewImageTool,
    "_ViewImageTool",
    ViewImageTool,
    ViewImageOptions,
    ViewImageInput,
    call,
    view
);
rooted_binding!(
    PythonCodeOutlineTool,
    "_CodeOutlineTool",
    CodeOutlineTool,
    CodeOutlineOptions,
    CodeOutlineInput,
    call,
    outline
);
rooted_binding!(
    PythonReadTool,
    "_ReadTool",
    ReadTool,
    ReadOptions,
    ReadInput,
    read,
    read
);
rooted_binding!(
    PythonListTool,
    "_ListTool",
    ListTool,
    ListOptions,
    ListInput,
    call,
    list
);
rooted_binding!(
    PythonGlobTool,
    "_GlobTool",
    GlobTool,
    GlobOptions,
    GlobInput,
    call,
    glob
);
rooted_binding!(
    PythonGrepTool,
    "_GrepTool",
    GrepTool,
    GrepOptions,
    GrepInput,
    call,
    grep
);

rooted_binding!(
    PythonDiffTool,
    "_DiffTool",
    DiffTool,
    DiffOptions,
    DiffInput,
    call,
    diff
);

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<agents::PythonAgentContract>()?;
    module.add_class::<docs::PythonDocsContract>()?;
    module.add_class::<docs::PythonFileDocs>()?;
    module.add_class::<plans::PythonPlanContract>()?;
    module.add_class::<plans::PythonMemoryPlanBackend>()?;
    runtime::install(module)?;
    fork::install(module)?;
    module.add_function(wrap_pyfunction!(ocr::ocr_definition_json, module)?)?;
    module.add_class::<Cancellation>()?;
    module.add_class::<PythonCodeOutlineTool>()?;
    module.add_class::<PythonViewImageTool>()?;
    module.add_class::<web::PythonWebFetchTool>()?;
    module.add_class::<search::PythonSearchBackend>()?;
    module.add_class::<search::PythonWebSearchTool>()?;
    module.add_class::<PythonReadTool>()?;
    module.add_class::<PythonListTool>()?;
    module.add_class::<PythonGlobTool>()?;
    module.add_class::<PythonGrepTool>()?;
    module.add_class::<PythonDiffTool>()?;
    module.add_class::<output::PythonShellTool>()?;
    module.add_class::<output::PythonOutputStore>()?;
    module.add_class::<process::PythonProcessSession>()?;
    process::install(module)?;
    module.add_class::<mutation::PythonWorkspace>()?;
    module.add_class::<mutation::PythonWriteTool>()?;
    module.add_class::<mutation::PythonEditTool>()?;
    module.add_class::<mutation::PythonPatchTool>()?;
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
    module.add("ForkedProcessError", py.get_type::<ForkedProcessError>())?;
    Ok(())
}
