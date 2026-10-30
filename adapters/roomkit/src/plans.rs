//! Plan adapters derive ownership from trusted RoomKit context.
use roomkit::model::ToolCall;
use roomkit_ai::{ToolContext, ToolDefinition, ToolError, ToolOutput, Toolset};
use roomkit_tools_plans::{
    PlanCreateTool, PlanDeleteTool, PlanListTool, PlanReadTool, PlanScope, PlanUpdateTool,
};
use tokio_util::sync::CancellationToken;
macro_rules! toolset {
    ($name:ident, $tool:ident, $wire:literal) => {
        #[doc = concat!("Independently selected `", $wire, "` adapter with trusted owner scope.")]
        #[derive(Debug)]
        pub struct $name {
            tool: $tool,
            closed: CancellationToken,
        }
        impl $name {
            /// Use a configured facade; closing this adapter preserves its host-owned store.
            pub fn new(tool: $tool) -> Self {
                Self {
                    tool,
                    closed: CancellationToken::new(),
                }
            }
        }
        impl Toolset for $name {
            fn tools(&self) -> Vec<ToolDefinition> {
                let definition = $tool::definition();
                vec![ToolDefinition::new(
                    definition.name,
                    definition.description,
                    definition.input_schema,
                )]
            }
            async fn call(
                &self,
                call: ToolCall,
                context: ToolContext,
            ) -> Result<ToolOutput, ToolError> {
                if call.name != $wire || self.closed.is_cancelled() {
                    return Err(ToolError::refused("unavailable plan tool"));
                }
                let scope = PlanScope {
                    room_id: context.room_id.to_string(),
                    caller_id: context.channel_id.to_string(),
                };
                let input = roomkit_ai::arguments(&call)?;
                let cancel = self.closed.child_token();
                let _guard = cancel.clone().drop_guard();
                let result = self
                    .tool
                    .execute(&scope, input, &cancel)
                    .await
                    .map_err(|e| super::tool_error(&e))?;
                serde_json::to_value(result)
                    .map(ToolOutput::Json)
                    .map_err(|e| ToolError::raised(&e))
            }
            async fn close(&self) {
                self.closed.cancel();
            }
        }
    };
}
toolset!(PlanCreateToolset, PlanCreateTool, "plan_create");
toolset!(PlanReadToolset, PlanReadTool, "plan_read");
toolset!(PlanListToolset, PlanListTool, "plan_list");
toolset!(PlanUpdateToolset, PlanUpdateTool, "plan_update");
toolset!(PlanDeleteToolset, PlanDeleteTool, "plan_delete");
