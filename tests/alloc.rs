//! Tests for what an exchange allocates, as counted by the allocator.
//!
//! The counters are per thread, so that tests running side by side do
//! not see each other, and a reallocation counts as a new block next to
//! the old one, the worst case of a buffer that grows.

#![cfg(feature = "client")]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

use io_http::{
    coroutine::*,
    rfc9110::{request::HttpRequest, send::*},
    rfc9112::send::Http11Send,
};
use url::Url;

thread_local! {
    static LIVE: Cell<usize> = const { Cell::new(0) };
    static PEAK: Cell<usize> = const { Cell::new(0) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: every block comes from and returns to the system allocator
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = LIVE.try_with(|live| {
            live.set(live.get().wrapping_add(layout.size()));
            let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
        });
        let _ = ALLOCS.try_with(|allocs| allocs.set(allocs.get() + 1));

        // SAFETY: the caller upholds the contract of `alloc`
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _ = LIVE.try_with(|live| live.set(live.get().wrapping_sub(layout.size())));

        // SAFETY: the caller upholds the contract of `dealloc`
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Runs `work`; returns its value, the peak of live bytes above what
/// was live at the start, and the number of allocations.
fn measure<T>(work: impl FnOnce() -> T) -> (T, usize, usize) {
    let live = LIVE.get();
    let allocs = ALLOCS.get();
    PEAK.set(live);

    let value = work();

    (value, PEAK.get() - live, ALLOCS.get() - allocs)
}

/// Receives `response` in reads of `read_len` bytes, returns the body.
fn receive(response: &[u8], read_len: usize) -> Vec<u8> {
    let url = Url::parse("http://example.com/").unwrap();
    let mut send = Http11Send::new(HttpRequest::get(url));
    let mut reads = response.chunks(read_len);
    let mut arg: Option<&[u8]> = None;

    loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => {}
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                arg = Some(reads.next().unwrap_or(b""))
            }
            HttpCoroutineState::Complete(Ok(out)) => return out.response.body,
            other => panic!("unexpected result: {other:?}"),
        }
    }
}

const BODY_LEN: usize = 4 * 1024 * 1024;

#[test]
fn content_length_body_held_once() {
    let mut response =
        format!("HTTP/1.1 200 OK\r\nContent-Length: {BODY_LEN}\r\n\r\n").into_bytes();
    response.resize(response.len() + BODY_LEN, b'x');

    let (body, peak, _) = measure(|| receive(&response, 16 * 1024));

    assert_eq!(body.len(), BODY_LEN);
    assert!(peak < BODY_LEN + BODY_LEN / 8, "peak of {peak} bytes");
}

#[test]
fn one_chunk_body_held_once() {
    let head = format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{BODY_LEN:x}\r\n");
    let mut response = head.into_bytes();
    response.resize(response.len() + BODY_LEN, b'x');
    response.extend(b"\r\n0\r\n\r\n");

    let (body, peak, _) = measure(|| receive(&response, 16 * 1024));

    assert_eq!(body.len(), BODY_LEN);
    assert!(peak < BODY_LEN + BODY_LEN / 8, "peak of {peak} bytes");
}

#[test]
fn request_body_copied_once() {
    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url)
        .header("Content-Type", "application/octet-stream")
        .body(vec![b'x'; BODY_LEN]);

    let (bytes, peak, _) = measure(|| request.to_http_11_vec().unwrap());

    assert!(bytes.len() > BODY_LEN);
    assert!(peak < BODY_LEN + BODY_LEN / 8, "peak of {peak} bytes");
}

#[test]
fn head_read_byte_by_byte_not_parsed_at_each_read() {
    let mut response = b"HTTP/1.1 200 OK\r\n".to_vec();
    for n in 0..20 {
        response.extend(format!("X-Header-{n}: {}\r\n", "v".repeat(400)).bytes());
    }
    response.extend(b"Content-Length: 2\r\n\r\nok");

    let (body, _, allocs) = measure(|| receive(&response, 1));

    assert_eq!(body, b"ok");
    assert!(
        allocs < 200,
        "{allocs} allocations for {} reads",
        response.len()
    );
}
