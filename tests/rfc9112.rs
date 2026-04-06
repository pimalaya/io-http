//! Tests for RFC 9112 — HTTP/1.1 message syntax.
//!
//! All tests drive [`Http11Send`] against a pre-crafted in-memory
//! buffer via [`stub::StubStream`]. No network connection is made.

mod stub;

use io_http::{
    rfc9110::request::HttpRequest,
    rfc9112::{
        chunk::{HttpChunksRead, HttpChunksReadResult},
        send::{Http11Send, Http11SendResult},
    },
};
use io_socket::{coroutines::read::SocketRead, runtimes::std_stream::handle};
use url::Url;

use crate::stub::StubStream;

fn test(response: &[u8]) -> Http11SendResult {
    let mut stream = StubStream::new(response);

    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url).header("Host", "example.com");

    let mut send = Http11Send::new(request);
    let mut arg = None;

    loop {
        match send.resume(arg.take()) {
            Http11SendResult::Io { input } => arg = Some(handle(&mut stream, input).unwrap()),
            any => return any,
        }
    }
}

#[test]
fn http11_200_ok() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";

    match test(response) {
        Http11SendResult::Ok { response, .. } => assert_eq!(*response.status, 200),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn http11_version() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Ok { response, .. } => assert_eq!(response.version, "HTTP/1.1"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn http10_response_version_and_connection() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Ok {
            response,
            keep_alive,
            ..
        } => {
            assert_eq!(response.version, "HTTP/1.0");
            assert!(!keep_alive);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_content_length() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world";

    match test(response) {
        Http11SendResult::Ok { response, .. } => assert_eq!(response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_chunked() {
    let response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";

    match test(response) {
        Http11SendResult::Ok { response, .. } => assert_eq!(response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_read_to_eof() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello world";

    match test(response) {
        Http11SendResult::Ok { response, .. } => assert_eq!(response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_empty_on_204() {
    let response = b"HTTP/1.1 204 No Content\r\n\r\n";

    match test(response) {
        Http11SendResult::Ok { response, .. } => {
            assert_eq!(*response.status, 204);
            assert!(response.body.is_empty());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_empty_on_304() {
    let response = b"HTTP/1.1 304 Not Modified\r\n\r\n";

    match test(response) {
        Http11SendResult::Ok { response, .. } => {
            assert_eq!(*response.status, 304);
            assert!(response.body.is_empty());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_chunked_ignored_on_http10_response() {
    let response = b"HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";

    match test(response) {
        // Body must be the raw wire bytes, not the decoded chunk payload.
        Http11SendResult::Ok { response, .. } => assert_ne!(response.body, b"hello"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_true_by_default_on_http11() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Ok { keep_alive, .. } => assert!(keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_false_on_connection_close() {
    let response = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Ok { keep_alive, .. } => assert!(!keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

// ── Redirects ─────────────────────────────────────────────────────────────────

#[test]
fn redirect_301_emits_redirect_result() {
    let response =
        b"HTTP/1.1 301 Moved Permanently\r\nLocation: http://example.com/new\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Redirect { url, response, .. } => {
            assert_eq!(url.as_str(), "http://example.com/new");
            assert_eq!(*response.status, 301);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_same_origin() {
    let response =
        b"HTTP/1.1 302 Found\r\nLocation: http://example.com/other\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Redirect { same_origin, .. } => assert!(same_origin),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_cross_origin_different_host() {
    let response =
        b"HTTP/1.1 302 Found\r\nLocation: http://other.com/\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Redirect { same_origin, .. } => assert!(!same_origin),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_cross_origin_different_scheme() {
    let response =
        b"HTTP/1.1 302 Found\r\nLocation: https://example.com/\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Redirect { same_origin, .. } => assert!(!same_origin),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_without_location_falls_through_to_ok() {
    let response = b"HTTP/1.1 301 Moved Permanently\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http11SendResult::Ok { response, .. } => assert_eq!(*response.status, 301),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_malformed_headers() {
    let response = b"NOT HTTP AT ALL\r\n\r\n";

    match test(response) {
        Http11SendResult::Err { .. } => {}
        other => panic!("expected Err, got: {other:?}"),
    }
}

fn test_chunks(encoded: &[u8]) -> Vec<u8> {
    let mut stream = StubStream::new(encoded);
    let mut http = HttpChunksRead::new(SocketRead::default());
    let mut arg = None;

    loop {
        match http.resume(arg.take()) {
            HttpChunksReadResult::Ok { body } => return body,
            HttpChunksReadResult::Err { err } => panic!("unexpected error: {err}"),
            HttpChunksReadResult::Io { input } => arg = Some(handle(&mut stream, input).unwrap()),
        }
    }
}

/// Test case from the Russian Wikipedia page on chunked transfer
/// encoding:
/// <https://ru.wikipedia.org/wiki/Chunked_transfer_encoding>
#[test]
fn chunks_wiki_ru() {
    let encoded = concat!(
        "9\r\n",
        "chunk 1, \r\n",
        "7\r\n",
        "chunk 2\r\n",
        "0\r\n",
        "\r\n",
    );
    assert_eq!(test_chunks(encoded.as_bytes()), b"chunk 1, chunk 2");
}

/// Test case from the French Wikipedia page on chunked transfer
/// encoding:
/// <https://fr.wikipedia.org/wiki/Chunked_transfer_encoding>
#[test]
fn chunks_wiki_fr() {
    let encoded = concat!(
        "27\r\n",
        "Voici les données du premier morceau\r\n\r\n",
        "1C\r\n",
        "et voici un second morceau\r\n\r\n",
        "20\r\n",
        "et voici deux derniers morceaux \r\n",
        "12\r\n",
        "sans saut de ligne\r\n",
        "0\r\n",
        "\r\n",
    );
    let expected = concat!(
        "Voici les données du premier morceau\r\n",
        "et voici un second morceau\r\n",
        "et voici deux derniers morceaux ",
        "sans saut de ligne",
    );
    assert_eq!(test_chunks(encoded.as_bytes()), expected.as_bytes());
}

/// Test case from the frewsxcv/rust-chunked-transfer repository:
/// <https://github.com/frewsxcv/rust-chunked-transfer/blob/main/src/decoder.rs>
#[test]
fn chunks_github_frewsxcv() {
    assert_eq!(
        test_chunks(b"3\r\nhel\r\nb\r\nlo world!!!\r\n0\r\n\r\n"),
        b"hello world!!!"
    );
}

#[test]
fn chunks_single() {
    assert_eq!(test_chunks(b"5\r\nhello\r\n0\r\n\r\n"), b"hello");
}

#[test]
fn chunks_empty_body() {
    assert_eq!(test_chunks(b"0\r\n\r\n"), b"");
}

#[test]
fn chunks_extension_ignored() {
    assert_eq!(
        test_chunks(b"5;ext=ignored\r\nhello\r\n0\r\n\r\n"),
        b"hello"
    );
}

#[test]
fn chunks_size_hex() {
    // 0x0a = 10 bytes
    assert_eq!(test_chunks(b"a\r\n0123456789\r\n0\r\n\r\n"), b"0123456789");
}
