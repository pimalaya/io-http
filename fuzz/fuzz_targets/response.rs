#![no_main]

//! Coverage-guided fuzz target for the response parsers. The oracle:
//! whatever a server sends, split however the transport splits it, the
//! HTTP/1.0 and HTTP/1.1 send coroutines and the streaming chunk decoder
//! answer or fail, never panic, never spin on EOF, and never return a
//! body larger than the limit they were given. The event stream parser
//! never returns an event larger than its limits, and returns the same
//! events and the same failure whether the stream comes split or whole.

use arbitrary::Arbitrary;
use io_http::{
    coroutine::*,
    rfc1945::send::Http10Send,
    rfc9110::{request::HttpRequest, send::*},
    rfc9112::{
        chunk_stream::{Http11ChunksReadStream, Http11ChunksReadStreamYield},
        send::Http11Send,
    },
    sse::frame::*,
};
use libfuzzer_sys::fuzz_target;
use url::Url;

/// How many reads a coroutine may still ask for once EOF was reported.
const MAX_READS_AFTER_EOF: usize = 2;

#[derive(Arbitrary, Debug)]
struct Input {
    max_headers_len: u16,
    max_body_len: u16,
    reads: Vec<Vec<u8>>,
}

fuzz_target!(|input: Input| {
    let max_headers_len = usize::from(input.max_headers_len);
    let max_body_len = usize::from(input.max_body_len);

    let url = Url::parse("http://example.com/").unwrap();
    let request = HttpRequest::get(url).header("Host", "example.com");

    let mut send = Http11Send::new(request.clone());
    send.max_headers_len = max_headers_len;
    send.max_body_len = max_body_len;
    check_send(send, &input.reads, max_body_len);

    let mut send = Http10Send::new(request);
    send.max_headers_len = max_headers_len;
    send.max_body_len = max_body_len;
    check_send(send, &input.reads, max_body_len);

    let mut chunks = Http11ChunksReadStream::default();
    chunks.max_chunk_len = max_body_len;
    check_chunk_stream(chunks, &input.reads, max_body_len);

    let whole = input.reads.concat();
    let split = input.reads.iter().map(Vec::as_slice);
    let split = parse_events(split, max_headers_len, max_body_len);
    let whole = parse_events([whole.as_slice()], max_headers_len, max_body_len);
    assert_eq!(split, whole, "events depend on the split");
});

fn check_send<C, E>(mut send: C, reads: &[Vec<u8>], max_body_len: usize)
where
    C: HttpCoroutine<Yield = HttpSendYield, Return = Result<HttpSendOutput, E>>,
{
    let mut reads = reads.iter().filter(|r| !r.is_empty());
    let mut reads_after_eof = 0;
    let mut arg: Option<&[u8]> = None;

    loop {
        match send.resume(arg.take()) {
            HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(_)) => {}
            HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => match reads.next() {
                Some(read) => arg = Some(read),
                None => {
                    reads_after_eof += 1;
                    assert!(reads_after_eof <= MAX_READS_AFTER_EOF, "spins on EOF");
                    arg = Some(b"");
                }
            },
            HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect { response, .. }) => {
                assert!(response.body.len() <= max_body_len);
                return;
            }
            HttpCoroutineState::Complete(Ok(out)) => {
                assert!(out.response.body.len() <= max_body_len);
                return;
            }
            HttpCoroutineState::Complete(Err(_)) => return,
        }
    }
}

fn check_chunk_stream(mut chunks: Http11ChunksReadStream, reads: &[Vec<u8>], max_len: usize) {
    let mut reads = reads.iter().filter(|r| !r.is_empty());
    let mut reads_after_eof = 0;
    let mut arg: Option<&[u8]> = None;

    loop {
        match chunks.resume(arg.take()) {
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::Frame { body }) => {
                assert!(body.len() <= max_len);
            }
            HttpCoroutineState::Yielded(Http11ChunksReadStreamYield::WantsRead) => {
                match reads.next() {
                    Some(read) => arg = Some(read),
                    None => {
                        reads_after_eof += 1;
                        assert!(reads_after_eof <= MAX_READS_AFTER_EOF, "spins on EOF");
                        arg = Some(b"");
                    }
                }
            }
            HttpCoroutineState::Complete(_) => return,
        }
    }
}

fn parse_events<'a>(
    reads: impl IntoIterator<Item = &'a [u8]>,
    max_line_len: usize,
    max_event_len: usize,
) -> (Vec<SseFrame>, Option<SseFrameParserError>) {
    let mut parser = SseFrameParser::default();
    parser.max_line_len = max_line_len;
    parser.max_event_len = max_event_len;

    let mut frames = Vec::new();

    for read in reads {
        let mut arg = Some(read);

        loop {
            match parser.resume(arg.take()) {
                HttpCoroutineState::Yielded(SseFrameParserYield::Frame(frame)) => {
                    assert!(frame.data.len() <= max_event_len);
                    assert!(frame.event.as_ref().is_none_or(|e| e.len() <= max_line_len));
                    assert!(frame.id.as_ref().is_none_or(|id| id.len() <= max_line_len));
                    frames.push(frame);
                }
                HttpCoroutineState::Yielded(SseFrameParserYield::WantsBytes) => break,
                HttpCoroutineState::Complete(Err(err)) => return (frames, Some(err)),
            }
        }
    }

    (frames, None)
}
