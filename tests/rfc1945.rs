//! Tests for RFC 1945: HTTP/1.0 message syntax.
//!
//! All tests drive [`Http10Send`] against a pre-crafted in-memory
//! response buffer. No network connection is made.

use io_http::{
    coroutine::*,
    rfc1945::send::{Http10Send, Http10SendError},
    rfc9110::{request::HttpRequest, send::*},
};
use url::Url;

type Step = HttpCoroutineState<HttpSendYield, Result<HttpSendOutput, Http10SendError>>;

fn test(response: &'static [u8]) -> Step {
    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url).header("Host", "example.com");

    let mut send = Http10Send::new(request);
    let mut arg: Option<&[u8]> = None;
    let mut sent = false;

    loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => arg = None,
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) if !sent => {
                sent = true;
                arg = Some(response);
            }
            // NOTE: after the response, signal EOF so a read-to-EOF
            // body strategy can terminate.
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => arg = Some(b""),
            any => return any,
        }
    }
}

#[test]
fn http10_200_ok() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\n\r\nhello";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(*out.response.status, 200),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn http10_version() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.version, "HTTP/1.0"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_content_length() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 11\r\n\r\nhello world";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_read_to_eof() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\nhello world";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert_eq!(out.response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_empty_on_204() {
    let response = b"HTTP/1.0 204 No Content\r\n\r\n";

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
    let response = b"HTTP/1.0 304 Not Modified\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(*out.response.status, 304);
            assert!(out.response.body.is_empty());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_false_by_default() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert!(!out.keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_true_on_connection_keep_alive() {
    let response = b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Complete(Ok(out)) => assert!(out.keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_301_emits_redirect_yield() {
    let response =
        b"HTTP/1.0 301 Moved Permanently\r\nLocation: http://example.com/new\r\nContent-Length: 0\r\n\r\n";

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
        b"HTTP/1.0 302 Found\r\nLocation: http://example.com/other\r\nContent-Length: 0\r\n\r\n";

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
        b"HTTP/1.0 302 Found\r\nLocation: http://other.com/\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { same_origin, .. }) => {
            assert!(!same_origin);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_without_location_falls_through_to_ok() {
    let response = b"HTTP/1.0 301 Moved Permanently\r\nContent-Length: 0\r\n\r\n";

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

/// Drives `send` against `response` once, then reports EOF.
fn test_once(mut send: Http10Send, response: &[u8]) -> Step {
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

fn new_send() -> Http10Send {
    let url = Url::parse("http://example.com/").unwrap();
    Http10Send::new(HttpRequest::get(url).header("Host", "example.com"))
}

#[test]
fn err_on_headers_over_limit() {
    let response = b"HTTP/1.0 200 OK\r\nX-Filler: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n";
    let mut send = new_send();
    send.max_headers_len = 64;

    match test_once(send, response) {
        HttpCoroutineState::Complete(Err(Http10SendError::HeadersTooLarge(64))) => {}
        other => panic!("expected HeadersTooLarge, got: {other:?}"),
    }
}

#[test]
fn err_on_content_length_over_limit() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 11\r\n\r\nhello world";
    let mut send = new_send();
    send.max_body_len = 5;

    match test_once(send, response) {
        HttpCoroutineState::Complete(Err(Http10SendError::BodyTooLarge(5))) => {}
        other => panic!("expected BodyTooLarge, got: {other:?}"),
    }
}

#[test]
fn err_on_read_to_eof_body_over_limit() {
    let response = b"HTTP/1.0 200 OK\r\n\r\nhello world";
    let mut send = new_send();
    send.max_body_len = 5;

    match test_once(send, response) {
        HttpCoroutineState::Complete(Err(Http10SendError::BodyTooLarge(5))) => {}
        other => panic!("expected BodyTooLarge, got: {other:?}"),
    }
}

#[test]
fn err_on_eof_before_content_length() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 11\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Err(Http10SendError::Eof)) => {}
        other => panic!("expected Eof, got: {other:?}"),
    }
}

#[test]
fn body_empty_on_head() {
    let url = Url::parse("http://example.com/").unwrap();
    let mut request = HttpRequest::get(url).header("Host", "example.com");
    request.method = "HEAD".into();

    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 1234\r\n\r\n";

    match test_once(Http10Send::new(request), response) {
        HttpCoroutineState::Complete(Ok(out)) => assert!(out.response.body.is_empty()),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_false_after_read_to_eof() {
    let response = b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Ok(out)) => {
            assert_eq!(out.response.body, b"hello");
            assert!(!out.keep_alive);
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_conflicting_content_length() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\nContent-Length: 5\r\n\r\nhello";

    match test_once(new_send(), response) {
        HttpCoroutineState::Complete(Err(Http10SendError::InvalidContentLength(len))) => {
            assert_eq!(len, "2, 5")
        }
        other => panic!("expected InvalidContentLength, got: {other:?}"),
    }
}
