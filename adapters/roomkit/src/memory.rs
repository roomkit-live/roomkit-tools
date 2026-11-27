//! A host-bound project memory exposed as a RoomKit tool.
use crate::bridge::{ai_tool, anyone, serve, tool_error};
use roomkit::model::Metadata;
use roomkit_ai::{AiTool, Tool, ToolFuture};
use roomkit_tools_memory::MemoryTool;

/// Read and maintain notes in the host-selected namespace.
#[derive(Clone, Debug)]
pub struct Memory {
    tool: MemoryTool,
}
impl Memory {
    /// Use this configured memory tool.
    pub fn new(tool: MemoryTool) -> Self {
        Self { tool }
    }
}
impl Tool for Memory {
    fn definition(&self) -> AiTool {
        ai_tool(MemoryTool::definition())
    }
    fn handler(&self, name: String, arguments: Metadata) -> ToolFuture {
        let tool = self.tool.clone();
        serve(
            "memory",
            name,
            arguments,
            anyone,
            move |(), input, cancel| async move {
                tool.call(input, &cancel).await.map_err(|e| tool_error(&e))
            },
        )
    }
}
