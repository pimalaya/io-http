//! Tests for RFC 1945 — HTTP/1.0 message syntax.
//!
//! All tests drive [`Http10Send`] against a pre-crafted in-memory buffer
//! via [`stub::StubStream`]. No network connection is made.

mod stub;

use io_http::{
    rfc1945::send::{Http10Send, Http10SendResult},
    rfc9110::request::HttpRequest,
};
use io_socket::runtimes::std_stream::handle;
use url::Url;

use crate::stub::StubStream;

fn test(response: &[u8]) -> Http10SendResult {
    let mut stream = StubStream::new(response);

    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url).header("Host", "example.com");

    let mut send = Http10Send::new(request);
    let mut arg = None;

    loop {
        match send.resume(arg.take()) {
            Http10SendResult::Io { input } => arg = Some(handle(&mut stream, input).unwrap()),
            any => return any,
        }
    }
}

#[test]
fn http10_200_ok() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 5\r\n\r\nhello";

    match test(response) {
        Http10SendResult::Ok { response, .. } => assert_eq!(*response.status, 200),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn http10_version() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http10SendResult::Ok { response, .. } => assert_eq!(response.version, "HTTP/1.0"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_content_length() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 11\r\n\r\nhello world";

    match test(response) {
        Http10SendResult::Ok { response, .. } => assert_eq!(response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_read_to_eof() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\nhello world";

    match test(response) {
        Http10SendResult::Ok { response, .. } => assert_eq!(response.body, b"hello world"),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_empty_on_204() {
    let response = b"HTTP/1.0 204 No Content\r\n\r\n";

    match test(response) {
        Http10SendResult::Ok { response, .. } => {
            assert_eq!(*response.status, 204);
            assert!(response.body.is_empty());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn body_empty_on_304() {
    let response = b"HTTP/1.0 304 Not Modified\r\n\r\n";

    match test(response) {
        Http10SendResult::Ok { response, .. } => {
            assert_eq!(*response.status, 304);
            assert!(response.body.is_empty());
        }
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_false_by_default() {
    let response = b"HTTP/1.0 200 OK\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http10SendResult::Ok { keep_alive, .. } => assert!(!keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn keep_alive_true_on_connection_keep_alive() {
    let response = b"HTTP/1.0 200 OK\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http10SendResult::Ok { keep_alive, .. } => assert!(keep_alive),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_301_emits_redirect_result() {
    let response =
        b"HTTP/1.0 301 Moved Permanently\r\nLocation: http://example.com/new\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http10SendResult::Redirect { url, response, .. } => {
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
        Http10SendResult::Redirect { same_origin, .. } => assert!(same_origin),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_cross_origin_different_host() {
    let response =
        b"HTTP/1.0 302 Found\r\nLocation: http://other.com/\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http10SendResult::Redirect { same_origin, .. } => assert!(!same_origin),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn redirect_without_location_falls_through_to_ok() {
    let response = b"HTTP/1.0 301 Moved Permanently\r\nContent-Length: 0\r\n\r\n";

    match test(response) {
        Http10SendResult::Ok { response, .. } => assert_eq!(*response.status, 301),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn err_on_malformed_headers() {
    let response = b"NOT HTTP AT ALL\r\n\r\n";

    match test(response) {
        Http10SendResult::Err { .. } => {}
        other => panic!("expected Err, got: {other:?}"),
    }
}
