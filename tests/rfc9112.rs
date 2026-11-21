//! Tests for RFC 9112: HTTP/1.1 message syntax.
//!
//! All tests drive [`Http11Send`] against a pre-crafted in-memory
//! response buffer. No network connection is made.

#![cfg(feature = "client")]

use io_http::{
    coroutine::*,
    rfc9110::{
        request::{HttpRequest, HttpRequestError},
        send::*,
    },
    rfc9112::{
        chunk::{Http11ChunksRead, Http11ChunksReadError},
        chunk_stream::{
            Http11ChunksReadStream, Http11ChunksReadStreamError, Http11ChunksReadStreamYield,
        },
        read_headers::HTTP11_MAX_INTERIM_RESPONSES,
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

    // NOTE: EOF terminates the body when neither Content-Length nor
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

    // NOTE: no Content-Length, so the body falls back to read-to-EOF;
    // drive an empty read after the response bytes.
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
        // NOTE: the body must be the raw wire bytes, not the decoded
        // chunk payload.
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
    let mut chunks = Http11ChunksRead::default();

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
    // NOTE: 0x0a = 10 bytes
    assert_eq!(test_chunks(b"a\r\n0123456789\r\n0\r\n\r\n"), b"0123456789");
}

/// Drives `send` against `response` once, then reports EOF.
fn test_once(mut send: Http11Send, response: &[u8]) -> Step {
    let mut arg: Option<&[u8]> = None;
    let mut sent = false;

    loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => arg = None,
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) if !sent => {
                sent = true;
                arg = Some(response);
            }
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => arg = Some(b""),
            any => return any,
        }
    }
}

fn new_send() -> Http11Send {
    let url = Url::parse("http://example.com/").unwrap();
    Http11Send::new(HttpRequest::get(url).header("Host", "example.com"))
}

#[test]
fn err_on_max_usize_chunk_size() {
    let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nFFFFFFFFFFFFFFFF\r\n";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Err(Http11SendError::ReadChunks(
            Http11ChunksReadError::BodyTooLarge(_),
        ))) => {}
        other => panic!("expected BodyTooLarge, got: {other:?}"),
    }

    // NOTE: even an unbounded limit must not overflow `size + CRLF`.
    let mut send = new_send();
    send.max_body_len = usize::MAX;

    match test_once(send, response) {
        HttpCoroutineState::Complete(Err(Http11SendError::ReadChunks(
            Http11ChunksReadError::BodyTooLarge(_),
        ))) => {}
        other => panic!("expected BodyTooLarge, got: {other:?}"),
    }
}

#[test]
fn err_on_chunked_body_over_limit() {
    let response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
    let mut send = new_send();
    send.max_body_len = 8;

    match test_once(send, response) {
        HttpCoroutineState::Complete(Err(Http11SendError::ReadChunks(
            Http11ChunksReadError::BodyTooLarge(8),
        ))) => {}
        other => panic!("expected BodyTooLarge, got: {other:?}"),
    }
}

#[test]
fn err_on_headers_over_limit() {
    let response = b"HTTP/1.1 200 OK\r\nX-Filler: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n";
    let mut send = new_send();
    send.max_headers_len = 64;

    match test_once(send, response) {
        HttpCoroutineState::Complete(Err(Http11SendError::HeadersTooLarge(64))) => {}
        other => panic!("expected HeadersTooLarge, got: {other:?}"),
    }
}

#[test]
fn err_on_content_length_over_limit() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello world";
    let mut send = new_send();
    send.max_body_len = 5;

    match test_once(send, response) {
        HttpCoroutineState::Complete(Err(Http11SendError::BodyTooLarge(5))) => {}
        other => panic!("expected BodyTooLarge, got: {other:?}"),
    }
}

#[test]
fn err_on_content_length_overflow() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999\r\n\r\n";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Err(Http11SendError::InvalidContentLength(_))) => {}
        other => panic!("expected InvalidContentLength, got: {other:?}"),
    }
}

#[test]
fn err_on_read_to_eof_body_over_limit() {
    let response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nhello world";
    let mut send = new_send();
    send.max_body_len = 5;

    match test_once(send, response) {
        HttpCoroutineState::Complete(Err(Http11SendError::BodyTooLarge(5))) => {}
        other => panic!("expected BodyTooLarge, got: {other:?}"),
    }
}

#[test]
fn err_on_eof_before_content_length() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Err(Http11SendError::Eof)) => {}
        other => panic!("expected Eof, got: {other:?}"),
    }
}

#[test]
fn err_on_eof_before_last_chunk() {
    let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Err(Http11SendError::ReadChunks(
            Http11ChunksReadError::Eof,
        ))) => {}
        other => panic!("expected Eof, got: {other:?}"),
    }
}

#[test]
fn chunks_trailers_consumed() {
    let mut chunks = Http11ChunksRead::default();

    match chunks.resume(Some(b"5\r\nhello\r\n0\r\nX-T: 1\r\nX-U: 2\r\n\r\nNEXT")) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(out.body, b"hello");
            assert_eq!(out.remaining, b"NEXT");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_chunk_data_without_crlf() {
    let mut chunks = Http11ChunksRead::default();

    match chunks.resume(Some(b"5\r\nhelloXX0\r\n\r\n")) {
        HttpCoroutineState::Complete(Err(Http11ChunksReadError::InvalidChunkTerminator)) => {}
        other => panic!("expected InvalidChunkTerminator, got: {other:?}"),
    }
}

#[test]
fn chunks_trailers_at_limit() {
    let mut chunks = Http11ChunksRead::default();
    chunks.max_trailers_len = 8;

    match chunks.resume(Some(b"0\r\nX-T: 1\r\nX:\r\n\r\n")) {
        HttpCoroutineState::Complete(Ok(out)) => assert!(out.remaining.is_empty()),
        other => panic!("unexpected result: {other:?}"),
    }

    let mut chunks = Http11ChunksReadStream::default();
    chunks.max_trailers_len = 8;

    match chunks.resume(Some(b"0\r\nX-T: 1\r\nX:\r\n\r\n")) {
        HttpCoroutineState::Complete(Ok(remaining)) => assert!(remaining.is_empty()),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_trailers_over_limit() {
    let mut chunks = Http11ChunksRead::default();
    chunks.max_trailers_len = 8;

    match chunks.resume(Some(b"0\r\nX-T: 1\r\nX:2\r\n\r\n")) {
        HttpCoroutineState::Complete(Err(Http11ChunksReadError::TrailersTooLarge(8))) => {}
        other => panic!("expected TrailersTooLarge, got: {other:?}"),
    }

    let mut chunks = Http11ChunksReadStream::default();
    chunks.max_trailers_len = 8;

    match chunks.resume(Some(b"0\r\nX-T: 1\r\nX:2\r\n\r\n")) {
        HttpCoroutineState::Complete(Err(Http11ChunksReadStreamError::TrailersTooLarge(8))) => {}
        other => panic!("expected TrailersTooLarge, got: {other:?}"),
    }
}

/// Feeds `encoded` once to the streaming decoder, then reports EOF.
fn test_chunk_stream(
    encoded: &[u8],
) -> (Vec<Vec<u8>>, Result<Vec<u8>, Http11ChunksReadStreamError>) {
    let mut chunks = Http11ChunksReadStream::default();
    let mut arg = Some(encoded);
    let mut frames = Vec::new();

    loop {
        match chunks.resume(arg.take()) {
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::Frame { body }) => {
                frames.push(body)
            }
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::WantsRead) => arg = Some(b""),
            HttpCoroutineState::Complete(end) => return (frames, end),
        }
    }
}

#[test]
fn chunk_stream_trailers_consumed() {
    let (frames, end) = test_chunk_stream(b"5\r\nhello\r\n0\r\nX-T: 1\r\nX-U: 2\r\n\r\nNEXT");

    assert_eq!(frames, [b"hello"]);
    assert_eq!(end.unwrap(), b"NEXT");
}

#[test]
fn err_on_chunk_stream_data_without_crlf() {
    let (frames, end) = test_chunk_stream(b"5\r\nhelloXX0\r\n\r\n");

    assert!(frames.is_empty(), "got {frames:?}");
    assert!(
        matches!(
            end,
            Err(Http11ChunksReadStreamError::InvalidChunkTerminator)
        ),
        "got {end:?}"
    );
}

#[test]
fn interim_responses_skipped() {
    let response = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 103 Early Hints\r\nLink: </s.css>\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(*out.response.status, 200);
            assert_eq!(out.response.header("link"), None);
            assert_eq!(out.response.body, b"ok");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_empty_on_head() {
    let url = Url::parse("http://example.com/").unwrap();
    let mut request = HttpRequest::get(url).header("Host", "example.com");
    request.method = "HEAD".into();

    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 1234\r\n\r\n";

    match test_once(Http11Send::new(request), response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert!(out.response.body.is_empty());
            assert!(out.keep_alive);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_chunked_as_final_transfer_coding() {
    let response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"hello"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_chunked_across_transfer_encoding_lines() {
    let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\nTransfer-Encoding: Chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"hello"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_final_transfer_coding_not_chunked() {
    let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Err(Http11SendError::UnsupportedTransferCoding(coding))) => {
            assert_eq!(coding, "gzip")
        }
        other => panic!("expected UnsupportedTransferCoding, got: {other:?}"),
    }
}

#[test]
fn switching_protocols_ends_at_head() {
    let response = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: other\r\n\r\nRAW";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(*out.response.status, 101);
            assert!(out.response.body.is_empty());
            assert_eq!(out.remaining, b"RAW");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_endless_interim_responses() {
    let mut send = new_send();
    let mut arg: Option<&[u8]> = None;

    for _ in 0..1000 {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => {}
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                arg = Some(b"HTTP/1.1 102 Processing\r\n\r\n")
            }
            HttpCoroutineState::Complete(Err(Http11SendError::TooManyInterimResponses(
                HTTP11_MAX_INTERIM_RESPONSES,
            ))) => return,
            other => panic!("unexpected result: {other:?}"),
        }
    }

    panic!("still reading after 1000 interim responses");
}

#[test]
fn interim_responses_at_limit() {
    let mut response = b"HTTP/1.1 102 Processing\r\n\r\n".repeat(HTTP11_MAX_INTERIM_RESPONSES);
    response.extend(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");

    match test_once(new_send(), &response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"ok"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_endless_trailers() {
    let mut send = new_send();
    let mut arg: Option<&[u8]> = None;
    let mut head = Some(&b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n"[..]);

    for _ in 0..10_000 {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => {}
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                arg = Some(head.take().unwrap_or(b"X-Trailer: 0123456789abcdef\r\n"))
            }
            HttpCoroutineState::Complete(Err(Http11SendError::ReadChunks(
                Http11ChunksReadError::TrailersTooLarge(_),
            ))) => return,
            other => panic!("unexpected result: {other:?}"),
        }
    }

    panic!("still reading after 10000 trailer fields");
}

#[test]
fn err_on_endless_chunk_stream_trailers() {
    let mut chunks = Http11ChunksReadStream::default();
    let mut arg = Some(&b"0\r\n"[..]);

    for _ in 0..10_000 {
        match chunks.resume(arg.take()) {
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::WantsRead) => {
                arg = Some(b"X-Trailer: 0123456789abcdef\r\n")
            }
            HttpCoroutineState::Complete(Err(Http11ChunksReadStreamError::TrailersTooLarge(_))) => {
                return;
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    panic!("still reading after 10000 trailer fields");
}

#[test]
fn body_content_length_on_lone_identity_transfer_coding() {
    let response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: Identity\r\nContent-Length: 5\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(out.response.body, b"hello");
            assert!(out.keep_alive);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

/// Starts the exchange of a request whose head is under test.
fn test_request(method: &str, headers: &[(&str, &str)]) -> Step {
    let url = Url::parse("http://example.com/").unwrap();
    let mut request = HttpRequest::get(url);
    request.method = method.into();

    for (name, value) in headers {
        request = request.header(name, value);
    }

    Http11Send::new(request).resume(None)
}

#[test]
fn err_on_header_value_with_line_break_or_nul() {
    for value in ["1\r\nX-Injected: 2", "1\nX-Injected: 2", "1\rX", "1\0"] {
        match test_request("GET", &[("X-A", value)]) {
            HttpCoroutineState::Complete(Err(Http11SendError::InvalidRequest(
                HttpRequestError::InvalidHeaderValue(name),
            ))) => assert_eq!(name, "X-A"),
            other => panic!("expected InvalidHeaderValue for {value:?}, got: {other:?}"),
        }
    }
}

#[test]
fn err_on_header_name_not_token() {
    for name in ["", "X A", "X-A:", "X-A\r\nX-Injected", "X-\u{e9}"] {
        match test_request("GET", &[(name, "1")]) {
            HttpCoroutineState::Complete(Err(Http11SendError::InvalidRequest(
                HttpRequestError::InvalidHeaderName(got),
            ))) => assert_eq!(got, name),
            other => panic!("expected InvalidHeaderName for {name:?}, got: {other:?}"),
        }
    }
}

#[test]
fn err_on_method_not_token() {
    for method in ["", "GET /x HTTP/1.1\r\nX-Injected: 2\r\n\r\nGET", "G ET"] {
        match test_request(method, &[]) {
            HttpCoroutineState::Complete(Err(Http11SendError::InvalidRequest(
                HttpRequestError::InvalidMethod(got),
            ))) => assert_eq!(got, method),
            other => panic!("expected InvalidMethod for {method:?}, got: {other:?}"),
        }
    }
}

#[test]
fn err_on_request_transfer_encoding() {
    match test_request("POST", &[("Transfer-Encoding", "chunked")]) {
        HttpCoroutineState::Complete(Err(Http11SendError::InvalidRequest(
            HttpRequestError::UnsupportedTransferEncoding,
        ))) => {}
        other => panic!("expected UnsupportedTransferEncoding, got: {other:?}"),
    }
}

#[test]
fn request_head_of_tokens_is_written() {
    let headers = [("X-!#$%&'*+-.^_`|~9", "a\tb \u{e9}"), ("Empty", "")];

    match test_request("M-SEARCH", &headers) {
        HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
            let head = String::from_utf8(bytes).unwrap();
            assert!(head.starts_with("M-SEARCH / HTTP/1.1\r\n"));
            assert!(head.contains("\r\nX-!#$%&'*+-.^_`|~9: a\tb \u{e9}\r\nEmpty: \r\n"));
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_final_transfer_coding_identity_after_another() {
    let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, identity\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Err(Http11SendError::UnsupportedTransferCoding(coding))) => {
            assert_eq!(coding, "identity")
        }
        other => panic!("expected UnsupportedTransferCoding, got: {other:?}"),
    }
}

#[test]
fn keep_alive_false_after_read_to_eof() {
    let response = b"HTTP/1.1 200 OK\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(out.response.body, b"hello");
            assert!(!out.keep_alive);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_false_on_close_among_connection_options() {
    for response in [
        &b"HTTP/1.1 200 OK\r\nConnection: TE, Close\r\nContent-Length: 0\r\n\r\n"[..],
        &b"HTTP/1.1 200 OK\r\nConnection: TE\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"[..],
    ] {
        match test_once(new_send(), response) {
            HttpCoroutineState::Complete(Ok(out)) => assert!(!out.keep_alive),
            other => panic!("unexpected result: {other:?}"),
        }
    }
}

#[test]
fn keep_alive_on_http10_needs_keep_alive_option() {
    let response = b"HTTP/1.0 200 OK\r\nConnection: TE\r\nContent-Length: 0\r\n\r\n";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => assert!(!out.keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_conflicting_content_length() {
    for response in [
        &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 5\r\n\r\nhello"[..],
        &b"HTTP/1.1 200 OK\r\nContent-Length: 2, 5\r\n\r\nhello"[..],
    ] {
        match test_once(new_send(), response) {
            HttpCoroutineState::Complete(Err(Http11SendError::InvalidContentLength(len))) => {
                assert_eq!(len, "2, 5")
            }
            other => panic!("expected InvalidContentLength, got: {other:?}"),
        }
    }
}

#[test]
fn body_content_length_repeated() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 5, 5\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"hello"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn head_with_more_than_64_headers() {
    let mut response = b"HTTP/1.1 200 OK\r\n".to_vec();
    for n in 0..200 {
        response.extend(format!("X-{n}: v\r\n").bytes());
    }
    response.extend(b"Content-Length: 2\r\n\r\nok");

    match test_once(new_send(), &response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(out.response.headers.len(), 201);
            assert_eq!(out.response.body, b"ok");
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

/// Runs an exchange over `reads` then EOF, returns how it ended and the
/// body. The bytes left past the response are kept out, they depend on
/// where the reads stop.
fn outcome(reads: &[&[u8]]) -> (String, Vec<u8>) {
    let mut send = new_send();
    let mut reads = reads.iter();
    let mut eofs = 0;
    let mut arg: Option<&[u8]> = None;

    loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => {}
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => match reads.next() {
                Some(read) => arg = Some(read),
                None => {
                    eofs += 1;
                    assert!(eofs <= 2, "spins on EOF");
                    arg = Some(b"");
                }
            },
            HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                url,
                response,
                keep_alive,
                same_origin,
            }) => {
                let end = format!("redirect {url} {response:?} {keep_alive} {same_origin}");
                return (end, response.body);
            }
            HttpCoroutineState::Complete(Ok(out)) => {
                let end = format!("ok {:?} {}", out.response, out.keep_alive);
                return (end, out.response.body);
            }
            HttpCoroutineState::Complete(Err(err)) => return (format!("err {err}"), Vec::new()),
        }
    }
}

fn responses() -> Vec<Vec<u8>> {
    let big = "x".repeat(70_000);

    let mut many_headers = b"HTTP/1.1 200 OK\r\n".to_vec();
    for n in 0..200 {
        many_headers.extend(format!("X-{n}: v\r\n").bytes());
    }
    many_headers.extend(b"Content-Length: 2\r\n\r\nok");

    let fixed: [&[u8]; 37] = [
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhello",
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;a=b\r\nhello\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked\r\n\r\nA\r\n0123456789\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\nX-T: 1\r\nX-U: 2\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhelloXX0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nX-T: 1\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked, gzip\r\n\r\nhello",
        b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nbody until close",
        b"HTTP/1.1 200 OK\r\nConnection: TE, close\r\nContent-Length: 2\r\n\r\nok",
        b"HTTP/1.1 200 OK\r\n\r\nbody until close",
        b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 2\r\n\r\nok",
        b"HTTP/1.1 204 No Content\r\n\r\n",
        b"HTTP/1.1 304 Not Modified\r\nETag: \"a\"\r\n\r\n",
        b"HTTP/1.1 302 Found\r\nLocation: https://other.example/x\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 301 Moved\r\nLocation: /c\r\nContent-Length: 2\r\n\r\nhi",
        b"HTTP/1.1 500 Oops\r\nContent-Length: 3\r\n\r\nerr",
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nokHTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nx",
        b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel",
        b"\x00\xff\r\n\r\nnot http at all",
        b"HTTP/1.1 200 OK\nContent-Length: 2\n\nok",
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 5\r\n\r\nhello",
        b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\nhello",
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
        b"HTTP/1.1 103 Early Hints\r\nLink: </s.css>\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok",
        b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 102 Processing\r\n\r\n",
        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: other\r\n\r\nRAW",
        b"HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\nx",
        b"HTTP/1.1 200 OK\r\nContent-Length: 99999999999999999999999\r\n\r\nx",
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffffff\r\nx\r\n",
        b"HTTP/1.1 200 OK\r\nBroken header\r\nContent-Length: 0\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nX-A: 1\r\n continued\r\nContent-Length: 0\r\n\r\n",
        &many_headers,
    ];

    let mut responses: Vec<Vec<u8>> = fixed.iter().map(|response| response.to_vec()).collect();

    responses.push(
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{big}",
            big.len()
        )
        .into_bytes(),
    );
    responses.push(
        format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{big}\r\n0\r\n\r\n",
            big.len()
        )
        .into_bytes(),
    );

    responses
}

#[test]
fn same_outcome_however_the_response_is_split() {
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };

    for response in responses() {
        let whole = outcome(&[&response]);
        let name = String::from_utf8_lossy(&response[..response.len().min(60)]).into_owned();

        let bytes: Vec<&[u8]> = response.chunks(1).collect();
        assert_eq!(outcome(&bytes), whole, "{name:?} byte by byte");

        let step = (response.len() / 2000).max(1);
        for cut in (1..response.len()).step_by(step) {
            let (head, tail) = response.split_at(cut);
            assert_eq!(outcome(&[head, tail]), whole, "{name:?} cut at {cut}");
        }

        let rounds = if response.len() > 10_000 { 40 } else { 500 };
        for _ in 0..rounds {
            let mut reads: Vec<&[u8]> = Vec::new();
            let mut rest = response.as_slice();

            while !rest.is_empty() {
                let max = rest.len().min(1 + random() % 97);
                let (head, tail) = rest.split_at(1 + random() % max);
                reads.push(head);
                rest = tail;
            }

            assert_eq!(outcome(&reads), whole, "{name:?} split as {reads:?}");
        }
    }
}
