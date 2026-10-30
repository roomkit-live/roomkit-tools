//! Shared Rust contracts for Python-owned orchestration backends.

use pyo3::prelude::*;
use roomkit_tools_agents::{
    AgentContract, CancelInput, CancelResult, CancelTaskTool, DelegateInput, DelegateTaskTool,
    StatusInput, TaskInfo, TaskList, TaskStatusTool, decode, render,
};

#[derive(Debug)]
#[pyclass(name = "_AgentContract")]
pub(crate) struct PythonAgentContract;

fn encode<T: serde::Serialize>(value: &T) -> PyResult<String> {
    serde_json::to_string(value).map_err(|e| crate::InvalidInputError::new_err(e.to_string()))
}
fn input<T: AgentContract>(json: &str) -> PyResult<String> {
    encode(&decode::<T>(json).map_err(|e| crate::tool_error(&e))?)
}
fn output<T: AgentContract>(json: &str) -> PyResult<String> {
    let data = decode::<T>(json).map_err(|e| crate::tool_error(&e))?;
    encode(&render(data).map_err(|e| crate::tool_error(&e))?)
}
fn unknown() -> PyErr {
    crate::InvalidInputError::new_err("unknown agent tool")
}

#[pymethods]
impl PythonAgentContract {
    #[staticmethod]
    fn definition_json(name: &str) -> PyResult<String> {
        match name {
            "delegate_task" => encode(&DelegateTaskTool::definition()),
            "task_status" => encode(&TaskStatusTool::definition()),
            "cancel_task" => encode(&CancelTaskTool::definition()),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn input_json(name: &str, json: &str) -> PyResult<String> {
        match name {
            "delegate_task" => input::<DelegateInput>(json),
            "task_status" => input::<StatusInput>(json),
            "cancel_task" => input::<CancelInput>(json),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn output_json(name: &str, json: &str) -> PyResult<String> {
        match name {
            "delegate_task" => output::<TaskInfo>(json),
            "task_status" => output::<TaskList>(json),
            "cancel_task" => output::<CancelResult>(json),
            _ => Err(unknown()),
        }
    }
}
