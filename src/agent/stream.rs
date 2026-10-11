//! Streamed model output on its way to the main conversation.
//!
//! A provider pushes a response's text and reasoning into a [`StreamSink`] as
//! it arrives, from inside its read loop, so the sink must never block. The
//! turn loop forwards them to the main conversation stream, and a provider
//! can push a token at a time, so [`DeltaCoalescer`] batches them into
//! frames of about [`FLUSH_INTERVAL`].

use std::time::Duration;

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::inference::{StreamDelta, StreamSink};

/// How long streamed output waits in the buffer before it is published.
pub(crate) const FLUSH_INTERVAL: Duration = Duration::from_millis(50);

/// A sink that hands every delta to the turn loop over an unbounded channel,
/// so a provider pushing from inside its read loop never waits on the bus.
pub(crate) struct ChannelSink(mpsc::UnboundedSender<StreamDelta>);

impl ChannelSink {
    /// A sink and the receiver the turn loop drains it from.
    pub(crate) fn channel() -> (Self, mpsc::UnboundedReceiver<StreamDelta>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self(tx), rx)
    }
}

impl StreamSink for ChannelSink {
    fn push(&self, delta: StreamDelta) {
        // The receiver is dropped only once the model call it served has
        // ended, and a delta pushed after that has nobody to show it to.
        drop(self.0.send(delta));
    }
}

/// A sink for a turn whose output is not streamed anywhere.
pub(crate) struct DiscardSink;

impl StreamSink for DiscardSink {
    fn push(&self, _delta: StreamDelta) {}
}

/// One unit of streamed output ready to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StreamPiece {
    /// Response text.
    Text(String),
    /// Readable reasoning.
    Thinking(String),
    /// Everything published so far for the call is void.
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Text,
    Thinking,
}

#[derive(Debug)]
struct Pending {
    kind: PendingKind,
    text: String,
    /// When the buffer must be published even if more is still coming.
    flush_at: Instant,
}

impl Pending {
    fn into_piece(self) -> StreamPiece {
        match self.kind {
            PendingKind::Text => StreamPiece::Text(self.text),
            PendingKind::Thinking => StreamPiece::Thinking(self.text),
        }
    }
}

/// Batches a model call's streamed deltas into pieces worth publishing.
///
/// Deltas of one kind accumulate until [`FLUSH_INTERVAL`] has passed since the
/// first of them arrived; a delta of the other kind publishes what was
/// buffered first, so text and reasoning keep their order. The caller
/// publishes whatever [`push`](Self::push) returns, [`take_pending`](Self::take_pending)
/// at the interval ([`flush_at`](Self::flush_at) says when) and once more when
/// the call ends.
#[derive(Debug, Default)]
pub(crate) struct DeltaCoalescer {
    pending: Option<Pending>,
}

impl DeltaCoalescer {
    /// Take in one delta at `now`, returning the pieces to publish right away.
    pub(crate) fn push(&mut self, delta: StreamDelta, now: Instant) -> Vec<StreamPiece> {
        let mut ready = Vec::new();
        match delta {
            StreamDelta::Text(text) => self.append(PendingKind::Text, text, now, &mut ready),
            StreamDelta::Thinking(text) => {
                self.append(PendingKind::Thinking, text, now, &mut ready);
            }
            StreamDelta::Restart => {
                // What was buffered belongs to the attempt being thrown away.
                self.pending = None;
                ready.push(StreamPiece::Restart);
            }
        }
        if self.pending.as_ref().is_some_and(|p| now >= p.flush_at) {
            ready.extend(self.take_pending());
        }
        ready
    }

    fn append(
        &mut self,
        kind: PendingKind,
        text: String,
        now: Instant,
        ready: &mut Vec<StreamPiece>,
    ) {
        if text.is_empty() {
            return;
        }
        match &mut self.pending {
            Some(pending) if pending.kind == kind => pending.text.push_str(&text),
            _ => {
                ready.extend(self.take_pending());
                self.pending = Some(Pending {
                    kind,
                    text,
                    flush_at: now + FLUSH_INTERVAL,
                });
            }
        }
    }

    /// When the buffered output must be published, if any is buffered.
    pub(crate) fn flush_at(&self) -> Option<Instant> {
        self.pending.as_ref().map(|p| p.flush_at)
    }

    /// Take whatever is buffered.
    pub(crate) fn take_pending(&mut self) -> Option<StreamPiece> {
        self.pending.take().map(Pending::into_piece)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> StreamDelta {
        StreamDelta::Text(s.to_string())
    }

    fn thinking(s: &str) -> StreamDelta {
        StreamDelta::Thinking(s.to_string())
    }

    #[test]
    fn deltas_of_one_kind_within_the_interval_become_one_piece() {
        let start = Instant::now();
        let mut coalescer = DeltaCoalescer::default();

        assert!(coalescer.push(text("Hel"), start).is_empty());
        assert!(
            coalescer
                .push(text("lo, "), start + Duration::from_millis(10))
                .is_empty()
        );
        assert!(
            coalescer
                .push(text("world"), start + Duration::from_millis(20))
                .is_empty()
        );

        assert_eq!(
            coalescer.take_pending(),
            Some(StreamPiece::Text("Hello, world".to_string()))
        );
        assert_eq!(coalescer.take_pending(), None, "taking empties the buffer");
    }

    #[test]
    fn the_buffer_is_due_one_interval_after_its_first_delta() {
        let start = Instant::now();
        let mut coalescer = DeltaCoalescer::default();
        assert_eq!(coalescer.flush_at(), None, "nothing buffered, nothing due");

        coalescer.push(text("a"), start);
        let due = coalescer.flush_at();
        coalescer.push(text("b"), start + Duration::from_millis(30));

        assert_eq!(due, Some(start + FLUSH_INTERVAL));
        assert_eq!(
            coalescer.flush_at(),
            due,
            "later deltas don't push the deadline out, or a steady stream would never publish"
        );
    }

    #[test]
    fn a_delta_arriving_after_the_deadline_publishes_the_buffer_with_it() {
        let start = Instant::now();
        let mut coalescer = DeltaCoalescer::default();
        coalescer.push(text("a"), start);

        let ready = coalescer.push(text("b"), start + FLUSH_INTERVAL);

        assert_eq!(ready, vec![StreamPiece::Text("ab".to_string())]);
        assert_eq!(coalescer.flush_at(), None);
    }

    #[test]
    fn a_change_of_kind_publishes_what_came_before_it() {
        let start = Instant::now();
        let mut coalescer = DeltaCoalescer::default();
        coalescer.push(thinking("hm"), start);

        let ready = coalescer.push(text("Sure"), start);

        assert_eq!(ready, vec![StreamPiece::Thinking("hm".to_string())]);
        assert_eq!(
            coalescer.take_pending(),
            Some(StreamPiece::Text("Sure".to_string()))
        );
    }

    #[test]
    fn a_restart_discards_the_buffer_and_is_published_at_once() {
        let start = Instant::now();
        let mut coalescer = DeltaCoalescer::default();
        coalescer.push(text("half a sent"), start);

        let ready = coalescer.push(StreamDelta::Restart, start);

        assert_eq!(ready, vec![StreamPiece::Restart]);
        assert_eq!(
            coalescer.take_pending(),
            None,
            "the buffered text belongs to the attempt being thrown away"
        );
    }

    #[test]
    fn empty_deltas_publish_nothing() {
        let start = Instant::now();
        let mut coalescer = DeltaCoalescer::default();

        assert!(coalescer.push(text(""), start).is_empty());
        assert!(coalescer.push(thinking(""), start).is_empty());
        assert_eq!(coalescer.flush_at(), None);
    }

    #[test]
    fn a_channel_sink_never_blocks_and_delivers_in_order() {
        let (sink, mut rx) = ChannelSink::channel();

        sink.push(text("a"));
        sink.push(thinking("b"));
        sink.push(StreamDelta::Restart);

        assert_eq!(rx.try_recv().ok(), Some(text("a")));
        assert_eq!(rx.try_recv().ok(), Some(thinking("b")));
        assert_eq!(rx.try_recv().ok(), Some(StreamDelta::Restart));
    }

    #[test]
    fn a_channel_sink_whose_receiver_is_gone_drops_deltas_quietly() {
        let (sink, rx) = ChannelSink::channel();
        drop(rx);

        sink.push(text("late"));
    }
}
