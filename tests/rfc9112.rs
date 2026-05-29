//! Tests for RFC 9112 — HTTP/1.1 message syntax.
//!
//! All tests drive [`Http11Send`] against a pre-crafted in-memory
//! response buffer. No network connection is made.

use io_http::{
    coroutine::*,
    rfc9110::{request::HttpRequest, send::*},
    rfc9112::{
        chunk::Http11ReadChunks,
        send::{Http11Send, Http11SendError},
    },
};
use url::Url;

type Step = HttpCoroutineState<HttpSendYield, Result<HttpSendOutput, Http11SendError>>;

fn test(response: &'static [u8]) -> Step {
    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url).header("Host", "example.com");

    let mut send = Http11Send::new(request);
    let mut arg: Option<&[u8]> = None;

    loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => arg = None,
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => arg = Some(response),
            any => return any,
        }
    }
}

#[test]
fn http10_response_version_and_connection() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(out.response.version, "HTTP/1.0");
            assert!(!out.keep_alive);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_content_length() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_chunked() {
    let response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_read_to_eof() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello world";

    // EOF terminates the body when neither Content-Length nor
    // Transfer-Encoding is present, so drive an explicit empty read.
    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url).header("Host", "example.com");
    let mut send = Http11Send::new(request);
    let mut arg: Option<&[u8]> = None;
    let mut sent = false;

    let result = loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => arg = None,
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) if !sent => {
                sent = true;
                arg = Some(response);
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => arg = Some(b""),
            any => break any,
        }
    };

    match result {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_empty_on_204() {
    let response = b"HTTP/1.1 204 No Content\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(*out.response.status, 204);
            assert!(out.response.body.is_empty());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_empty_on_304() {
    let response = b"HTTP/1.1 304 Not Modified\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(*out.response.status, 304);
            assert!(out.response.body.is_empty());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_chunked_ignored_on_http10_response() {
    let response = b"HTTP/1.0 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";

    // No Content-Length → falls back to read-to-EOF; drive an empty
    // read after the response bytes.
    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url).header("Host", "example.com");
    let mut send = Http11Send::new(request);
    let mut arg: Option<&[u8]> = None;
    let mut sent = false;

    let result = loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => arg = None,
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) if !sent => {
                sent = true;
                arg = Some(response);
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => arg = Some(b""),
            any => break any,
        }
    };

    match result {
        // Body must be the raw wire bytes, not the decoded chunk payload.
        HttpCoroutineState::Complete(Ok(out)) => assert_ne!(out.response.body, b"hello"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_true_by_default_on_http11() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert!(out.keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_false_on_connection_close() {
    let response = b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert!(!out.keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

// ── Redirects ─────────────────────────────────────────────────────────────────

#[test]
fn redirect_301_emits_redirect_yield() {
    let response =
        b"HTTP/1.1 301 Moved Permanently\r\nLocation: http://example.com/new\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { url, response, .. }) => {
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
        HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { same_origin, .. }) => {
            assert!(same_origin);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_cross_origin_different_host() {
    let response =
        b"HTTP/1.1 302 Found\r\nLocation: http://other.com/\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { same_origin, .. }) => {
            assert!(!same_origin);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_cross_origin_different_scheme() {
    let response =
        b"HTTP/1.1 302 Found\r\nLocation: https://example.com/\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { same_origin, .. }) => {
            assert!(!same_origin);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_without_location_falls_through_to_ok() {
    let response = b"HTTP/1.1 301 Moved Permanently\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(*out.response.status, 301),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_malformed_headers() {
    let response = b"NOT HTTP AT ALL\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Err(_)) => {}
        other => panic!("expected Err, got: {other:?}"),
    }
}

fn test_chunks(encoded: &[u8]) -> Vec<u8> {
    let mut chunks = Http11ReadChunks::default();

    match chunks.resume(Some(encoded)) {
        HttpCoroutineState::Complete(Ok(out)) => out.body,
        HttpCoroutineState::Yielded(HttpYield::WantsRead) => panic!("unexpected WantsRead"),
        HttpCoroutineState::Yielded(HttpYield::WantsWrite(_)) => panic!("unexpected WantsWrite"),
        HttpCoroutineState::Complete(Err(err)) => panic!("unexpected error: {err}"),
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
