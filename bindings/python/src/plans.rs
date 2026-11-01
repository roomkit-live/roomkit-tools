//! Plan contracts and a native memory backend; host callbacks stay in Python.
use super::{Cancellation, InvalidInputError, tool_error};
use pyo3::prelude::*;
use roomkit_tools_plans::{
    MemoryPlanBackend, Plan, PlanBackend, PlanContract, PlanCreateInput, PlanCreateTool,
    PlanDeleteInput, PlanDeleteResult, PlanDeleteTool, PlanListInput, PlanListResult, PlanListTool,
    PlanReadInput, PlanReadTool, PlanScope, PlanUpdateInput, PlanUpdateTool, decode, render,
};

#[derive(Debug)]
#[pyclass(name = "_PlanContract", module = "roomkit_tools._native")]
pub(crate) struct PythonPlanContract;
fn encode<T: serde::Serialize>(value: &T) -> PyResult<String> {
    serde_json::to_string(value).map_err(|e| InvalidInputError::new_err(e.to_string()))
}
fn input<T: PlanContract>(json: &str) -> PyResult<String> {
    encode(&decode::<T>(json).map_err(|e| tool_error(&e))?)
}
fn output<T: PlanContract>(json: &str) -> PyResult<String> {
    let data = decode::<T>(json).map_err(|e| tool_error(&e))?;
    encode(&render(data).map_err(|e| tool_error(&e))?)
}
fn unknown() -> PyErr {
    InvalidInputError::new_err("unknown plan tool")
}
#[pymethods]
impl PythonPlanContract {
    #[staticmethod]
    fn definition_json(name: &str) -> PyResult<String> {
        match name {
            "plan_create" => encode(&PlanCreateTool::definition()),
            "plan_read" => encode(&PlanReadTool::definition()),
            "plan_list" => encode(&PlanListTool::definition()),
            "plan_update" => encode(&PlanUpdateTool::definition()),
            "plan_delete" => encode(&PlanDeleteTool::definition()),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn input_json(name: &str, json: &str) -> PyResult<String> {
        match name {
            "plan_create" => input::<PlanCreateInput>(json),
            "plan_read" => input::<PlanReadInput>(json),
            "plan_list" => input::<PlanListInput>(json),
            "plan_update" => input::<PlanUpdateInput>(json),
            "plan_delete" => input::<PlanDeleteInput>(json),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn output_json(name: &str, json: &str) -> PyResult<String> {
        match name {
            "plan_create" | "plan_read" | "plan_update" => output::<Plan>(json),
            "plan_list" => output::<PlanListResult>(json),
            "plan_delete" => output::<PlanDeleteResult>(json),
            _ => Err(unknown()),
        }
    }
    #[staticmethod]
    fn scope(room_id: String, caller_id: String) -> PyResult<()> {
        PlanScope { room_id, caller_id }
            .validate()
            .map_err(|e| tool_error(&e))
    }
}

#[derive(Debug)]
#[pyclass(name = "_MemoryPlanBackend", frozen, module = "roomkit_tools._native")]
pub(crate) struct PythonMemoryPlanBackend {
    backend: MemoryPlanBackend,
}
#[pymethods]
impl PythonMemoryPlanBackend {
    #[new]
    fn new(max_plans: usize, max_plans_per_scope: usize) -> PyResult<Self> {
        Ok(Self {
            backend: MemoryPlanBackend::new(max_plans, max_plans_per_scope)
                .map_err(|e| tool_error(&e))?,
        })
    }
    fn call<'py>(
        &self,
        py: Python<'py>,
        name: String,
        scope: (String, String),
        arguments_json: String,
        cancellation: &Cancellation,
    ) -> PyResult<Bound<'py, PyAny>> {
        let backend = self.backend.clone();
        let scope = PlanScope {
            room_id: scope.0,
            caller_id: scope.1,
        };
        let token = cancellation.token.clone();
        let guard = token.clone().drop_guard();
        crate::runtime::future_into_py(py, async move {
            let _guard = guard;
            macro_rules! invoke {
                ($method:ident, $input:ty) => {{
                    let input = decode::<$input>(&arguments_json).map_err(|e| tool_error(&e))?;
                    let data = backend
                        .$method(&scope, &input, &token)
                        .await
                        .map_err(|e| tool_error(&e))?;
                    encode(&data)
                }};
            }
            match name.as_str() {
                "plan_create" => invoke!(create, PlanCreateInput),
                "plan_read" => invoke!(read, PlanReadInput),
                "plan_list" => invoke!(list, PlanListInput),
                "plan_update" => invoke!(update, PlanUpdateInput),
                "plan_delete" => invoke!(delete, PlanDeleteInput),
                _ => Err(unknown()),
            }
        })
    }
}
