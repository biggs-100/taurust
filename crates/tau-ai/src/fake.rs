//! A deterministic in-memory provider, used by tests and by local runs.
//!
//! Ported from tau's `tau_ai/fake.py`.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::vec::IntoIter;

use futures_core::Stream;

use tau_protocol::message::AgentMessage;
use tau_protocol::provider::{CancellationToken, ModelProvider, ModelRequest, ProviderStream};
use tau_protocol::provider_event::AssistantMessageEvent;
use tau_protocol::tool::ToolSpec;

/// What one call to the provider was asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCall {
    /// The requested model.
    pub model: String,
    /// The requested system prompt.
    pub system: String,
    /// The conversation the provider was given.
    pub messages: Vec<AgentMessage>,
    /// The tools the provider was given.
    pub tools: Vec<ToolSpec>,
}

/// A provider that replays predefined assistant event streams.
///
/// Streams are consumed in order: the first request replays the first stream, and
/// a request beyond the supplied streams replays nothing rather than failing.
#[derive(Debug, Default)]
pub struct FakeProvider {
    streams: Mutex<VecDeque<Vec<AssistantMessageEvent>>>,
    calls: Mutex<Vec<RecordedCall>>,
    session_ids: Mutex<Vec<Option<String>>>,
}

impl FakeProvider {
    /// A provider that replays the given streams, in order.
    pub fn new(streams: Vec<Vec<AssistantMessageEvent>>) -> Self {
        Self {
            streams: Mutex::new(streams.into()),
            calls: Mutex::new(Vec::new()),
            session_ids: Mutex::new(Vec::new()),
        }
    }

    /// Every call made so far, in order.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("calls lock").clone()
    }

    /// The session id of every call so far, in order.
    pub fn session_ids(&self) -> Vec<Option<String>> {
        self.session_ids.lock().expect("session ids lock").clone()
    }

    /// How many streams are still queued.
    pub fn remaining_streams(&self) -> usize {
        self.streams.lock().expect("streams lock").len()
    }
}

impl ModelProvider for FakeProvider {
    fn stream_response(&self, request: ModelRequest) -> ProviderStream {
        self.calls.lock().expect("calls lock").push(RecordedCall {
            model: request.model.clone(),
            system: request.system.clone(),
            messages: request.messages.clone(),
            tools: request.tools.clone(),
        });
        self.session_ids
            .lock()
            .expect("session ids lock")
            .push(request.session_id.clone());

        let events = self
            .streams
            .lock()
            .expect("streams lock")
            .pop_front()
            .unwrap_or_default();

        Box::pin(ReplayStream {
            events: events.into_iter(),
            signal: request.signal,
        })
    }
}

/// Replays a fixed list of events, checking the cancellation token before each one.
struct ReplayStream {
    events: IntoIter<AssistantMessageEvent>,
    signal: Option<Arc<dyn CancellationToken>>,
}

impl Stream for ReplayStream {
    type Item = AssistantMessageEvent;

    fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if let Some(signal) = this.signal.as_ref()
            && signal.is_cancelled()
        {
            return Poll::Ready(None);
        }
        Poll::Ready(this.events.next())
    }
}
