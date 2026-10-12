//! Behaviour of the deterministic in-memory provider.

use std::sync::Arc;

use futures_util::StreamExt;
use tau_ai::fake::FakeProvider;
use tau_protocol::content::TextContent;
use tau_protocol::message::AssistantMessage;
use tau_protocol::provider::{ManualCancellation, ModelProvider, ModelRequest};
use tau_protocol::provider_event::{AssistantMessageEvent, DoneReason};

fn text_delta(delta: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::TextDelta {
        content_index: 0,
        delta: delta.into(),
        partial: AssistantMessage {
            content: vec![
                TextContent {
                    text: delta.into(),
                    text_signature: None,
                }
                .into(),
            ],
            ..AssistantMessage::default()
        },
    }
}

fn done(text: &str) -> AssistantMessageEvent {
    AssistantMessageEvent::Done {
        reason: DoneReason::Stop,
        message: AssistantMessage {
            content: vec![
                TextContent {
                    text: text.into(),
                    text_signature: None,
                }
                .into(),
            ],
            ..AssistantMessage::default()
        },
    }
}

fn collect(provider: &FakeProvider, request: ModelRequest) -> Vec<AssistantMessageEvent> {
    futures_executor::block_on(provider.stream_response(request).collect::<Vec<_>>())
}

#[test]
fn the_streams_are_replayed_in_order() {
    let provider = FakeProvider::new(vec![vec![text_delta("a"), done("a")]]);

    let events = collect(&provider, ModelRequest::new("fake", "system"));

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind(), "text_delta");
    assert_eq!(events[1].kind(), "done");
}

#[test]
fn each_call_is_recorded_with_its_model_system_messages_and_tools() {
    let provider = FakeProvider::new(vec![vec![done("a")]]);
    let request = ModelRequest::new("fake-model", "be helpful").with_session_id("session-1");

    let _ = collect(&provider, request);

    let calls = provider.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].model, "fake-model");
    assert_eq!(calls[0].system, "be helpful");
    assert!(calls[0].messages.is_empty());
    assert!(calls[0].tools.is_empty());
    assert_eq!(provider.session_ids(), vec![Some("session-1".to_string())]);
}

#[test]
fn an_exhausted_provider_yields_nothing_instead_of_panicking() {
    let provider = FakeProvider::new(vec![vec![done("a")]]);

    let first = collect(&provider, ModelRequest::new("fake", "system"));
    let second = collect(&provider, ModelRequest::new("fake", "system"));

    assert_eq!(first.len(), 1);
    assert!(second.is_empty());
    assert_eq!(provider.calls().len(), 2);
}

#[test]
fn a_cancelled_signal_stops_the_stream() {
    let provider = FakeProvider::new(vec![vec![text_delta("a"), done("a")]]);
    let signal = Arc::new(ManualCancellation::default());
    signal.cancel();

    let events = collect(
        &provider,
        ModelRequest::new("fake", "system").with_signal(signal),
    );

    assert!(
        events.is_empty(),
        "a cancelled request must yield no events"
    );
}
