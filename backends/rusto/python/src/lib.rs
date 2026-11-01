//! Separate optional native OCR distribution, reusing the shared Rust tool.
use pyo3::prelude::*;
use roomkit_tools_core::Error;
use roomkit_tools_ocr::{OcrEngine, OcrInput, OcrTool};
use roomkit_tools_ocr_rusto::RustoBackend;
use tokio_util::sync::CancellationToken;

// Reuse the public exception identities from the base package across extensions.
pyo3::import_exception!(roomkit_tools, ToolError);
pyo3::import_exception!(roomkit_tools, InvalidInputError);
pyo3::import_exception!(roomkit_tools, AccessRefusedError);
pyo3::import_exception!(roomkit_tools, LimitExceededError);
pyo3::import_exception!(roomkit_tools, UnsupportedResourceError);
pyo3::import_exception!(roomkit_tools, ToolIOError);
pyo3::import_exception!(roomkit_tools, ForkedProcessError);
pyo3::import_exception!(asyncio, CancelledError);

// Each extension owns its bridge shutdown boundary; no Python callbacks outlive it.
mod runtime;
// Each extension owns its runtime threads, so it also refuses work after fork().
mod fork;

fn error(error: &Error) -> PyErr {
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

#[pyclass(frozen, module = "roomkit_tools_ocr_rusto._native")]
#[derive(Clone)]
struct Cancellation {
    token: CancellationToken,
}

#[pymethods]
impl Cancellation {
    #[new]
    fn new() -> PyResult<Self> {
        // Every wrapper creates a token first: a forked child fails here.
        fork::ensure_unforked()?;
        Ok(Self {
            token: runtime::cancellation_token(),
        })
    }
    fn cancel(&self) {
        self.token.cancel();
    }
}

#[pyclass(frozen, module = "roomkit_tools_ocr_rusto._native")]
struct Engine {
    engine: OcrEngine,
}

#[pymethods]
impl Engine {
    #[new]
    fn new(config_json: &str) -> PyResult<Self> {
        let config = serde_json::from_str(config_json)
            .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
        Ok(Self {
            engine: RustoBackend::lazy(config).map_err(|e| error(&e))?,
        })
    }

    fn bind<'py>(
        &self,
        py: Python<'py>,
        root: String,
        limits_json: String,
        cancel: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let engine = self.engine.clone();
        let token = cancel.token.clone();
        let guard = token.clone().drop_guard();
        runtime::future_into_py(py, async move {
            let _guard = guard;
            let limits = serde_json::from_str(&limits_json)
                .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
            tokio::select! {
                biased;
                () = token.cancelled() => Err(CancelledError::new_err("OCR binding cancelled")),
                tool = OcrTool::new(root, engine) => Ok(Tool { tool: tool.map_err(|e| error(&e))?.with_limits(limits).map_err(|e| error(&e))? }),
            }
        })
    }
}

#[pyclass(frozen, module = "roomkit_tools_ocr_rusto._native")]
struct Tool {
    tool: OcrTool,
}

#[pymethods]
impl Tool {
    fn call<'py>(
        &self,
        py: Python<'py>,
        arguments_json: String,
        cancel: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let tool = self.tool.clone();
        let token = cancel.token.clone();
        let guard = token.clone().drop_guard();
        runtime::future_into_py(py, async move {
            let _guard = guard;
            let input: OcrInput = serde_json::from_str(&arguments_json)
                .map_err(|e| InvalidInputError::new_err(e.to_string()))?;
            let output = tool.ocr(input, &token).await.map_err(|e| error(&e))?;
            serde_json::to_string(&output)
                .map_err(|_| ToolError::new_err("Unable to serialize OCR output"))
        })
    }
}

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    runtime::install(module)?;
    module.add_class::<Cancellation>()?;
    module.add_class::<Engine>()?;
    module.add_class::<Tool>()?;
    Ok(())
}
