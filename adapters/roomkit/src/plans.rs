//! Plan adapters derive ownership from trusted RoomKit context.
use std::sync::Arc;

use roomkit::model::Metadata;
use roomkit_ai::{AiTool, Tool, ToolFuture};
use roomkit_tools_plans::{
    PlanCreateTool, PlanDeleteTool, PlanListTool, PlanReadTool, PlanScope, PlanUpdateTool,
};

use crate::bridge::{ai_tool, current_caller, serve, tool_error};

macro_rules! adapter {
    ($name:ident, $tool:ident, $wire:literal) => {
        #[doc = concat!("Independently selected `", $wire, "` adapter with trusted owner scope.")]
        #[derive(Clone, Debug)]
        pub struct $name {
            tool: Arc<$tool>,
        }
        impl $name {
            /// Use a configured facade over a host-owned store.
            pub fn new(tool: $tool) -> Self {
                Self {
                    tool: Arc::new(tool),
                }
            }
        }
        impl Tool for $name {
            fn definition(&self) -> AiTool {
                ai_tool($tool::definition())
            }
            fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
                let tool = Arc::clone(&self.tool);
                let caller = || current_caller("plan");
                serve(
                    $wire,
                    name,
                    arguments,
                    caller,
                    move |(room_id, caller_id), input, cancel| async move {
                        let scope = PlanScope { room_id, caller_id };
                        tool.call(&scope, input, &cancel)
                            .await
                            .map_err(|e| tool_error(&e))
                    },
                )
            }
        }
    };
}
adapter!(PlanCreate, PlanCreateTool, "plan_create");
adapter!(PlanRead, PlanReadTool, "plan_read");
adapter!(PlanList, PlanListTool, "plan_list");
adapter!(PlanUpdate, PlanUpdateTool, "plan_update");
adapter!(PlanDelete, PlanDeleteTool, "plan_delete");
