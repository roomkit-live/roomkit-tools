//! OCR schemas remain available without installing a recognition backend.
use pyo3::prelude::*;

#[pyfunction]
pub(super) fn ocr_definition_json() -> PyResult<String> {
    serde_json::to_string(&roomkit_tools_ocr::OcrTool::definition())
        .map_err(|_| super::ToolError::new_err("Unable to serialize OCR definition"))
}
