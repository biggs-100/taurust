//! The tool registry.
//!
//! The reference builds a plain name-to-tool mapping, where a later registration
//! silently shadows an earlier one while the list handed to the provider keeps
//! both entries. The registry here keeps the mapping and reports the shadowing,
//! because a silent override is how a built-in tool quietly stops being the one
//! that runs.

use std::sync::Arc;

use serde_json::json;
use tau_agent::async_trait;
use tau_agent::registry::ToolRegistry;
use tau_agent::tools::{Tool, ToolError, ToolUpdates};
use tau_protocol::{AgentToolResult, JsonObject, ToolSpec};

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: name.to_string(),
        label: name.to_string(),
        description: description.to_string(),
        parameters: json!({ "type": "object" }),
        prompt_snippet: None,
        prompt_guidelines: Vec::new(),
    }
}

struct NamedTool {
    spec: ToolSpec,
}

#[async_trait]
impl Tool for NamedTool {
    fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    async fn execute(
        &self,
        _call_id: &str,
        _arguments: &JsonObject,
        _updates: &ToolUpdates,
    ) -> Result<AgentToolResult, ToolError> {
        Ok(AgentToolResult::from("ok"))
    }
}

fn tool(name: &str, description: &str) -> Arc<dyn Tool> {
    Arc::new(NamedTool {
        spec: spec(name, description),
    })
}

fn names(registry: &ToolRegistry) -> Vec<String> {
    registry.specs().into_iter().map(|spec| spec.name).collect()
}

#[test]
fn an_empty_registry_holds_nothing() {
    let registry = ToolRegistry::new();

    assert!(registry.is_empty());
    assert!(registry.get("echo").is_none());
    assert!(names(&registry).is_empty());
}

#[test]
fn a_registered_tool_is_found_by_name() {
    let mut registry = ToolRegistry::new();
    registry.register(tool("echo", "echo tool"));

    let found = registry.get("echo").expect("echo is registered");

    assert_eq!(found.spec().description, "echo tool");
    assert_eq!(registry.len(), 1);
}

#[test]
fn an_unknown_name_is_not_found() {
    let mut registry = ToolRegistry::new();
    registry.register(tool("echo", "echo tool"));

    assert!(registry.get("read").is_none());
}

#[test]
fn the_specifications_keep_the_registration_order() {
    let mut registry = ToolRegistry::new();
    registry.register(tool("read", "read a file"));
    registry.register(tool("echo", "echo text"));
    registry.register(tool("bash", "run a command"));

    assert_eq!(names(&registry), vec!["read", "echo", "bash"]);
}

#[test]
fn the_first_registration_is_not_a_replacement() {
    let mut registry = ToolRegistry::new();

    assert!(registry.register(tool("echo", "echo tool")).is_none());
}

#[test]
fn re_registering_a_name_reports_what_it_replaced() {
    let mut registry = ToolRegistry::new();
    let original = tool("echo", "original");
    registry.register(Arc::clone(&original));

    let previous = registry
        .register(tool("echo", "replacement"))
        .expect("re-registering must report the tool it replaced");

    assert!(Arc::ptr_eq(&previous, &original));
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry.get("echo").unwrap().spec().description,
        "replacement"
    );
}

#[test]
fn a_replacement_keeps_the_original_position() {
    let mut registry = ToolRegistry::new();
    registry.register(tool("read", "read a file"));
    registry.register(tool("echo", "original"));
    registry.register(tool("bash", "run a command"));

    registry.register(tool("echo", "replacement"));

    assert_eq!(
        names(&registry),
        vec!["read", "echo", "bash"],
        "an override must not reorder the list sent to the provider"
    );
}
