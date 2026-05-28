//! Tests for the W3C Server-Sent Events client.
//!
//! Drives [`HttpClientStd::send_streaming`] against a mock
//! `Read + Write` stream that replays a pre-crafted response.

#![cfg(feature = "client")]

use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};

use io_http::{
    client::{HttpClientStd, HttpClientStdError, SseStream},
    rfc9110::request::HttpRequest,
};
use url::Url;

#[derive(Clone)]
struct MockStream {
    inner: Arc<Mutex<MockInner>>,
}

struct MockInner {
    read_buf: Vec<u8>,
    read_pos: usize,
    written: Vec<u8>,
}

impl MockStream {
    fn new(response: &[u8]) -> Self {
        Self {
            inner: Arc::new(Mutex::new(MockInner {
                read_buf: response.to_vec(),
                read_pos: 0,
                written: Vec::new(),
            })),
        }
    }

    fn written(&self) -> Vec<u8> {
        self.inner.lock().unwrap().written.clone()
    }
}

impl Read for MockStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap();
        let available = inner.read_buf.len() - inner.read_pos;
        if available == 0 {
            return Ok(0);
        }
        let n = buf.len().min(available);
        buf[..n].copy_from_slice(&inner.read_buf[inner.read_pos..inner.read_pos + n]);
        inner.read_pos += n;
        Ok(n)
    }
}

impl Write for MockStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.lock().unwrap().written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn request() -> HttpRequest {
    HttpRequest::get(Url::parse("http://example.com/sse").unwrap())
        .header("Accept", "text/event-stream")
}

fn open(response: &[u8]) -> (MockStream, Result<SseStream, HttpClientStdError>) {
    let stream = MockStream::new(response);
    let client = HttpClientStd::new(stream.clone());
    let result = client.send_streaming(request());
    (stream, result)
}

#[test]
fn writes_request_and_parses_headers() {
    let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n";
    let (mock, sse) = open(response);
    let sse = sse.expect("send_streaming");

    assert_eq!(*sse.response().status, 200);
    assert_eq!(
        sse.response().header("content-type"),
        Some("text/event-stream")
    );
    assert!(sse.keep_alive());

    let written = mock.written();
    let written = std::str::from_utf8(&written).unwrap();
    assert!(written.starts_with("GET /sse HTTP/1.1\r\n"));
    assert!(written.contains("Accept: text/event-stream"));
}

#[test]
fn rejects_non_chunked_response() {
    let response =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 0\r\n\r\n";
    let (_mock, sse) = open(response);

    match sse {
        Err(HttpClientStdError::StreamingNotChunked(code)) => assert_eq!(code, 200),
        Err(err) => panic!("unexpected error: {err:?}"),
        Ok(_) => panic!("expected StreamingNotChunked"),
    }
}

#[test]
fn single_event_frame() {
    let body = "13\r\ndata: hello world\n\n\r\n0\r\n\r\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
    );
    let (_mock, sse) = open(response.as_bytes());
    let mut sse = sse.unwrap();

    let frame = sse.next_frame().unwrap().expect("frame");
    assert_eq!(frame.data, "hello world");

    assert!(sse.next_frame().unwrap().is_none());
}

#[test]
fn multiple_events_across_chunks() {
    let body = "B\r\ndata: one\n\n\r\nB\r\ndata: two\n\n\r\n0\r\n\r\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
    );
    let (_mock, sse) = open(response.as_bytes());
    let sse = sse.unwrap();

    let frames: Vec<_> = sse.collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].data, "one");
    assert_eq!(frames[1].data, "two");
}

#[test]
fn one_event_split_across_two_chunks() {
    let body = "7\r\ndata: h\r\nC\r\nello world\n\n\r\n0\r\n\r\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
    );
    let (_mock, sse) = open(response.as_bytes());
    let mut sse = sse.unwrap();

    let frame = sse.next_frame().unwrap().expect("frame");
    assert_eq!(frame.data, "hello world");
}

#[test]
fn event_with_id_and_type() {
    let body = "1F\r\nevent: state\nid: 42\ndata: ok\n\n\r\n0\r\n\r\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
    );
    let (_mock, sse) = open(response.as_bytes());
    let mut sse = sse.unwrap();

    let frame = sse.next_frame().unwrap().expect("frame");
    assert_eq!(frame.event.as_deref(), Some("state"));
    assert_eq!(frame.id.as_deref(), Some("42"));
    assert_eq!(frame.data, "ok");
    assert_eq!(sse.last_event_id(), Some("42"));
}

#[test]
fn comment_lines_are_skipped() {
    let body = "D\r\n: keep-alive\n\n\r\nB\r\ndata: hi\n\n\r\n0\r\n\r\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
    );
    let (_mock, sse) = open(response.as_bytes());
    let mut sse = sse.unwrap();

    let frame = sse.next_frame().unwrap().expect("frame");
    assert_eq!(frame.data, "hi");
}
