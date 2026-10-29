//! HTTP fetching delegates policy, conversion and cancellation to Rust.
use super::{Cancellation, InvalidInputError, ToolError, tool_error};
use pyo3::prelude::*;
use roomkit_tools_web::{WebFetchInput, WebFetchTool};

#[pyclass(name = "_WebFetchTool", frozen, module = "roomkit_tools._native")]
#[derive(Clone, Debug)]
pub(super) struct PythonWebFetchTool {
    tool: WebFetchTool,
}

#[pymethods]
impl PythonWebFetchTool {
    #[new]
    fn new(limits_json: &str, policy_json: &str) -> PyResult<Self> {
        let limits = serde_json::from_str(limits_json)
            .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
        let policy = serde_json::from_str(policy_json)
            .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
        let tool = WebFetchTool::new()
            .with_limits(limits)
            .and_then(|tool| tool.with_policy(policy))
            .map_err(|e| tool_error(&e))?;
        Ok(Self { tool })
    }

    #[staticmethod]
    fn definition_json() -> PyResult<String> {
        serde_json::to_string(&WebFetchTool::definition())
            .map_err(|_| ToolError::new_err("Unable to serialize web definition"))
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
            let input: WebFetchInput = serde_json::from_str(&arguments_json)
                .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
            let output = tool
                .fetch(input, &token)
                .await
                .map_err(|e| tool_error(&e))?;
            serde_json::to_string(&output)
                .map_err(|_| ToolError::new_err("Unable to serialize web output"))
        })
    }
}
