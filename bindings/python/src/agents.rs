//! Shared Rust contracts for Python-owned orchestration backends.

use pyo3::prelude::*;
use roomkit_tools_agents::{
    AgentContract, AgentFollowupInput, AgentFollowupTool, AgentSendInput, AgentSendTool,
    CancelResult, CancelTaskInput, CancelTaskTool, DelegateTaskInput, DelegateTaskTool, SendResult,
    TaskInfo, TaskList, TaskStatusInput, TaskStatusTool, decode, render,
};

#[derive(Debug)]
#[pyclass(name = "_AgentContract", module = "roomkit_tools._native")]
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
            "agent_followup" => encode(&AgentFollowupTool::definition()),
            "agent_send" => encode(&AgentSendTool::definition()),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn input_json(name: &str, json: &str) -> PyResult<String> {
        match name {
            "delegate_task" => input::<DelegateTaskInput>(json),
            "task_status" => input::<TaskStatusInput>(json),
            "cancel_task" => input::<CancelTaskInput>(json),
            "agent_followup" => input::<AgentFollowupInput>(json),
            "agent_send" => input::<AgentSendInput>(json),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn output_json(name: &str, json: &str) -> PyResult<String> {
        match name {
            "delegate_task" | "agent_followup" => output::<TaskInfo>(json),
            "task_status" => output::<TaskList>(json),
            "cancel_task" => output::<CancelResult>(json),
            "agent_send" => output::<SendResult>(json),
            _ => Err(unknown()),
        }
    }
}
