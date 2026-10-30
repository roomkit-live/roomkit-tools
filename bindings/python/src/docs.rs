//! Rust documentation contracts and a lazy native file backend.
use super::{Cancellation, InvalidInputError, tool_error};
use pyo3::prelude::*;
use roomkit_tools_docs::{
    DocsBackend, DocsContract, DocsListInput, DocsListResult, DocsListTool, DocsReadInput,
    DocsReadResult, DocsReadTool, DocsSearchInput, DocsSearchResult, DocsSearchTool, FileDocs,
    FileDocsLimits, decode, render,
};
fn encode<T: serde::Serialize>(value: &T) -> PyResult<String> {
    serde_json::to_string(value).map_err(|e| InvalidInputError::new_err(e.to_string()))
}
fn input<T: DocsContract>(value: &str) -> PyResult<String> {
    encode(&decode::<T>(value).map_err(|e| tool_error(&e))?)
}
fn output<T: DocsContract>(value: &str) -> PyResult<String> {
    encode(&render(decode::<T>(value).map_err(|e| tool_error(&e))?).map_err(|e| tool_error(&e))?)
}
fn unknown() -> PyErr {
    InvalidInputError::new_err("unknown documentation tool")
}
#[derive(Debug)]
#[pyclass(name = "_DocsContract")]
pub(crate) struct PythonDocsContract;
#[pymethods]
impl PythonDocsContract {
    #[staticmethod]
    fn definition_json(name: &str) -> PyResult<String> {
        match name {
            "docs_list" => encode(&DocsListTool::definition()),
            "docs_search" => encode(&DocsSearchTool::definition()),
            "docs_read" => encode(&DocsReadTool::definition()),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn input_json(name: &str, json: &str) -> PyResult<String> {
        match name {
            "docs_list" => input::<DocsListInput>(json),
            "docs_search" => input::<DocsSearchInput>(json),
            "docs_read" => input::<DocsReadInput>(json),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn output_json(name: &str, json: &str) -> PyResult<String> {
        match name {
            "docs_list" => output::<DocsListResult>(json),
            "docs_search" => output::<DocsSearchResult>(json),
            "docs_read" => output::<DocsReadResult>(json),
            _ => Err(unknown()),
        }
    }
}
#[derive(Debug)]
#[pyclass(name = "_FileDocs", frozen, module = "roomkit_tools._native")]
pub(crate) struct PythonFileDocs {
    backend: FileDocs,
}
#[pymethods]
impl PythonFileDocs {
    #[new]
    fn new(root: String, version: Option<String>, limits_json: &str) -> PyResult<Self> {
        let limits: FileDocsLimits = serde_json::from_str(limits_json)
            .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
        Ok(Self {
            backend: FileDocs::new(root, version, limits).map_err(|e| tool_error(&e))?,
        })
    }
    fn call<'py>(
        &self,
        py: Python<'py>,
        name: String,
        arguments_json: String,
        cancellation: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let backend = self.backend.clone();
        let token = cancellation.token.clone();
        let guard = token.clone().drop_guard();
        crate::runtime::future_into_py(py, async move {
            let _guard = guard;
            macro_rules! invoke {
                ($method:ident,$input:ty) => {{
                    let input = decode::<$input>(&arguments_json).map_err(|e| tool_error(&e))?;
                    encode(
                        &backend
                            .$method(&input, &token)
                            .await
                            .map_err(|e| tool_error(&e))?,
                    )
                }};
            }
            match name.as_str() {
                "docs_list" => invoke!(list, DocsListInput),
                "docs_search" => invoke!(search, DocsSearchInput),
                "docs_read" => invoke!(read, DocsReadInput),
                _ => Err(unknown()),
            }
        })
    }
}
