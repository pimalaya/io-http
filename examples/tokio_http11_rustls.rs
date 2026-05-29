//! HTTP/1.1 request over TLS using the async Tokio runtime.
//!
//! Demonstrates [`Http11Send`] with `tokio-rustls` for TLS. The
//! coroutine itself does no I/O; bytes are pushed/pulled via
//! [`AsyncReadExt::read`] / [`AsyncWriteExt::write_all`].
//!
//! # Usage
//!
//! ```sh
//! URL=https://example.com/ cargo run --example tokio_http11_rustls
//! ```

use std::{env, sync::Arc};

use io_http::{
    coroutine::*,
    rfc9110::{request::HttpRequest, send::HttpSendYield},
    rfc9112::send::Http11Send,
};
use log::info;
use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;
use url::Url;

#[tokio::main]
async fn main() {
    env_logger::init();

    let mut url: Url = match env::var("URL") {
        Ok(url) => url.parse().unwrap(),
        Err(_) => "https://example.com/".parse().unwrap(),
    };

    // loop for potential redirections
    let response = 'outer: loop {
        info!("connect to {url}");

        let domain = url.domain().unwrap().to_owned();
        let port = url.port_or_known_default().unwrap_or(443);
        let tcp = TcpStream::connect((domain.as_str(), port)).await.unwrap();

        let config = Arc::new(ClientConfig::with_platform_verifier().unwrap());
        let connector = TlsConnector::from(config);
        let server_name = domain.clone().try_into().unwrap();
        let mut stream = connector.connect(server_name, tcp).await.unwrap();

        let request = HttpRequest::get(url.clone())
            .header("Host", &domain)
            .header("Connection", "close");

        let mut send = Http11Send::new(request);
        let mut arg: Option<&[u8]> = None;
        let mut buf = [0u8; 4096];

        loop {
            match send.resume(arg.take()) {
                HttpCoroutineState::Complete(Ok(out)) => break 'outer out.response,
                HttpCoroutineState::Complete(Err(err)) => panic!("{err}"),
                HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                    stream.write_all(&bytes).await.unwrap();
                }
                HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                    let n = stream.read(&mut buf).await.unwrap();
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
