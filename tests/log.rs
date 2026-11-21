//! Tests for what the crate logs: no credential reaches a log line.

#![cfg(feature = "client")]

use std::sync::Mutex;

use io_http::{
    coroutine::*,
    rfc9110::{request::HttpRequest, send::*},
    rfc9112::send::Http11Send,
};
use log::{LevelFilter, Log, Metadata, Record};
use url::Url;

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl Log for Capture {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        LINES.lock().unwrap().push(record.args().to_string());
    }

    fn flush(&self) {}
}

#[test]
fn no_credential_in_logs() {
    log::set_logger(&Capture).unwrap();
    log::set_max_level(LevelFilter::Trace);

    let url = "http://user:URLPASSWORD@example.com/path?token=QUERYSECRET&page=2";
    let request = HttpRequest::get(Url::parse(url).unwrap())
        .header("Authorization", "Bearer REQUESTSECRET")
        .body(b"REQUESTBODY".to_vec());

    let response = b"HTTP/1.1 200 OK\r\nSet-Cookie: sid=COOKIESECRET\r\nWWW-Authenticate: Basic realm=\"REALMSECRET\"\r\nContent-Length: 2\r\n\r\nok";

    let mut send = Http11Send::new(request);
    let mut arg: Option<&[u8]> = None;

    loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => {}
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => arg = Some(response),
            HttpCoroutineState::Complete(Ok(_)) => break,
            other => panic!("unexpected result: {other:?}"),
        }
    }

    let lines = LINES.lock().unwrap().join("\n");

    for secret in [
        "URLPASSWORD",
        "QUERYSECRET",
        "REQUESTSECRET",
        "REQUESTBODY",
        "COOKIESECRET",
        "REALMSECRET",
    ] {
        assert!(!lines.contains(secret), "{secret} in logs:\n{lines}");
    }

    for kept in [
        "example.com/path",
        "token=",
        "Authorization",
        "set-cookie",
        "200",
    ] {
        assert!(lines.contains(kept), "{kept} not in logs:\n{lines}");
    }
}
