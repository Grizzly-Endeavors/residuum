//! Tests for `POST /api/hub/uploads`, against the harness's hub directory.

use super::*;
use crate::uploads::{MAX_UPLOAD_BYTES, UploadError};

/// A `POST /api/hub/uploads?filename=<encoded>` request whose body is `bytes`.
fn upload_request(query: &str, bytes: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/hub/uploads?{query}"))
        .header("content-type", "application/octet-stream")
        .body(Body::from(bytes))
        .unwrap()
}

#[tokio::test]
async fn a_staged_file_answers_its_id_and_resolves_to_its_bytes() {
    let h = Harness::new();
    let (status, body) = h
        .send_body(upload_request("filename=notes.txt", b"hello".to_vec()))
        .await;
    assert_eq!(status, StatusCode::OK);
    let reply: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(reply["filename"], "notes.txt");
    assert_eq!(reply["size"], 5);
    let id = reply["id"].as_str().unwrap();

    let staged = h.uploads.resolve(id).await.unwrap();
    assert_eq!(staged.filename, "notes.txt");
    assert_eq!(tokio::fs::read(&staged.path).await.unwrap(), b"hello");
}

#[tokio::test]
async fn a_missing_filename_is_refused() {
    let h = Harness::new();
    let (status, body) = h.send_body(upload_request("", b"hello".to_vec())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let reply: Value = serde_json::from_slice(&body).unwrap();
    assert!(reply["error"].as_str().unwrap().contains("filename"));
}

#[tokio::test]
async fn a_filename_with_a_path_is_refused() {
    let h = Harness::new();
    for query in [
        "filename=..%2F..%2Fetc%2Fpasswd",
        "filename=..",
        "filename=.",
        "filename=a%5Cb.txt",
        "filename=%20",
    ] {
        let (status, body) = h.send_body(upload_request(query, b"x".to_vec())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        let reply: Value = serde_json::from_slice(&body).unwrap();
        assert!(reply["error"].is_string(), "{query}");
    }
}

#[tokio::test]
async fn an_empty_body_is_refused() {
    let h = Harness::new();
    let reply = h
        .expect(
            Method::POST,
            "/api/hub/uploads?filename=empty.txt",
            None,
            StatusCode::BAD_REQUEST,
        )
        .await;
    assert_eq!(reply["error"], "The upload is empty.");
}

#[tokio::test]
async fn a_body_over_the_limit_is_refused_with_413() {
    let h = Harness::new();
    let (status, body) = h
        .send_body(upload_request(
            "filename=big.bin",
            vec![0_u8; MAX_UPLOAD_BYTES + 1],
        ))
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let reply: Value = serde_json::from_slice(&body).unwrap();
    assert!(reply["error"].as_str().unwrap().contains("50 MB"));
}

#[tokio::test]
async fn resolving_an_unknown_or_malformed_id_is_a_typed_error() {
    let h = Harness::new();
    let unknown = "f".repeat(32);
    assert!(matches!(
        h.uploads.resolve(&unknown).await,
        Err(UploadError::NotFound(_))
    ));
    assert!(matches!(
        h.uploads.resolve("../hub").await,
        Err(UploadError::InvalidId(_))
    ));
}
