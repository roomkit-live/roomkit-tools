//! Modular search delegates transport and normalization to Rust.
use super::{Cancellation, InvalidInputError, ToolError, tool_error};
use pyo3::prelude::*;
use roomkit_tools_search::{ExaSearch, OllamaSearch, SearchBackend, SearchInput, WebSearchTool};
use std::sync::Arc;

#[pyclass(name = "_WebSearchTool", frozen, module = "roomkit_tools._native")]
#[derive(Clone, Debug)]
pub(super) struct PythonWebSearchTool {
    tool: WebSearchTool,
}

#[pymethods]
impl PythonWebSearchTool {
    #[new]
    fn new(backend: &PythonSearchBackend, limits_json: &str) -> PyResult<Self> {
        let limits = serde_json::from_str(limits_json)
            .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
        let tool = WebSearchTool::new(Arc::clone(&backend.backend))
            .with_limits(limits)
            .map_err(|e| tool_error(&e))?;
        Ok(Self { tool })
    }

    #[staticmethod]
    fn definition_json() -> PyResult<String> {
        serde_json::to_string(&WebSearchTool::definition())
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
            let input: SearchInput = serde_json::from_str(&arguments_json)
                .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
            let output = tool
                .search(input, &token)
                .await
                .map_err(|e| tool_error(&e))?;
            serde_json::to_string(&output)
                .map_err(|_| ToolError::new_err("Unable to serialize web output"))
        })
    }
}

#[pyclass(name = "_SearchBackend", frozen, module = "roomkit_tools._native")]
#[derive(Clone, Debug)]
pub(super) struct PythonSearchBackend {
    backend: Arc<dyn SearchBackend>,
}

#[pymethods]
impl PythonSearchBackend {
    fn capabilities_json(&self) -> PyResult<String> {
        serde_json::to_string(&self.backend.capabilities())
            .map_err(|_| ToolError::new_err("Unable to serialize search capabilities"))
    }

    #[staticmethod]
    fn exa(api_key: Option<&str>, config_json: &str) -> PyResult<Self> {
        let config = serde_json::from_str(config_json)
            .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
        let backend = ExaSearch::new(api_key, config).map_err(|e| tool_error(&e))?;
        Ok(Self {
            backend: Arc::new(backend),
        })
    }
    #[staticmethod]
    fn ollama(api_key: &str, config_json: &str) -> PyResult<Self> {
        let config = serde_json::from_str(config_json)
            .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
        let backend = OllamaSearch::new(api_key, config).map_err(|e| tool_error(&e))?;
        Ok(Self {
            backend: Arc::new(backend),
        })
    }
}
