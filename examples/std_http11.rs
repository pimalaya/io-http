//! HTTP/1.1 request via the std-blocking [`HttpClientStd`].
//!
//! Requires one of the TLS feature flags (`rustls-ring`, `rustls-aws`
//! or `native-tls`) so the client can open both `http://` and
//! `https://` URLs end-to-end via [`pimalaya_stream`].
//!
//! # Usage
//!
//! ```sh
//! URL=https://example.com/ cargo run --example std_http11
//! ```

use std::env;

use io_http::{client::HttpClientStd, rfc9110::request::HttpRequest};
use log::info;
use pimalaya_stream::tls::Tls;
use url::Url;

fn main() {
    env_logger::init();

    let url: Url = match env::var("URL") {
        Ok(url) => url.parse().unwrap(),
        Err(_) => "https://example.com/".parse().unwrap(),
    };

    info!("connect to {url}");
    let mut client = HttpClientStd::connect(&url, &Tls::default()).unwrap();

    let host = url.host_str().unwrap().to_owned();
    let request = HttpRequest::get(url)
        .header("Host", &host)
        .header("Connection", "close");

    let output = client.send(request).unwrap();

    println!("-------------------------");
    println!("-------- HEADERS --------");
    println!("-------------------------");
    println!("{} {}", output.response.version, *output.response.status);

    for (key, val) in &output.response.headers {
        println!("{key}: {val}");
    }

    println!("-------------------------");
    println!("--------- BODY ----------");
    println!("-------------------------");

    print!("{}", String::from_utf8_lossy(&output.response.body));
}
