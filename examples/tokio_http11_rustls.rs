//! HTTP/1.1 request over TLS using the async Tokio runtime.
//!
//! Demonstrates [`Http11Send`] with `tokio-rustls` for TLS and
//! [`io_socket::runtimes::tokio_stream::handle`] as the async I/O
//! driver.
//!
//! # Usage
//!
//! ```sh
//! URL=https://example.com/ cargo run --example tokio_http11_rustls
//! ```

use std::{env, sync::Arc};

use io_http::{
    rfc9110::request::HttpRequest,
    rfc9112::send::{Http11Send, Http11SendResult},
};
use io_socket::runtimes::tokio_stream::handle;
use log::info;
use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;
use tokio::net::TcpStream;
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

        let mut arg = None;
        let mut send = Http11Send::new(request);

        loop {
            match send.resume(arg.take()) {
                Http11SendResult::Ok { response, .. } => break 'outer response,
                Http11SendResult::Err { err } => panic!("{err}"),
                Http11SendResult::Io { input } => {
                    arg = Some(handle(&mut stream, input).await.unwrap())
                }
                Http11SendResult::Redirect { url: new_url, .. } => {
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
