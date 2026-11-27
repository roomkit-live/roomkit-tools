//! Python access to the native memory contracts and Markdown store.
use super::{Cancellation, InvalidInputError, tool_error};
use pyo3::prelude::*;
use roomkit_tools_memory::{FileMemory, MemoryBackend, MemoryInput, MemoryResult, MemoryTool};

fn encode<T: serde::Serialize>(value: &T) -> PyResult<String> {
    serde_json::to_string(value).map_err(|e| InvalidInputError::new_err(e.to_string()))
}
fn decode(input: &str) -> PyResult<MemoryInput> {
    let args: MemoryInput =
        serde_json::from_str(input).map_err(|e| InvalidInputError::new_err(e.to_string()))?;
    MemoryTool::validate_arguments(&args).map_err(|e| tool_error(&e))?;
    Ok(args)
}

#[derive(Debug)]
#[pyclass(module = "roomkit_tools._native", name = "_MemoryContract")]
pub(crate) struct Contract;
#[pymethods]
impl Contract {
    #[staticmethod]
    fn definition() -> PyResult<String> {
        encode(&MemoryTool::definition())
    }
    #[staticmethod]
    fn input(value: &str) -> PyResult<String> {
        encode(&decode(value)?)
    }
    #[staticmethod]
    fn output(arguments: &str, value: &str) -> PyResult<String> {
        let result: MemoryResult =
            serde_json::from_str(value).map_err(|e| InvalidInputError::new_err(e.to_string()))?;
        MemoryTool::validate_result(&decode(arguments)?, &result).map_err(|e| tool_error(&e))?;
        let text = encode(&result)?;
        encode(&roomkit_tools_core::Output::new(result, text))
    }
}

#[derive(Debug)]
#[pyclass(module = "roomkit_tools._native", name = "_FileMemory")]
pub(crate) struct Files {
    backend: FileMemory,
}
#[pymethods]
impl Files {
    #[new]
    fn new(root: String, max_bytes: usize) -> Self {
        Self {
            backend: FileMemory::new(root.into(), max_bytes),
        }
    }
    fn execute<'py>(
        &self,
        py: Python<'py>,
        arguments: &str,
        cancellation: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let input = decode(arguments)?;
        let store = self.backend.clone();
        let token = cancellation.token.clone();
        let guard = token.clone().drop_guard();
        crate::runtime::future_into_py(py, async move {
            let _guard = guard;
            encode(
                &store
                    .execute(&input, &token)
                    .await
                    .map_err(|e| tool_error(&e))?,
            )
        })
    }
}
