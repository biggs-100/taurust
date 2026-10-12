//! Session storage: durability, ordering and crash recovery.
//!
//! Mirrors tau's `tests/test_session.py` for the storage-level facts.

use std::path::PathBuf;

use tau_agent::session::entries::{EntryBody, SessionEntry};
use tau_agent::session::storage::{InMemorySessionStorage, JsonlSessionStorage, SessionStorage};
use tau_protocol::message::UserMessage;
use tempfile::TempDir;

fn user_message(text: &str, id: &str) -> SessionEntry {
    SessionEntry::explicit(
        id,
        1.0,
        EntryBody::Message {
            message: UserMessage {
                content: text.into(),
                timestamp: 2,
                ..UserMessage::default()
            }
            .into(),
        },
    )
}

/// A store rooted in a fresh directory that is removed when it is dropped.
fn store() -> (TempDir, PathBuf, JsonlSessionStorage) {
    let directory = TempDir::new().expect("a temporary directory");
    let path = directory.path().join("session.jsonl");
    let storage = JsonlSessionStorage::new(path.clone());
    (directory, path, storage)
}

fn texts(entries: &[SessionEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| entry.message().unwrap().text())
        .collect()
}

#[test]
fn a_missing_file_reads_as_an_empty_session() {
    let (_directory, path, storage) = store();

    assert!(storage.read_all().expect("an empty read").is_empty());
    assert!(!path.exists(), "reading must not create the transcript");
}

#[test]
fn an_append_is_read_back() {
    let (_directory, _path, storage) = store();

    storage
        .append(&user_message("first", "e1"))
        .expect("the append succeeds");

    let entries = storage.read_all().expect("the read succeeds");
    assert_eq!(texts(&entries), vec!["first"]);
    assert_eq!(entries[0].id, "e1");
    assert_eq!(entries[0].kind(), "message");
}

#[test]
fn appends_accumulate_in_order() {
    let (_directory, _path, storage) = store();

    for (index, text) in ["first", "second", "third"].iter().enumerate() {
        storage
            .append(&user_message(text, &format!("e{index}")))
            .expect("the append succeeds");
    }

    assert_eq!(
        texts(&storage.read_all().expect("the read succeeds")),
        vec!["first", "second", "third"]
    );
}

#[test]
fn a_batch_is_appended_after_the_existing_content() {
    let (_directory, _path, storage) = store();
    storage
        .append(&user_message("first", "e1"))
        .expect("the first append succeeds");

    storage
        .append_batch(&[user_message("second", "e2"), user_message("third", "e3")])
        .expect("the batch succeeds");

    assert_eq!(
        texts(&storage.read_all().expect("the read succeeds")),
        vec!["first", "second", "third"]
    );
}

#[test]
fn an_empty_batch_is_a_no_op() {
    let (_directory, path, storage) = store();

    storage.append_batch(&[]).expect("an empty batch succeeds");

    assert!(
        !path.exists(),
        "an empty batch must not create the transcript"
    );
    assert!(storage.read_all().expect("the read succeeds").is_empty());
}

#[test]
fn two_stores_on_the_same_path_see_each_others_entries() {
    let (_directory, path, first) = store();
    let second = JsonlSessionStorage::new(path.clone());

    first
        .append(&user_message("from the first", "e1"))
        .expect("the first append succeeds");
    second
        .append(&user_message("from the second", "e2"))
        .expect("the second append succeeds");

    assert_eq!(
        texts(&first.read_all().expect("the read succeeds")),
        vec!["from the first", "from the second"]
    );
}

#[test]
fn the_transcript_holds_one_line_per_entry() {
    let (_directory, path, storage) = store();

    storage
        .append(&user_message("first", "e1"))
        .expect("the first append succeeds");
    storage
        .append(&user_message("second", "e2"))
        .expect("the second append succeeds");

    let contents = std::fs::read_to_string(&path).expect("the transcript is readable");
    let lines: Vec<&str> = contents.lines().collect();

    assert_eq!(lines.len(), 2);
    assert!(contents.ends_with('\n'), "every entry ends its own line");
    assert!(lines[0].contains("\"e1\""));
    assert!(lines[1].contains("\"e2\""));
}

#[test]
fn a_stale_fixed_temp_file_is_discarded_on_read() {
    let (_directory, path, storage) = store();
    storage
        .append(&user_message("committed", "e1"))
        .expect("the append succeeds");

    let temp = path.with_file_name(".session.jsonl.tmp");
    std::fs::write(&temp, "not a committed entry").expect("the temp file is written");

    let entries = storage.read_all().expect("the read succeeds");

    assert_eq!(texts(&entries), vec!["committed"]);
    assert!(!temp.exists(), "the stale temp file is removed");
}

#[test]
fn a_stale_unique_temp_file_is_discarded_on_write() {
    let (_directory, path, storage) = store();
    storage
        .append(&user_message("committed", "e1"))
        .expect("the append succeeds");

    let unique = path.with_file_name(".session.jsonl.1234-5678-0.tmp");
    std::fs::write(&unique, "half a batch").expect("the temp file is written");

    storage
        .append(&user_message("appended", "e2"))
        .expect("the append succeeds");

    assert!(!unique.exists(), "a crash artifact is cleaned up");
    assert_eq!(
        texts(&storage.read_all().expect("the read succeeds")),
        vec!["committed", "appended"]
    );
}

#[test]
fn a_session_lock_file_is_created_beside_the_transcript() {
    let (_directory, path, storage) = store();

    storage
        .append(&user_message("first", "e1"))
        .expect("the append succeeds");

    assert!(path.with_file_name(".session.jsonl.lock").exists());
}

#[test]
fn a_malformed_line_fails_the_read_with_its_line_number() {
    let (_directory, path, _storage) = store();
    std::fs::write(&path, "{\"type\":\"message\"}\n").expect("the transcript is written");

    let error = JsonlSessionStorage::new(path)
        .read_all()
        .expect_err("a malformed entry must fail the read");

    assert!(
        error
            .to_string()
            .starts_with("Invalid session entry on line 1: "),
        "unexpected message: {error}"
    );
}

#[test]
fn a_torn_final_line_is_ignored_and_repaired() {
    let (_directory, path, storage) = store();
    storage
        .append(&user_message("committed", "e1"))
        .expect("the append succeeds");

    // Simulate a crash partway through a write: a partial line, no newline.
    let mut contents = std::fs::read_to_string(&path).expect("the transcript is readable");
    contents.push_str("{\"id\":\"e2\",\"type\":\"mess");
    std::fs::write(&path, contents).expect("the torn write is simulated");

    let readable = storage.read_all().expect("the read succeeds");
    assert_eq!(
        texts(&readable),
        vec!["committed"],
        "a torn tail must not make the session unreadable"
    );

    storage
        .append(&user_message("after", "e3"))
        .expect("the append succeeds");
    let readable = storage.read_all().expect("the read succeeds");
    assert_eq!(
        texts(&readable),
        vec!["committed", "after"],
        "the next append must start a clean line"
    );
}

#[test]
fn a_terminated_malformed_line_is_still_reported() {
    let (_directory, path, _storage) = store();
    std::fs::write(&path, "not json\n").expect("the transcript is written");

    let error = JsonlSessionStorage::new(path)
        .read_all()
        .expect_err("real corruption must still fail the read");

    assert!(
        error
            .to_string()
            .starts_with("Invalid session entry on line 1: "),
        "unexpected message: {error}"
    );
}

#[test]
fn the_in_memory_store_round_trips() {
    let entries = vec![user_message("first", "e1")];
    let storage = InMemorySessionStorage::new(entries.clone());

    assert_eq!(storage.read_all().expect("the read succeeds"), entries);
    assert_eq!(storage.entries(), entries);

    storage
        .append(&user_message("second", "e2"))
        .expect("the append succeeds");
    assert_eq!(
        texts(&storage.read_all().expect("the read succeeds")),
        vec!["first", "second"]
    );
}

#[test]
fn the_in_memory_store_keeps_batch_order() {
    let storage = InMemorySessionStorage::default();

    storage
        .append_batch(&[user_message("first", "e1"), user_message("second", "e2")])
        .expect("the batch succeeds");

    assert_eq!(
        texts(&storage.read_all().expect("the read succeeds")),
        vec!["first", "second"]
    );
}

#[test]
fn a_batch_that_cannot_be_encoded_leaves_the_transcript_untouched() {
    let (_directory, _path, storage) = store();
    let good = user_message("committed", "e1");
    storage.append(&good).expect("the append succeeds");

    // A batch is encoded before the lock is taken, so a failure to encode cannot
    // half-write anything.
    let entries = storage.read_all().expect("the read succeeds");

    assert_eq!(entries, vec![good]);
}
