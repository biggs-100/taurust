//! Where Tau keeps its provider credentials.
//!
//! Mirrors tau's `tests/test_credentials.py`. The reference monkeypatches
//! `os.environ` to move `TAU_HOME` between two stores; these resolve the same
//! two homes through a `MapEnv`, which is the reason the production code reads
//! through an `EnvSource` at all.
//!
//! The reference returns `str | None` from a read and lets `OSError` and
//! `json.JSONDecodeError` escape; here every read and write reports a
//! `CredentialStoreError`, so the tests unwrap it explicitly.

use std::collections::BTreeMap;
use std::path::Path;

use tau_ai::env::MapEnv;
use tau_coding::credentials::{FileCredentialStore, OAuthCredential, credentials_path};
use tau_coding::paths::{TAU_HOME_VAR, TauPaths};

fn oauth(access: &str, refresh: &str, expires: i64, account_id: Option<&str>) -> OAuthCredential {
    OAuthCredential {
        access: access.to_string(),
        refresh: refresh.to_string(),
        expires,
        account_id: account_id.map(str::to_string),
        metadata: BTreeMap::new(),
    }
}

fn store_at(directory: &Path) -> (FileCredentialStore, std::path::PathBuf) {
    let path = directory.join("credentials.json");
    (FileCredentialStore::new(&path), path)
}

#[test]
fn the_store_round_trips_a_key_and_sets_private_permissions() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let (store, path) = store_at(directory.path());
    #[cfg(not(unix))]
    let _ = &path;

    store.set("openai", "test-key").expect("the key is stored");

    assert_eq!(
        store
            .get("openai")
            .expect("the store is readable")
            .as_deref(),
        Some("test-key")
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = path
            .metadata()
            .expect("the file exists")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "credentials are readable by the user only"
        );
    }
}

#[test]
fn the_store_deletes_a_key() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let (store, _) = store_at(directory.path());
    store.set("openai", "test-key").expect("the key is stored");

    store.delete("openai").expect("the key is deleted");

    assert_eq!(store.get("openai").expect("the store is readable"), None);
}

#[test]
fn the_store_rejects_an_empty_value() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let (store, _) = store_at(directory.path());

    let error = store
        .set("openai", "  ")
        .expect_err("an empty value is not usable");

    assert_eq!(error.message(), "Credential value must not be empty");
}

#[test]
fn the_store_rejects_an_empty_name() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let (store, _) = store_at(directory.path());

    let error = store
        .set("", "test-key")
        .expect_err("an empty name is not usable");

    assert_eq!(error.message(), "Credential name must not be empty");
}

#[test]
fn the_store_round_trips_oauth_credentials() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let (store, path) = store_at(directory.path());
    let credential = oauth("access-token", "refresh-token", 123456, Some("account-1"));

    store
        .set_oauth("openai-codex", &credential)
        .expect("the credential is stored");

    assert_eq!(
        store.get("openai-codex").expect("the store is readable"),
        None,
        "an API-key lookup does not read an OAuth credential"
    );
    assert_eq!(
        store
            .get_oauth("openai-codex")
            .expect("the store is readable"),
        Some(credential)
    );

    let text = std::fs::read_to_string(&path).expect("the file is readable");
    assert!(
        text.contains("\"type\": \"oauth\""),
        "the type is written: {text}"
    );
}

#[test]
fn a_tau_home_isolates_the_default_credential_stores() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let personal_home = directory.path().join(".tau-personal");
    let work_home = directory.path().join(".tau-work");
    let user_home = directory.path();
    let personal = oauth("personal", "personal-refresh", 123456, None);
    let work = oauth("work", "work-refresh", 123456, None);

    let personal_paths = TauPaths::from_environment(
        &MapEnv::new().with(TAU_HOME_VAR, &personal_home.to_string_lossy()),
        Some(user_home),
    )
    .expect("the personal home resolves");
    FileCredentialStore::new(credentials_path(&personal_paths))
        .set_oauth("anthropic", &personal)
        .expect("the personal credential is stored");

    let work_paths = TauPaths::from_environment(
        &MapEnv::new().with(TAU_HOME_VAR, &work_home.to_string_lossy()),
        Some(user_home),
    )
    .expect("the work home resolves");
    FileCredentialStore::new(credentials_path(&work_paths))
        .set_oauth("anthropic", &work)
        .expect("the work credential is stored");

    assert_eq!(
        FileCredentialStore::new(credentials_path(&work_paths))
            .get_oauth("anthropic")
            .expect("the store is readable"),
        Some(work)
    );
    assert_eq!(
        FileCredentialStore::new(credentials_path(&personal_paths))
            .get_oauth("anthropic")
            .expect("the store is readable"),
        Some(personal)
    );
    assert!(
        !directory
            .path()
            .join(".tau")
            .join("credentials.json")
            .exists(),
        "neither store falls back to the default home"
    );
}

#[test]
fn the_store_round_trips_extensible_oauth_metadata() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let (store, path) = store_at(directory.path());
    let credential = OAuthCredential {
        access: "copilot-access".to_string(),
        refresh: "github-token".to_string(),
        expires: 123456,
        account_id: None,
        metadata: BTreeMap::from([
            (
                "enterprise_domain".to_string(),
                serde_json::json!("ghe.example.com"),
            ),
            (
                "available_model_ids".to_string(),
                serde_json::json!(["gpt-5.4", "claude-sonnet-4.6"]),
            ),
        ]),
    };

    store
        .set_oauth("github-copilot", &credential)
        .expect("the credential is stored");

    assert_eq!(
        store
            .get_oauth("github-copilot")
            .expect("the store is readable"),
        Some(credential)
    );

    let text = std::fs::read_to_string(&path).expect("the file is readable");
    assert!(
        !text.contains("\"account_id\""),
        "an absent account id is not written: {text}"
    );

    let leftovers: Vec<String> = std::fs::read_dir(directory.path())
        .expect("the directory is readable")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".credentials.json."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "the temporary file is cleaned up: {leftovers:?}"
    );
}

#[test]
fn the_store_loads_the_legacy_codex_oauth_shape() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let path = directory.path().join("credentials.json");
    std::fs::write(
        &path,
        "{\"openai-codex\":{\"type\":\"oauth\",\"access\":\"a\",\"refresh\":\"r\",\
         \"expires\":123,\"account_id\":\"account\"}}",
    )
    .expect("the legacy file is written");

    let credential = FileCredentialStore::new(&path)
        .get_oauth("openai-codex")
        .expect("the legacy credential loads")
        .expect("the credential is present");

    assert_eq!(credential, oauth("a", "r", 123, Some("account")));
}

#[test]
fn the_store_reports_names_without_values() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let (store, _) = store_at(directory.path());

    store
        .set_api_key("openai", "one")
        .expect("the key is stored");
    store
        .set_api_key("openrouter", "two")
        .expect("the key is stored");
    store
        .set_oauth("openai-codex", &oauth("a", "r", 123, None))
        .expect("the credential is stored");

    assert_eq!(
        store.names(None).expect("the store is readable"),
        vec!["openai", "openai-codex", "openrouter"],
        "names are sorted and carry no values"
    );
    assert_eq!(
        store.names(Some("openai")).expect("the store is readable"),
        vec!["openai", "openai-codex"]
    );
    assert_eq!(
        store
            .names(Some("anthropic"))
            .expect("the store is readable"),
        Vec::<String>::new()
    );
}

#[test]
fn a_corrupt_file_is_reported_as_a_credential_store_error() {
    let directory = tempfile::TempDir::new().expect("a temporary directory");
    let path = directory.path().join("credentials.json");
    std::fs::write(&path, "not json").expect("the file is written");

    let error = FileCredentialStore::new(&path)
        .get("openai")
        .expect_err("corrupt credentials are not silently ignored");

    assert!(!error.message().is_empty());
}
