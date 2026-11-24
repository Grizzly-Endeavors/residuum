//! A scripted HTTP server for the streaming tests. `wiremock` sends a body in
//! one piece, but how a streamed body is chunked, when it stalls and where it
//! breaks is exactly what the streaming code has to survive.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::{StreamDelta, StreamSink};
use residuum_core::testing::gate::GateEntry;
use residuum_core::util::spawn_in_span;

#[derive(Default)]
pub(crate) struct RecordingSink(Mutex<Vec<StreamDelta>>);

impl RecordingSink {
    pub(crate) fn deltas(&self) -> Vec<StreamDelta> {
        self.0.lock().unwrap().clone()
    }

    pub(crate) fn text(&self) -> String {
        self.kept(|delta| match delta {
            StreamDelta::Text(text) => Some(text.as_str()),
            StreamDelta::Thinking(_) | StreamDelta::Restart => None,
        })
    }

    pub(crate) fn thinking(&self) -> String {
        self.kept(|delta| match delta {
            StreamDelta::Thinking(text) => Some(text.as_str()),
            StreamDelta::Text(_) | StreamDelta::Restart => None,
        })
    }

    fn kept(&self, pick: impl Fn(&StreamDelta) -> Option<&str>) -> String {
        let deltas = self.deltas();
        let from = deltas
            .iter()
            .rposition(|delta| *delta == StreamDelta::Restart)
            .map_or(0, |i| i + 1);
        deltas.iter().skip(from).filter_map(pick).collect()
    }
}

impl StreamSink for RecordingSink {
    fn push(&self, delta: StreamDelta) {
        self.0.lock().unwrap().push(delta);
    }
}

pub(crate) enum Step {
    Write(Vec<u8>),
    Pause(Duration),
    Hold(GateEntry),
}

impl Step {
    pub(crate) fn head(status: u16, content_type: &str) -> Self {
        Self::Write(
            format!(
                "HTTP/1.1 {status} Scripted\r\ncontent-type: {content_type}\r\n\
                 transfer-encoding: chunked\r\nconnection: close\r\n\r\n"
            )
            .into_bytes(),
        )
    }

    pub(crate) fn chunk(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        let mut framed = format!("{:x}\r\n", bytes.len()).into_bytes();
        framed.extend_from_slice(bytes);
        framed.extend_from_slice(b"\r\n");
        Self::Write(framed)
    }

    pub(crate) fn end() -> Self {
        Self::Write(b"0\r\n\r\n".to_vec())
    }

    pub(crate) fn pause(duration: Duration) -> Self {
        Self::Pause(duration)
    }
}

pub(crate) fn sse_response<T: AsRef<[u8]>>(chunks: &[T]) -> Vec<Step> {
    let mut script = sse_chunks(chunks);
    script.push(Step::end());
    script
}

pub(crate) fn sse_chunks<T: AsRef<[u8]>>(chunks: &[T]) -> Vec<Step> {
    let mut script = vec![Step::head(200, "text/event-stream")];
    for chunk in chunks {
        script.push(Step::chunk(chunk));
        script.push(Step::pause(Duration::from_millis(2)));
    }
    script
}

pub(crate) fn ndjson_response<T: AsRef<[u8]>>(lines: &[T]) -> Vec<Step> {
    let mut script = vec![Step::head(200, "application/x-ndjson")];
    for line in lines {
        script.push(Step::chunk(line));
        script.push(Step::pause(Duration::from_millis(2)));
    }
    script.push(Step::end());
    script
}

pub(crate) fn split_bytes(text: &str, size: usize) -> Vec<Vec<u8>> {
    text.as_bytes().chunks(size).map(<[u8]>::to_vec).collect()
}

pub(crate) fn at<'a>(value: &'a serde_json::Value, pointer: &str) -> &'a serde_json::Value {
    value
        .pointer(pointer)
        .unwrap_or_else(|| panic!("no {pointer} in {value}"))
}

pub(crate) fn assert_same_response(
    streamed: &super::InferenceResponse,
    whole: &super::InferenceResponse,
) {
    assert_eq!(
        format!("{streamed:?}"),
        format!("{whole:?}"),
        "the streaming and non-streaming paths must produce the same response"
    );
}

pub(crate) fn json_response(status: u16, body: &str) -> Vec<Step> {
    vec![
        Step::head(status, "application/json"),
        Step::chunk(body),
        Step::end(),
    ]
}

#[derive(Debug, Clone)]
pub(crate) struct RecordedRequest {
    pub(crate) target: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

impl RecordedRequest {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

pub(crate) struct ScriptedServer {
    uri: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl ScriptedServer {
    pub(crate) async fn start(responses: Vec<Vec<Step>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let uri = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        spawn_in_span(async move {
            let mut scripts = responses.into_iter();
            while let Ok((socket, _)) = listener.accept().await {
                let script = scripts.next();
                spawn_in_span(serve(socket, script, Arc::clone(&recorded)));
            }
        });
        Self { uri, requests }
    }

    pub(crate) fn uri(&self) -> String {
        self.uri.clone()
    }

    pub(crate) fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

async fn serve(
    mut socket: TcpStream,
    script: Option<Vec<Step>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
) {
    if let Some(request) = read_request(&mut socket).await {
        requests.lock().unwrap().push(request);
    }
    let script = script.unwrap_or_else(|| json_response(500, r#"{"error":"script exhausted"}"#));
    for step in script {
        match step {
            Step::Write(bytes) => {
                if socket.write_all(&bytes).await.is_err() || socket.flush().await.is_err() {
                    return;
                }
            }
            Step::Pause(duration) => tokio::time::sleep(duration).await,
            Step::Hold(entry) => entry.pass().await,
        }
    }
    if socket.shutdown().await.is_err() {}
}

async fn read_request(socket: &mut TcpStream) -> Option<RecordedRequest> {
    let mut received = Vec::new();
    let mut buf = [0_u8; 4096];
    let head_end = loop {
        if let Some(pos) = received.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        let n = socket.read(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        received.extend_from_slice(buf.get(..n)?);
    };
    let head = String::from_utf8_lossy(received.get(..head_end)?).into_owned();
    let mut lines = head.lines();
    let target = lines.next()?.split_whitespace().nth(1)?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
        .collect();
    let length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    while received.len() < head_end + length {
        let n = socket.read(&mut buf).await.ok()?;
        if n == 0 {
            break;
        }
        received.extend_from_slice(buf.get(..n)?);
    }
    let body = String::from_utf8_lossy(received.get(head_end..)?).into_owned();
    Some(RecordedRequest {
        target,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use residuum_core::testing::gate::Gate;
    use residuum_core::testing::wait;

    #[tokio::test]
    async fn a_held_step_sends_nothing_more_until_released() {
        let gate = Gate::closed("stream");
        let server = ScriptedServer::start(vec![vec![
            Step::head(200, "text/plain"),
            Step::chunk("before"),
            Step::Hold(gate.entry()),
            Step::chunk(" after"),
            Step::end(),
        ]])
        .await;
        let uri = server.uri();
        let body = residuum_core::util::spawn_in_span(async move {
            reqwest::get(uri).await.unwrap().text().await.unwrap()
        });
        gate.until_held(1).await;
        assert!(!body.is_finished(), "the held body hasn't ended");
        gate.release(1);
        let body = wait::guarded("the released body", body).await.unwrap();
        assert_eq!(body, "before after");
    }
}
