//! The assembled coding-tool set and the declarations the model sees.
//!
//! Mirrors the reference tests
//! `test_create_coding_tools_returns_initial_tool_set`,
//! `test_tool_definitions_expose_pi_style_prompt_metadata`,
//! `test_read_tool_schema_defines_line_controls_as_integers` and
//! `test_bash_tool_schema_requires_display_description`.

use tau_coding::tools::{bash, create_coding_tools, read};

#[test]
fn create_coding_tools_returns_the_initial_tool_set_in_order() {
    let directory = tempfile::tempdir().expect("a temporary directory");

    let tools = create_coding_tools(directory.path(), None, None);
    let names: Vec<&str> = tools.iter().map(|tool| tool.spec().name.as_str()).collect();

    assert_eq!(names, ["read", "write", "edit", "bash"]);
    let edit = tools[2].spec();
    assert!(edit.prompt_snippet.is_some());
    assert!(
        edit.prompt_guidelines[0].contains("Use edit for precise changes"),
        "{}",
        edit.prompt_guidelines[0]
    );
    assert!(
        tools[3].spec().prompt_guidelines[0].contains("present-participle description"),
        "{}",
        tools[3].spec().prompt_guidelines[0]
    );
}

#[test]
fn edit_spec_exposes_the_pi_style_prompt_metadata() {
    let spec = tau_coding::tools::edit::create_edit_tool_spec();

    assert!(
        spec.prompt_snippet
            .as_deref()
            .is_some_and(|snippet| snippet.starts_with("Make precise file edits")),
        "{:?}",
        spec.prompt_snippet
    );
    assert_eq!(spec.prompt_guidelines.len(), 4);
}

#[test]
fn read_spec_defines_line_controls_as_integers() {
    let spec = read::create_read_tool_spec();
    let properties = &spec.parameters["properties"];

    assert_eq!(properties["offset"]["type"], "integer");
    assert_eq!(properties["limit"]["type"], "integer");
}

#[test]
fn bash_spec_requires_the_command_and_its_description() {
    let spec = bash::create_bash_tool_spec();
    let description = &spec.parameters["properties"]["description"];

    assert_eq!(description["type"], "string");
    assert!(
        description["description"]
            .as_str()
            .is_some_and(|text| text.contains("present-participle summary")),
        "{description}"
    );
    assert_eq!(
        spec.parameters["required"],
        serde_json::json!(["command", "description"])
    );
}
