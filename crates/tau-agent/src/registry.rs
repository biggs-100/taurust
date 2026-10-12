//! The name-to-tool registry the agent loop resolves calls against.
//!
//! The reference builds a plain mapping and lets a later registration silently
//! shadow an earlier one, while the list handed to the provider keeps both
//! entries. Keeping the mapping but reporting the shadowing removes a real
//! footgun: a silent override is how a built-in tool quietly stops being the one
//! that runs. An override is still allowed, because the extensions runtime
//! relies on wrapping a tool and registering it under the same name.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use tau_protocol::ToolSpec;

use crate::tools::Tool;

/// A set of tools addressed by name, in a stable order.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
    index: HashMap<String, usize>,
}

impl ToolRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool, or replace the one already registered under its name.
    ///
    /// Returns the tool it replaced, so a caller can notice the shadowing instead
    /// of discovering it later. A replacement keeps the original position: the
    /// list sent to the provider must not be reordered by an override, because
    /// that list participates in prompt caching.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Option<Arc<dyn Tool>> {
        let name = tool.spec().name.clone();

        match self.index.get(&name).copied() {
            Some(position) => Some(std::mem::replace(&mut self.tools[position], tool)),
            None => {
                self.index.insert(name, self.tools.len());
                self.tools.push(tool);
                None
            }
        }
    }

    /// The tool registered under `name`.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.index.get(name).map(|position| &self.tools[*position])
    }

    /// The declarations of every registered tool, in registration order.
    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools.iter().map(|tool| tool.spec().clone()).collect()
    }

    /// Every registered tool, in registration order.
    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Tool>> {
        self.tools.iter()
    }

    /// How many tools are registered.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Whether no tool is registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

impl fmt::Debug for ToolRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field(
                "tools",
                &self
                    .tools
                    .iter()
                    .map(|tool| tool.spec().name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}
