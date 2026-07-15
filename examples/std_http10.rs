//! HTTP/1.0 request over a plain TCP connection (blocking).
//!
//! Demonstrates [`Http10Send`] with the synchronous `std` runtime.
//! HTTP/1.0 (RFC 1945) has no persistent connections by default and
//! no chunked transfer encoding.
//!
//! # Usage
//!
//! ```sh
//! URL=http://example.com/ cargo run --example std_http10
//! ```

use std::{
    env,
    io::{Read, Write},
    net::TcpStream,
};

use io_http::{
    coroutine::*,
    rfc1945::send::Http10Send,
    rfc9110::{request::HttpRequest, send::HttpSendYield},
};
use log::info;
use url::Url;

fn main() {
    env_logger::init();

    let mut url: Url = match env::var("URL") {
        Ok(url) => url.parse().unwrap(),
        Err(_) => "http://example.com/".parse().unwrap(),
    };

    // NOTE: the outer loop follows potential redirections.
    let response = 'outer: loop {
        info!("connect to {url}");

        let host = url.host_str().unwrap().to_owned();
        let port = url.port_or_known_default().unwrap_or(80);
        let mut stream = TcpStream::connect((host.as_str(), port)).unwrap();

        let request = HttpRequest::get(url.clone()).header("Host", &host);

        let mut send = Http10Send::new(request);
        let mut arg: Option<&[u8]> = None;
        let mut buf = [0u8; 4096];

        loop {
            match send.resume(arg.take()) {
                HttpCoroutineState::Complete(Ok(out)) => break 'outer out.response,
                HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                    stream.write_all(&bytes).unwrap();
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                    let n = stream.read(&mut buf).unwrap();
                    arg = Some(&buf[..n]);
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                    url: new_url, ..
                }) => {
                    info!("redirection requested");
                    url = new_url;
                    break;
                }
            }
        }
    };

    println!("-------------------------");
    println!("-------- HEADERS --------");
    println!("-------------------------");
    println!("{} {}", response.version, *response.status);

    for (key, val) in &response.headers {
        println!("{key}: {val}");
    }

    println!("-------------------------");
    println!("--------- BODY ----------");
    println!("-------------------------");

    print!("{}", String::from_utf8_lossy(&response.body));
}
