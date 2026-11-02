//! Plan adapters derive ownership from trusted RoomKit context.
use std::sync::Arc;

use roomkit::model::Metadata;
use roomkit_ai::{AiTool, Tool, ToolFuture};
use roomkit_tools_plans::{
    PlanCreateTool, PlanDeleteTool, PlanListTool, PlanReadTool, PlanScope, PlanUpdateTool,
};

use crate::bridge::{ai_tool, current_caller, serve, tool_error};

macro_rules! toolset {
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
                        tool.execute(&scope, input, &cancel)
                            .await
                            .map_err(|e| tool_error(&e))
                    },
                )
            }
        }
    };
}
toolset!(PlanCreateToolset, PlanCreateTool, "plan_create");
toolset!(PlanReadToolset, PlanReadTool, "plan_read");
toolset!(PlanListToolset, PlanListTool, "plan_list");
toolset!(PlanUpdateToolset, PlanUpdateTool, "plan_update");
toolset!(PlanDeleteToolset, PlanDeleteTool, "plan_delete");
