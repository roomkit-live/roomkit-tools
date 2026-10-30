//! Individually selected documentation adapters over a configured backend.
use roomkit::model::ToolCall;
use roomkit_ai::{ToolContext, ToolDefinition, ToolError, ToolOutput, Toolset};
use roomkit_tools_docs::{DocsListTool, DocsReadTool, DocsSearchTool};
use tokio_util::sync::CancellationToken;
macro_rules! toolset {
    ($name:ident, $tool:ident, $wire:literal) => {
        #[doc = concat!("Independently selected `", $wire, "` documentation adapter.")]
        #[derive(Debug)]
        pub struct $name {
            tool: $tool,
            closed: CancellationToken,
        }
        impl $name {
            /// Use a configured facade; closing this adapter preserves its host-owned backend.
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
                _context: ToolContext,
            ) -> Result<ToolOutput, ToolError> {
                if call.name != $wire || self.closed.is_cancelled() {
                    return Err(ToolError::refused("unavailable documentation tool"));
                }
                let input = roomkit_ai::arguments(&call)?;
                let cancel = self.closed.child_token();
                let _guard = cancel.clone().drop_guard();
                let result = self
                    .tool
                    .execute(input, &cancel)
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
toolset!(DocsListToolset, DocsListTool, "docs_list");
toolset!(DocsSearchToolset, DocsSearchTool, "docs_search");
toolset!(DocsReadToolset, DocsReadTool, "docs_read");
