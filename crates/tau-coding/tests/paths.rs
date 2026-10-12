//! Where Tau keeps its durable data and its project resources.
//!
//! Mirrors tau's `tests/test_paths.py`. The reference monkeypatches `os.environ`
//! for the environment cases; these use a `MapEnv` so no process state is
//! touched, which is the same reason the production code reads through an
//! `EnvSource`.

use std::path::{Path, PathBuf};

use tau_ai::env::MapEnv;
use tau_coding::paths::{TAU_HOME_VAR, TauPaths, tau_home_from};

fn env(value: &str) -> MapEnv {
    MapEnv::new().with(TAU_HOME_VAR, value)
}

fn without_home() -> MapEnv {
    MapEnv::new()
}

#[test]
fn the_user_locations_hang_off_the_two_homes() {
    let paths = TauPaths::new("/home/tau/.tau", "/home/tau/.agents");

    assert_eq!(
        paths.sessions_dir(),
        PathBuf::from("/home/tau/.tau/sessions")
    );
    assert_eq!(
        paths.user_skills_dir(),
        PathBuf::from("/home/tau/.tau/skills")
    );
    assert_eq!(
        paths.user_prompts_dir(),
        PathBuf::from("/home/tau/.tau/prompts")
    );
    assert_eq!(
        paths.user_themes_dir(),
        PathBuf::from("/home/tau/.tau/themes")
    );
    assert_eq!(
        paths.user_extensions_dir(),
        PathBuf::from("/home/tau/.tau/extensions")
    );
    assert_eq!(
        paths.user_agents_skills_dir(),
        PathBuf::from("/home/tau/.agents/skills")
    );
    assert_eq!(
        paths.user_agents_prompts_dir(),
        PathBuf::from("/home/tau/.agents/prompts")
    );
}

#[test]
fn the_derived_store_paths_hang_off_the_home() {
    let paths = TauPaths::new("/home/tau/.tau", "/home/tau/.agents");

    assert_eq!(paths.logs_dir(), PathBuf::from("/home/tau/.tau/logs"));
    assert_eq!(
        paths.agent_calls_log_path(),
        PathBuf::from("/home/tau/.tau/logs/agent-calls.jsonl")
    );
    assert_eq!(
        paths.models_store_path(),
        PathBuf::from("/home/tau/.tau/models-store.json")
    );
    assert_eq!(
        paths.codex_version_store_path(),
        PathBuf::from("/home/tau/.tau/codex-version-store.json")
    );
    assert_eq!(
        paths.codex_models_store_path(),
        PathBuf::from("/home/tau/.tau/codex-models-store.json")
    );
    assert_eq!(
        paths.llama_cpp_state_path(),
        PathBuf::from("/home/tau/.tau/state/extensions/llama.cpp.json")
    );
}

#[test]
fn the_project_locations_hang_off_the_working_directory() {
    let paths = TauPaths::new("/home/tau/.tau", "/home/tau/.agents");
    let cwd = Path::new("/work/project");

    assert_eq!(
        paths.project_tau_dir(cwd),
        PathBuf::from("/work/project/.tau")
    );
    assert_eq!(
        paths.project_agents_dir(cwd),
        PathBuf::from("/work/project/.agents")
    );
    assert_eq!(
        paths.project_skills_dir(cwd),
        PathBuf::from("/work/project/.tau/skills")
    );
    assert_eq!(
        paths.project_prompts_dir(cwd),
        PathBuf::from("/work/project/.tau/prompts")
    );
    assert_eq!(
        paths.project_themes_dir(cwd),
        PathBuf::from("/work/project/.tau/themes")
    );
    assert_eq!(
        paths.project_agents_skills_dir(cwd),
        PathBuf::from("/work/project/.agents/skills")
    );
    assert_eq!(
        paths.project_agents_prompts_dir(cwd),
        PathBuf::from("/work/project/.agents/prompts")
    );
}

#[test]
fn an_unset_home_falls_back_to_dot_tau_in_the_user_home() {
    let user_home = Path::new("/home/tau");

    let home = tau_home_from(&without_home(), Some(user_home)).expect("the default resolves");

    assert_eq!(home, user_home.join(".tau"));
}

#[test]
fn an_empty_home_falls_back_to_the_same_default() {
    let user_home = Path::new("/home/tau");

    let home = tau_home_from(&env(""), Some(user_home)).expect("the default resolves");

    assert_eq!(home, user_home.join(".tau"));
}

#[test]
fn the_environment_value_is_read_afresh_for_each_instance() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let user_home = directory.path().to_path_buf();
    let first_home = directory.path().join("first");
    let second_home = directory.path().join("second");

    let first = TauPaths::from_environment(&env(&first_home.to_string_lossy()), Some(&user_home))
        .expect("the first home resolves");
    let second = TauPaths::from_environment(&env(&second_home.to_string_lossy()), Some(&user_home))
        .expect("the second home resolves");

    assert_eq!(first.home(), first_home);
    assert_eq!(first.sessions_dir(), first_home.join("sessions"));
    assert_eq!(second.home(), second_home);
    assert_eq!(
        second.models_store_path(),
        second_home.join("models-store.json")
    );
    assert_eq!(
        first.agents_home(),
        second.agents_home(),
        "the agents home does not follow the Tau home"
    );
}

#[test]
fn a_tilde_home_expands_against_the_user_home() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let user_home = directory.path().to_path_buf();

    let home = tau_home_from(&env("~/.tau-personal"), Some(&user_home)).expect("the tilde expands");

    assert_eq!(home, user_home.join(".tau-personal"));
}

#[test]
fn a_relative_home_is_rejected() {
    let user_home = Path::new("/home/tau");

    let error = tau_home_from(&env(".tau-personal"), Some(user_home))
        .expect_err("a relative home is not usable");

    assert_eq!(
        error.message(),
        "TAU_HOME must be an absolute path after '~' expansion"
    );
}

#[test]
fn the_default_session_path_is_readable_and_scoped_to_the_project() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let paths = TauPaths::new(
        directory.path().join("home"),
        directory.path().join("agents"),
    );
    let cwd = directory
        .path()
        .join("repos")
        .join("exploration")
        .join("tau");

    let session_path = paths
        .default_session_path(&cwd)
        .expect("the session path resolves");

    assert_eq!(session_path.file_name().unwrap(), "default.jsonl");
    assert_eq!(
        session_path.parent().unwrap().parent().unwrap(),
        paths.sessions_dir()
    );

    let name = session_path
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.contains("repos-exploration-tau-"),
        "the project is readable in the name: {name}"
    );

    let digest = name.rsplit('-').next().expect("a digest suffix");
    assert_eq!(digest.len(), 6, "the digest is six characters: {name}");
    assert!(session_path.parent().unwrap().exists());
}

#[test]
fn the_same_project_always_gets_the_same_directory() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let paths = TauPaths::new(
        directory.path().join("home"),
        directory.path().join("agents"),
    );
    let cwd = directory.path().join("work").join("project");

    let first = paths.project_session_dir(&cwd);
    let second = paths.project_session_dir(&cwd);

    assert_eq!(first, second, "the name must be stable across calls");
}

#[test]
fn two_projects_with_the_same_leaf_name_do_not_collide() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let paths = TauPaths::new(
        directory.path().join("home"),
        directory.path().join("agents"),
    );

    let first = paths.project_session_dir(&directory.path().join("one").join("project"));
    let second = paths.project_session_dir(&directory.path().join("two").join("project"));

    assert_ne!(
        first, second,
        "the digest must separate projects that share a name"
    );
}
