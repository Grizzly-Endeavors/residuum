//! One proxied connection carried over a v2 tunnel: its flow-control state
//! and [`TunnelIo`], the `AsyncRead + AsyncWrite` handle the handler uses.
//!
//! Each direction has a window of [`WINDOW`] bytes. The sender spends credit
//! as it writes and the receiver returns it with `stream_credit` as its
//! consumer reads, so a stalled consumer stalls only its own stream: the
//! websocket reader never waits on a consumer, it queues data (bounded by the
//! window) and wakes the reader task of that one stream.

use std::collections::{HashMap, VecDeque};
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::{Bytes, Message};
use tokio_util::sync::PollSender;
use uuid::Uuid;

use super::frames::V2Frame;

/// Unacknowledged bytes allowed in flight per stream, per direction.
pub(crate) const WINDOW: usize = 256 * 1024;

/// Largest payload in one binary data message.
pub(crate) const MAX_CHUNK: usize = 64 * 1024;

/// Most concurrent streams one tunnel carries.
pub(crate) const MAX_STREAMS: usize = 256;

/// A stream with no data in either direction for this long is closed.
pub(crate) const IDLE_LIMIT: Duration = Duration::from_mins(10);

/// Read credit is returned in batches of at least this many bytes, or as soon
/// as the receive queue is drained, so small reads do not each cost a frame.
const CREDIT_BATCH: usize = 32 * 1024;

/// Capacity of the ordered outbound queue (stream data and closes).
const ORDERED_QUEUE: usize = 64;

/// The sending ends of a tunnel's outbound queues.
#[derive(Clone)]
pub(crate) struct Outbox {
    /// Stream data and stream closes, in order.
    ordered: mpsc::Sender<Message>,
    /// Everything else: credit, keepalives, requests. Never waits.
    control: mpsc::UnboundedSender<Message>,
}

/// The receiving ends, owned by the writer task.
pub(crate) struct OutboxReceivers {
    pub ordered: mpsc::Receiver<Message>,
    pub control: mpsc::UnboundedReceiver<Message>,
}

impl Outbox {
    pub(crate) fn new() -> (Self, OutboxReceivers) {
        let (ordered, ordered_rx) = mpsc::channel(ORDERED_QUEUE);
        let (control, control_rx) = mpsc::unbounded_channel();
        (
            Self { ordered, control },
            OutboxReceivers {
                ordered: ordered_rx,
                control: control_rx,
            },
        )
    }

    fn text(frame: &V2Frame) -> Option<Message> {
        match serde_json::to_string(frame) {
            Ok(json) => Some(Message::text(json)),
            Err(e) => {
                tracing::error!(error = %e, "failed to serialize a v2 frame");
                None
            }
        }
    }

    /// Queue a control frame without waiting. Control frames may pass queued
    /// data, so a frame that must follow a stream's data goes through the
    /// stream itself.
    pub(crate) fn send_control(&self, frame: &V2Frame) {
        if let Some(message) = Self::text(frame)
            && self.control.send(message).is_err()
        {
            tracing::debug!("v2 control queue closed");
        }
    }

    /// Queue a raw message on the control queue (used for the websocket close).
    pub(crate) fn send_control_message(&self, message: Message) {
        if self.control.send(message).is_err() {
            tracing::debug!("v2 control queue closed");
        }
    }
}

/// Why inbound data from the relay was refused.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PushError {
    /// The relay sent more than the window allows.
    WindowExceeded,
    /// The payload is larger than [`MAX_CHUNK`].
    TooLarge,
}

struct Inner {
    /// Data received from the relay and not yet read.
    queue: VecDeque<Vec<u8>>,
    /// How much of the front chunk has been read.
    front_offset: usize,
    /// Bytes received and not yet returned to the relay as credit.
    unacked_in: usize,
    /// Bytes read by the consumer whose credit has not been sent yet.
    read_uncredited: usize,
    /// The relay closed the stream: EOF once the queue is drained.
    relay_closed: bool,
    /// The stream ended abnormally; reads and writes fail.
    reset: Option<&'static str>,
    /// A `stream_close` has been sent, or none is needed.
    close_settled: bool,
    /// Bytes the relay will currently accept.
    send_credit: usize,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
    last_activity: Instant,
}

/// State shared between the websocket reader and the stream's [`TunnelIo`].
pub(crate) struct StreamShared {
    id: Uuid,
    outbox: Outbox,
    inner: Mutex<Inner>,
}

impl StreamShared {
    fn new(id: Uuid, outbox: Outbox) -> Self {
        Self {
            id,
            outbox,
            inner: Mutex::new(Inner {
                queue: VecDeque::new(),
                front_offset: 0,
                unacked_in: 0,
                read_uncredited: 0,
                relay_closed: false,
                reset: None,
                close_settled: false,
                send_credit: WINDOW,
                read_waker: None,
                write_waker: None,
                last_activity: Instant::now(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Accept data from the relay. Never waits.
    ///
    /// # Errors
    ///
    /// [`PushError`] when the data breaks the protocol; the caller closes the
    /// stream.
    pub(crate) fn push_data(&self, payload: &[u8]) -> Result<(), PushError> {
        if payload.len() > MAX_CHUNK {
            return Err(PushError::TooLarge);
        }
        let waker = {
            let mut inner = self.lock();
            if inner.relay_closed || inner.reset.is_some() {
                tracing::debug!(stream_id = %self.id, "data after stream close dropped");
                return Ok(());
            }
            if payload.is_empty() {
                return Ok(());
            }
            if inner.unacked_in + payload.len() > WINDOW {
                return Err(PushError::WindowExceeded);
            }
            inner.unacked_in += payload.len();
            inner.queue.push_back(payload.to_vec());
            inner.last_activity = Instant::now();
            inner.read_waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    /// The relay granted more send window. The window never grows past
    /// [`WINDOW`], so a misbehaving relay cannot inflate it.
    pub(crate) fn add_credit(&self, bytes: u64) {
        let waker = {
            let mut inner = self.lock();
            let grant = usize::try_from(bytes).unwrap_or(usize::MAX);
            inner.send_credit = inner.send_credit.saturating_add(grant).min(WINDOW);
            inner.write_waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    /// The relay closed the stream: what it already sent is still readable,
    /// then reads see EOF.
    pub(crate) fn relay_closed(&self) {
        let wakers = {
            let mut inner = self.lock();
            inner.relay_closed = true;
            inner.close_settled = true;
            (inner.read_waker.take(), inner.write_waker.take())
        };
        wake_both(wakers);
    }

    /// End the stream abnormally. When `tell_relay` a `stream_close` with
    /// `reason` is sent (at most once per stream).
    pub(crate) fn reset(&self, reason: &'static str, tell_relay: bool) {
        let wakers = {
            let mut inner = self.lock();
            if inner.reset.is_none() {
                inner.reset = Some(reason);
            }
            if tell_relay && !inner.close_settled {
                self.outbox.send_control(&V2Frame::StreamClose {
                    stream_id: self.id,
                    reason: Some(reason.to_string()),
                });
            }
            inner.close_settled = true;
            inner.queue.clear();
            (inner.read_waker.take(), inner.write_waker.take())
        };
        wake_both(wakers);
    }

    fn idle_for(&self) -> Duration {
        Instant::now().saturating_duration_since(self.lock().last_activity)
    }
}

fn wake_both((read, write): (Option<Waker>, Option<Waker>)) {
    if let Some(waker) = read {
        waker.wake();
    }
    if let Some(waker) = write {
        waker.wake();
    }
}

/// Why a stream could not be opened.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OpenError {
    /// The tunnel already carries [`MAX_STREAMS`] streams.
    TooManyStreams,
    /// The tunnel has ended.
    Closed,
    /// The relay reused the id of a live stream.
    Duplicate,
}

struct TableInner {
    streams: HashMap<Uuid, Arc<StreamShared>>,
    closed: bool,
}

/// The live streams of one tunnel.
pub(crate) struct StreamTable {
    inner: Mutex<TableInner>,
    outbox: Outbox,
}

impl StreamTable {
    pub(crate) fn new(outbox: Outbox) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(TableInner {
                streams: HashMap::new(),
                closed: false,
            }),
            outbox,
        })
    }

    fn lock(&self) -> MutexGuard<'_, TableInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Register a stream the relay opened and return the consumer's handle.
    ///
    /// # Errors
    ///
    /// [`OpenError`] when the tunnel is full, ended, or the id is in use.
    pub(crate) fn open(self: &Arc<Self>, id: Uuid) -> Result<TunnelIo, OpenError> {
        let shared = Arc::new(StreamShared::new(id, self.outbox.clone()));
        {
            let mut table = self.lock();
            if table.closed {
                return Err(OpenError::Closed);
            }
            if table.streams.contains_key(&id) {
                return Err(OpenError::Duplicate);
            }
            if table.streams.len() >= MAX_STREAMS {
                return Err(OpenError::TooManyStreams);
            }
            table.streams.insert(id, Arc::clone(&shared));
        }
        Ok(TunnelIo {
            shared,
            table: Arc::clone(self),
            sender: PollSender::new(self.outbox.ordered.clone()),
        })
    }

    pub(crate) fn get(&self, id: Uuid) -> Option<Arc<StreamShared>> {
        self.lock().streams.get(&id).cloned()
    }

    pub(crate) fn remove(&self, id: Uuid) {
        self.lock().streams.remove(&id);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().streams.len()
    }

    /// The tunnel ended: fail every stream and refuse new ones. The relay is
    /// not told; it already dropped them with the connection.
    pub(crate) fn close_all(&self) {
        let drained: Vec<Arc<StreamShared>> = {
            let mut table = self.lock();
            table.closed = true;
            table.streams.drain().map(|(_, s)| s).collect()
        };
        for stream in drained {
            stream.reset("the relay connection ended", false);
        }
    }

    /// Close streams with no data in either direction for [`IDLE_LIMIT`].
    pub(crate) fn sweep_idle(&self) {
        let idle: Vec<Arc<StreamShared>> = self
            .lock()
            .streams
            .values()
            .filter(|s| s.idle_for() >= IDLE_LIMIT)
            .cloned()
            .collect();
        for stream in idle {
            tracing::info!(stream_id = %stream.id, "closing idle tunnel stream");
            self.remove(stream.id);
            stream.reset("idle", true);
        }
    }
}

/// A browser connection carried over the tunnel, as a byte stream.
///
/// Reads return what the relay sent and hand receive window back as they
/// drain it. Writes spend send window and wait when it is gone. Shutting
/// down or dropping the handle tells the relay to close the stream.
pub(crate) struct TunnelIo {
    shared: Arc<StreamShared>,
    table: Arc<StreamTable>,
    sender: PollSender<Message>,
}

impl TunnelIo {
    fn close_frame(&self) -> Option<Message> {
        Outbox::text(&V2Frame::StreamClose {
            stream_id: self.shared.id,
            reason: None,
        })
    }
}

fn reset_error(reason: &str) -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionReset, reason.to_string())
}

impl AsyncRead for TunnelIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut inner = this.shared.lock();
        if let Some(reason) = inner.reset {
            return Poll::Ready(Err(reset_error(reason)));
        }
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let offset = inner.front_offset;
        if let Some(front) = inner.queue.front() {
            let available = front.get(offset..).unwrap_or_default();
            let take = available.len().min(buf.remaining());
            buf.put_slice(available.get(..take).unwrap_or_default());
            let finished = offset + take >= front.len();
            if finished {
                inner.queue.pop_front();
                inner.front_offset = 0;
            } else {
                inner.front_offset = offset + take;
            }
            inner.read_uncredited += take;
            inner.last_activity = Instant::now();
            if inner.read_uncredited >= CREDIT_BATCH || inner.queue.is_empty() {
                let bytes = inner.read_uncredited;
                inner.read_uncredited = 0;
                inner.unacked_in = inner.unacked_in.saturating_sub(bytes);
                if !inner.relay_closed {
                    this.shared.outbox.send_control(&V2Frame::StreamCredit {
                        stream_id: this.shared.id,
                        bytes: bytes as u64,
                    });
                }
            }
            return Poll::Ready(Ok(()));
        }
        if inner.relay_closed || inner.close_settled {
            return Poll::Ready(Ok(()));
        }
        inner.read_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl AsyncWrite for TunnelIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let mut inner = this.shared.lock();
        if let Some(reason) = inner.reset {
            return Poll::Ready(Err(reset_error(reason)));
        }
        if inner.close_settled {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if inner.send_credit == 0 {
            inner.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
        match this.sender.poll_reserve(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(_)) => return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into())),
            Poll::Ready(Ok(())) => {}
        }
        let take = data.len().min(inner.send_credit).min(MAX_CHUNK);
        let mut message = Vec::with_capacity(16 + take);
        message.extend_from_slice(this.shared.id.as_bytes());
        message.extend_from_slice(data.get(..take).unwrap_or_default());
        if this
            .sender
            .send_item(Message::Binary(Bytes::from(message)))
            .is_err()
        {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        inner.send_credit -= take;
        inner.last_activity = Instant::now();
        Poll::Ready(Ok(take))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let mut inner = this.shared.lock();
        if inner.close_settled {
            drop(inner);
            this.table.remove(this.shared.id);
            return Poll::Ready(Ok(()));
        }
        match this.sender.poll_reserve(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Err(_)) => {
                inner.close_settled = true;
                drop(inner);
                this.table.remove(this.shared.id);
                return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
            }
            Poll::Ready(Ok(())) => {}
        }
        let Some(frame) = this.close_frame() else {
            this.sender.abort_send();
            return Poll::Ready(Err(io::ErrorKind::InvalidData.into()));
        };
        inner.close_settled = true;
        let read_waker = inner.read_waker.take();
        drop(inner);
        let sent = this.sender.send_item(frame);
        this.table.remove(this.shared.id);
        if let Some(waker) = read_waker {
            waker.wake();
        }
        Poll::Ready(sent.map_err(|_e| io::ErrorKind::BrokenPipe.into()))
    }
}

impl Drop for TunnelIo {
    fn drop(&mut self) {
        self.sender.abort_send();
        let needs_close = {
            let mut inner = self.shared.lock();
            let needed = !inner.close_settled;
            inner.close_settled = true;
            needed
        };
        self.table.remove(self.shared.id);
        if !needs_close {
            return;
        }
        let Some(frame) = self.close_frame() else {
            return;
        };
        let ordered = self.shared.outbox.ordered.clone();
        match ordered.try_send(frame) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(frame)) => {
                // The close has to follow the stream's queued data, so wait for
                // room instead of overtaking it on the control queue.
                if tokio::runtime::Handle::try_current().is_ok() {
                    crate::util::spawn_in_span(async move {
                        if ordered.send(frame).await.is_err() {
                            tracing::debug!("v2 ordered queue closed before a stream close");
                        }
                    });
                } else {
                    tracing::debug!("no runtime to deliver a stream close; relay will time it out");
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::debug!("v2 ordered queue closed; stream close not sent");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    fn setup() -> (Arc<StreamTable>, OutboxReceivers) {
        let (outbox, rx) = Outbox::new();
        (StreamTable::new(outbox), rx)
    }

    async fn next_control(rx: &mut OutboxReceivers) -> V2Frame {
        let Message::Text(text) = rx.control.recv().await.unwrap() else {
            panic!("expected a text frame");
        };
        serde_json::from_str(text.as_str()).unwrap()
    }

    #[tokio::test]
    async fn credit_is_returned_as_the_consumer_reads_not_before() {
        let (table, mut rx) = setup();
        let id = Uuid::new_v4();
        let mut io = table.open(id).unwrap();
        let shared = table.get(id).unwrap();
        shared.push_data(&vec![7; 50_000]).unwrap();
        assert!(rx.control.try_recv().is_err(), "no credit before a read");
        let mut buf = vec![0; 40_000];
        io.read_exact(&mut buf).await.unwrap();
        let V2Frame::StreamCredit { bytes: first, .. } = next_control(&mut rx).await else {
            panic!("expected credit");
        };
        assert_eq!(first, 40_000);
        let mut rest = vec![0; 10_000];
        io.read_exact(&mut rest).await.unwrap();
        let V2Frame::StreamCredit { bytes: second, .. } = next_control(&mut rx).await else {
            panic!("expected credit");
        };
        assert_eq!(second, 10_000);
    }

    #[tokio::test]
    async fn the_receive_window_is_enforced() {
        let (table, _rx) = setup();
        let id = Uuid::new_v4();
        let _io = table.open(id).unwrap();
        let shared = table.get(id).unwrap();
        for _ in 0..WINDOW / MAX_CHUNK {
            shared.push_data(&vec![1; MAX_CHUNK]).unwrap();
        }
        assert_eq!(shared.push_data(&[1]), Err(PushError::WindowExceeded));
    }

    #[tokio::test]
    async fn oversized_payloads_are_refused() {
        let (table, _rx) = setup();
        let id = Uuid::new_v4();
        let _io = table.open(id).unwrap();
        let shared = table.get(id).unwrap();
        assert_eq!(
            shared.push_data(&vec![1; MAX_CHUNK + 1]),
            Err(PushError::TooLarge)
        );
    }

    #[tokio::test]
    async fn relay_close_delivers_queued_data_then_eof() {
        let (table, _rx) = setup();
        let id = Uuid::new_v4();
        let mut io = table.open(id).unwrap();
        let shared = table.get(id).unwrap();
        shared.push_data(b"hello").unwrap();
        shared.relay_closed();
        let mut all = Vec::new();
        io.read_to_end(&mut all).await.unwrap();
        assert_eq!(all, b"hello");
        assert!(io.write_all(b"x").await.is_err());
    }

    #[tokio::test]
    async fn writes_stop_without_credit_and_resume_when_granted() {
        let (table, mut rx) = setup();
        let id = Uuid::new_v4();
        let mut io = table.open(id).unwrap();
        let shared = table.get(id).unwrap();
        let drain = tokio::spawn(async move { while rx.ordered.recv().await.is_some() {} });
        io.write_all(&vec![0; WINDOW]).await.unwrap();
        let blocked = tokio::time::timeout(Duration::from_millis(100), io.write_all(b"more")).await;
        assert!(blocked.is_err(), "no credit means no progress");
        shared.add_credit(10);
        tokio::time::timeout(Duration::from_secs(2), io.write_all(b"more"))
            .await
            .unwrap()
            .unwrap();
        drain.abort();
    }

    #[tokio::test]
    async fn a_stalled_stream_does_not_block_another() {
        let (table, mut rx) = setup();
        let mut stalled = table.open(Uuid::new_v4()).unwrap();
        let live_id = Uuid::new_v4();
        let mut live = table.open(live_id).unwrap();
        stalled.write_all(&vec![0; WINDOW]).await.unwrap();
        let stalled_task = tokio::spawn(async move {
            stalled.write_all(b"blocked").await.ok();
        });
        let data = tokio::spawn(async move {
            let mut seen = 0;
            while let Some(Message::Binary(b)) = rx.ordered.recv().await {
                if b.get(..16) == Some(live_id.as_bytes().as_slice()) {
                    seen += b.len() - 16;
                    if seen >= 1000 {
                        return seen;
                    }
                }
            }
            seen
        });
        live.write_all(&[9; 1000]).await.unwrap();
        let seen = tokio::time::timeout(Duration::from_secs(2), data)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seen, 1000);
        assert!(!stalled_task.is_finished());
        stalled_task.abort();
    }

    #[tokio::test]
    async fn shutdown_and_drop_send_a_single_close() {
        let (table, mut rx) = setup();
        let id = Uuid::new_v4();
        let mut io = table.open(id).unwrap();
        io.shutdown().await.unwrap();
        drop(io);
        let Some(Message::Text(text)) = rx.ordered.recv().await else {
            panic!("expected a close");
        };
        assert!(text.as_str().contains("stream_close"));
        assert!(rx.ordered.try_recv().is_err(), "close is sent once");
        assert_eq!(table.len(), 0);
    }

    #[tokio::test]
    async fn dropping_without_shutdown_sends_a_close_after_the_data() {
        let (table, mut rx) = setup();
        let mut io = table.open(Uuid::new_v4()).unwrap();
        io.write_all(b"bye").await.unwrap();
        drop(io);
        assert!(matches!(rx.ordered.recv().await, Some(Message::Binary(_))));
        assert!(matches!(rx.ordered.recv().await, Some(Message::Text(_))));
    }

    #[tokio::test]
    async fn the_stream_count_is_capped() {
        let (table, _rx) = setup();
        let mut held = Vec::new();
        for _ in 0..MAX_STREAMS {
            held.push(table.open(Uuid::new_v4()).unwrap());
        }
        assert_eq!(
            table.open(Uuid::new_v4()).err(),
            Some(OpenError::TooManyStreams)
        );
        held.pop();
        assert!(table.open(Uuid::new_v4()).is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn idle_streams_are_closed() {
        let (table, mut rx) = setup();
        let id = Uuid::new_v4();
        let mut io = table.open(id).unwrap();
        tokio::time::advance(Duration::from_secs(599)).await;
        table.sweep_idle();
        assert_eq!(table.len(), 1);
        tokio::time::advance(Duration::from_secs(2)).await;
        table.sweep_idle();
        assert_eq!(table.len(), 0);
        assert!(matches!(
            next_control(&mut rx).await,
            V2Frame::StreamClose { stream_id, .. } if stream_id == id
        ));
        assert!(io.read_u8().await.is_err());
    }

    #[tokio::test]
    async fn closing_the_table_fails_streams_without_telling_the_relay() {
        let (table, mut rx) = setup();
        let mut io = table.open(Uuid::new_v4()).unwrap();
        table.close_all();
        assert!(io.read_u8().await.is_err());
        assert!(rx.control.try_recv().is_err());
        assert_eq!(table.open(Uuid::new_v4()).err(), Some(OpenError::Closed));
    }
}
