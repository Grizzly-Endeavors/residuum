//! Plumbing shared by the providers' streaming paths: turning a response body
//! into lines and server-sent events, and tracking what a [`StreamSink`] has
//! been sent.

use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::StreamExt;

use super::http::map_stream_read_error;
use super::{InferenceError, StreamDelta, StreamSink};

/// Wraps a [`StreamSink`] and remembers whether its consumer is holding a
/// partial response, so a failed attempt can be voided exactly once and only
/// when there is something to void.
pub(crate) struct TrackedSink<'a> {
    inner: &'a dyn StreamSink,
    holds_partial: AtomicBool,
}

impl<'a> TrackedSink<'a> {
    /// Track what is pushed to `inner`.
    pub(crate) fn new(inner: &'a dyn StreamSink) -> Self {
        Self {
            inner,
            holds_partial: AtomicBool::new(false),
        }
    }

    /// Tell the consumer everything streamed so far is void, if it was sent
    /// anything since the last restart. Called when an attempt fails, before
    /// the request is sent again or the next provider is tried.
    pub(crate) fn restart_if_needed(&self) {
        if self.holds_partial.swap(false, Ordering::SeqCst) {
            self.inner.push(StreamDelta::Restart);
        }
    }
}

impl StreamSink for TrackedSink<'_> {
    fn push(&self, delta: StreamDelta) {
        match &delta {
            StreamDelta::Text(text) | StreamDelta::Thinking(text) if text.is_empty() => return,
            StreamDelta::Text(_) | StreamDelta::Thinking(_) => {
                self.holds_partial.store(true, Ordering::SeqCst);
            }
            StreamDelta::Restart => self.holds_partial.store(false, Ordering::SeqCst),
        }
        self.inner.push(delta);
    }
}

/// Splits bytes into lines. A line ends at `\n`, `\r` or `\r\n`, and a
/// multi-byte character split across chunks is only decoded once its line is
/// complete.
#[derive(Default)]
pub(crate) struct LineBuffer {
    pending: Vec<u8>,
    /// The previous byte was `\r`, so a `\n` right after it belongs to the
    /// same line break.
    after_cr: bool,
}

impl LineBuffer {
    /// Take the next bytes of the stream; returns every line they complete.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &byte in bytes {
            if std::mem::take(&mut self.after_cr) && byte == b'\n' {
                continue;
            }
            match byte {
                b'\n' => lines.push(self.take_line()),
                b'\r' => {
                    lines.push(self.take_line());
                    self.after_cr = true;
                }
                _ => self.pending.push(byte),
            }
        }
        lines
    }

    /// The final line when the stream ended without a line break.
    pub(crate) fn finish(&mut self) -> Option<String> {
        (!self.pending.is_empty()).then(|| self.take_line())
    }

    fn take_line(&mut self) -> String {
        let bytes = std::mem::take(&mut self.pending);
        String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }
}

/// One server-sent event: its optional name and its data lines joined by
/// newlines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub(crate) event: Option<String>,
    pub(crate) data: String,
}

/// Incremental server-sent-events decoder.
#[derive(Default)]
pub(crate) struct SseDecoder {
    lines: LineBuffer,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    /// Take the next bytes of the stream; returns every event they complete.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let lines = self.lines.push(bytes);
        lines
            .into_iter()
            .filter_map(|line| self.accept_line(&line))
            .collect()
    }

    /// The final event when the stream ended without the blank line that
    /// normally closes it.
    pub(crate) fn finish(&mut self) -> Option<SseEvent> {
        let last_line = self.lines.finish();
        let completed = last_line.and_then(|line| self.accept_line(&line));
        completed.or_else(|| self.dispatch())
    }

    fn accept_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        // A line starting with a colon is a comment, used by servers as a
        // keep-alive.
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => self.data.push(value.to_string()),
            // `id` and `retry` steer reconnection, which the providers handle
            // themselves.
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let event = self.event.take();
        if self.data.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.data).join("\n");
        Some(SseEvent { event, data })
    }
}

/// Whether to keep reading a stream.
pub(crate) enum Flow {
    Continue,
    /// The stream's terminal event arrived; stop reading.
    Done,
}

/// Whether a response to a request to stream came back whole instead, as a
/// plain JSON body. Some OpenAI-compatible servers and proxies ignore the
/// request to stream; their answer is read as a non-streaming response, and
/// nothing is streamed for it.
pub(crate) fn answered_whole(response: &reqwest::Response) -> bool {
    let whole = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("application/json")
        });
    if whole {
        tracing::debug!("the server answered a request to stream with a whole response");
    }
    whole
}

/// Read a server-sent-events response to its end, handing each event to
/// `on_event` as it completes.
///
/// A stall of `idle_secs` surfaces as [`InferenceError::Stalled`] and a body
/// that breaks as [`InferenceError::StreamInterrupted`]; the caller decides
/// whether a stream that ended cleanly without its terminal event is complete.
pub(crate) async fn read_sse(
    response: reqwest::Response,
    idle_secs: u64,
    mut on_event: impl FnMut(SseEvent) -> Result<Flow, InferenceError>,
) -> Result<(), InferenceError> {
    let mut body = response.bytes_stream();
    let mut decoder = SseDecoder::default();
    while let Some(chunk) = body.next().await {
        let bytes = chunk.map_err(|e| map_stream_read_error(&e, idle_secs))?;
        for event in decoder.push(&bytes) {
            if matches!(on_event(event)?, Flow::Done) {
                return Ok(());
            }
        }
    }
    if let Some(event) = decoder.finish() {
        on_event(event)?;
    }
    Ok(())
}

/// Read a newline-delimited response to its end, handing each non-blank
/// line to `on_line` as it completes. Errors are as for [`read_sse`].
pub(crate) async fn read_ndjson(
    response: reqwest::Response,
    idle_secs: u64,
    mut on_line: impl FnMut(&str) -> Result<Flow, InferenceError>,
) -> Result<(), InferenceError> {
    let mut body = response.bytes_stream();
    let mut buffer = LineBuffer::default();
    while let Some(chunk) = body.next().await {
        let bytes = chunk.map_err(|e| map_stream_read_error(&e, idle_secs))?;
        for line in buffer.push(&bytes) {
            if !line.trim().is_empty() && matches!(on_line(&line)?, Flow::Done) {
                return Ok(());
            }
        }
    }
    if let Some(line) = buffer.finish()
        && !line.trim().is_empty()
    {
        on_line(&line)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::test_support::RecordingSink;

    #[test]
    fn lines_split_across_chunks_are_reassembled() {
        let mut buffer = LineBuffer::default();
        assert!(buffer.push(b"hel").is_empty(), "no line break yet");
        assert_eq!(
            buffer.push(b"lo\nwor"),
            vec!["hello"],
            "first line completes"
        );
        assert_eq!(buffer.push(b"ld\n"), vec!["world"], "second line completes");
        assert_eq!(buffer.finish(), None, "nothing is left over");
    }

    #[test]
    fn crlf_split_between_chunks_is_one_line_break() {
        let mut buffer = LineBuffer::default();
        assert_eq!(buffer.push(b"a\r"), vec!["a"], "CR ends the line");
        assert_eq!(
            buffer.push(b"\nb\r\n"),
            vec!["b"],
            "the LF that follows the CR is not a second break"
        );
    }

    #[test]
    fn multibyte_character_split_across_chunks_decodes_whole() {
        let text = "caf\u{e9} \u{1f600}\n";
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            let mut buffer = LineBuffer::default();
            let (head, tail) = bytes.split_at(split);
            let mut lines = buffer.push(head);
            lines.extend(buffer.push(tail));
            assert_eq!(
                lines,
                vec!["caf\u{e9} \u{1f600}"],
                "split at byte {split} must not corrupt the character"
            );
        }
    }

    #[test]
    fn unterminated_final_line_is_returned_by_finish() {
        let mut buffer = LineBuffer::default();
        assert!(buffer.push(b"tail").is_empty(), "no break yet");
        assert_eq!(buffer.finish(), Some("tail".to_string()), "flushed at end");
        assert_eq!(buffer.finish(), None, "flushing twice yields nothing");
    }

    fn decode_in_chunks(input: &[u8], chunk: usize) -> Vec<SseEvent> {
        let mut decoder = SseDecoder::default();
        let mut events = Vec::new();
        for piece in input.chunks(chunk) {
            events.extend(decoder.push(piece));
        }
        events.extend(decoder.finish());
        events
    }

    #[test]
    fn sse_events_decode_the_same_at_every_chunk_size() {
        let input = b"event: ping\ndata: {\"a\":1}\n\n: keep-alive\n\ndata: first\ndata: second\n\nevent: x\r\ndata: crlf\r\n\r\n";
        let expected = vec![
            SseEvent {
                event: Some("ping".to_string()),
                data: "{\"a\":1}".to_string(),
            },
            SseEvent {
                event: None,
                data: "first\nsecond".to_string(),
            },
            SseEvent {
                event: Some("x".to_string()),
                data: "crlf".to_string(),
            },
        ];
        for chunk in 1..=input.len() {
            assert_eq!(
                decode_in_chunks(input, chunk),
                expected,
                "chunk size {chunk} must decode identically"
            );
        }
    }

    #[test]
    fn sse_data_without_a_space_after_the_colon_is_kept_verbatim() {
        let events = decode_in_chunks(b"data:{\"x\":1}\n\n", 64);
        assert_eq!(
            events,
            vec![SseEvent {
                event: None,
                data: "{\"x\":1}".to_string(),
            }],
            "the space after the colon is optional"
        );
    }

    #[test]
    fn sse_final_event_without_blank_line_is_flushed() {
        let events = decode_in_chunks(b"data: last", 3);
        assert_eq!(
            events,
            vec![SseEvent {
                event: None,
                data: "last".to_string(),
            }],
            "a stream that ends mid-event still delivers it"
        );
    }

    #[test]
    fn sse_event_without_data_is_dropped() {
        assert!(
            decode_in_chunks(b"event: ping\n\n", 4).is_empty(),
            "an event with no data carries nothing to handle"
        );
    }

    #[test]
    fn tracked_sink_restarts_only_when_something_was_pushed() {
        let recorder = RecordingSink::default();
        let tracked = TrackedSink::new(&recorder);

        tracked.restart_if_needed();
        assert!(
            recorder.deltas().is_empty(),
            "nothing pushed, so nothing to void"
        );

        tracked.push(StreamDelta::Text("partial".to_string()));
        tracked.restart_if_needed();
        tracked.restart_if_needed();
        assert_eq!(
            recorder.deltas(),
            vec![
                StreamDelta::Text("partial".to_string()),
                StreamDelta::Restart
            ],
            "one restart voids the partial response, and only one"
        );
    }

    #[test]
    fn tracked_sink_counts_reasoning_as_something_pushed() {
        let recorder = RecordingSink::default();
        let tracked = TrackedSink::new(&recorder);
        tracked.push(StreamDelta::Thinking("hmm".to_string()));
        tracked.restart_if_needed();
        assert_eq!(
            recorder.deltas().last(),
            Some(&StreamDelta::Restart),
            "streamed reasoning is void too"
        );
    }

    #[test]
    fn tracked_sink_drops_empty_deltas() {
        let recorder = RecordingSink::default();
        let tracked = TrackedSink::new(&recorder);
        tracked.push(StreamDelta::Text(String::new()));
        tracked.push(StreamDelta::Thinking(String::new()));
        tracked.restart_if_needed();
        assert!(
            recorder.deltas().is_empty(),
            "empty deltas are neither forwarded nor counted as output"
        );
    }

    #[test]
    fn restart_pushed_by_a_provider_clears_the_partial_flag() {
        let recorder = RecordingSink::default();
        let tracked = TrackedSink::new(&recorder);
        tracked.push(StreamDelta::Text("partial".to_string()));
        tracked.push(StreamDelta::Restart);
        tracked.restart_if_needed();
        assert_eq!(
            recorder.deltas(),
            vec![
                StreamDelta::Text("partial".to_string()),
                StreamDelta::Restart
            ],
            "a restart already sent is not repeated"
        );
    }
}
