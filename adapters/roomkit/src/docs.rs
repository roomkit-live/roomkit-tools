//! Individually selected documentation adapters over a configured backend.
use std::sync::Arc;

use roomkit::model::Metadata;
use roomkit_ai::{AiTool, Tool, ToolFuture};
use roomkit_tools_docs::{DocsListTool, DocsReadTool, DocsSearchTool};

use crate::bridge::{ai_tool, anyone, serve, tool_error};

macro_rules! toolset {
    ($name:ident, $tool:ident, $wire:literal) => {
        #[doc = concat!("Independently selected `", $wire, "` documentation adapter.")]
        #[derive(Clone, Debug)]
        pub struct $name {
            tool: Arc<$tool>,
        }
        impl $name {
            /// Use a configured facade over a host-owned backend.
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
                serve(
                    $wire,
                    name,
                    arguments,
                    anyone,
                    move |(), input, cancel| async move {
                        tool.call(input, &cancel).await.map_err(|e| tool_error(&e))
                    },
                )
            }
        }
    };
}
toolset!(DocsListToolset, DocsListTool, "docs_list");
toolset!(DocsSearchToolset, DocsSearchTool, "docs_search");
toolset!(DocsReadToolset, DocsReadTool, "docs_read");
