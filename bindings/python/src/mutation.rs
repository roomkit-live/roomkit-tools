//! Mutation bindings share the existing Rust workspace and commit boundary.

use std::path::PathBuf;
use std::sync::Arc;

use pyo3::prelude::*;
use roomkit_tools_core::Error;
use roomkit_tools_fs::{
    EditInput, EditLimits, EditTool, MutationWorkspace, PatchInput, PatchLimits, PatchTool,
    WriteInput, WriteLimits, WriteTool,
};
use serde::Deserialize;
use tokio::sync::OnceCell;

use super::{Cancellation, DiffOptions, InvalidInputError, ToolError, tool_error};

host_options!(WriteOptions, WriteLimits, max_file_bytes);
host_options!(EditOptions, EditLimits, max_file_bytes);

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[expect(
    clippy::struct_field_names,
    reason = "preserve public Rust limit names"
)]
struct PatchOptions {
    max_changes: Option<usize>,
    max_input_bytes: Option<usize>,
    max_file_bytes: Option<usize>,
    max_lines: Option<usize>,
    max_comparison_cells: Option<usize>,
    max_output_bytes: Option<usize>,
    max_context_lines: Option<usize>,
}

impl PatchOptions {
    fn limits(self) -> PatchLimits {
        let mut limits = PatchLimits::default();
        if let Some(value) = self.max_changes {
            limits.max_changes = value;
        }
        if let Some(value) = self.max_input_bytes {
            limits.max_input_bytes = value;
        }
        limits.diff = DiffOptions {
            max_file_bytes: self.max_file_bytes,
            max_lines: self.max_lines,
            max_comparison_cells: self.max_comparison_cells,
            max_output_bytes: self.max_output_bytes,
            max_context_lines: self.max_context_lines,
        }
        .limits();
        limits
    }
}

/// Lazily resolved host scope shared across tool instances and Python loops.
#[pyclass(name = "_Workspace", frozen, module = "roomkit_tools._native")]
#[derive(Clone, Debug)]
pub(super) struct PythonWorkspace {
    root: PathBuf,
    resolved: Arc<OnceCell<MutationWorkspace>>,
}

#[pymethods]
impl PythonWorkspace {
    #[new]
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            resolved: Arc::new(OnceCell::new()),
        }
    }
}

impl PythonWorkspace {
    async fn resolve(&self) -> Result<MutationWorkspace, Error> {
        // Unlike independent lazy read tools, concurrent mutation constructors
        // must resolve to the same lock. Cancelled/failed initialization can be
        // retried; no Python task or lock is bound to a particular asyncio loop.
        self.resolved
            .get_or_try_init(|| MutationWorkspace::new(&self.root))
            .await
            .cloned()
    }
}

macro_rules! mutation_binding {
    ($binding:ident, $name:literal, $tool:ty, $options:ty, $input:ty, $operation:ident) => {
        #[pyclass(name = $name, frozen, module = "roomkit_tools._native")]
        #[derive(Clone, Debug)]
        pub(super) struct $binding {
            tool: $tool,
        }

        #[pymethods]
        impl $binding {
            #[staticmethod]
            fn create<'py>(
                py: Python<'py>,
                workspace: &PythonWorkspace,
                options_json: String,
                cancellation: &Cancellation,
            ) -> PyResult<Bound<'py, PyAny>> {
                let workspace = workspace.clone();
                let token = cancellation.token.clone();
                let guard = token.clone().drop_guard();
                crate::runtime::future_into_py(py, async move {
                    let _guard = guard;
                    let options: $options = serde_json::from_str(&options_json)
                        .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
                    if token.is_cancelled() {
                        return Err(tool_error(&Error::Cancelled));
                    }
                    let workspace = workspace
                        .resolve()
                        .await
                        .map_err(|error| tool_error(&error))?;
                    let tool = <$tool>::from_workspace(workspace)
                        .with_limits(options.limits())
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
                    let input: $input = serde_json::from_str(&arguments_json)
                        .map_err(|error| InvalidInputError::new_err(error.to_string()))?;
                    // The Rust worker keeps the workspace lock even if Python
                    // drops this future. A commit already started may finish.
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

mutation_binding!(
    PythonWriteTool,
    "_WriteTool",
    WriteTool,
    WriteOptions,
    WriteInput,
    write
);
mutation_binding!(
    PythonEditTool,
    "_EditTool",
    EditTool,
    EditOptions,
    EditInput,
    edit
);
mutation_binding!(
    PythonPatchTool,
    "_PatchTool",
    PatchTool,
    PatchOptions,
    PatchInput,
    patch
);
