use std::{
    env,
    io::{stdin, stdout, Read, Write},
    net::TcpStream,
    sync::Arc,
};

use http::{header::LOCATION, Request};
use io_http::v1_1::coroutines::send::{SendHttp, SendHttpResult};
use io_stream::runtimes::std::handle;
use log::info;
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use rustls_platform_verifier::ConfigVerifierExt;
use url::Url;

fn main() {
    env_logger::init();

    let mut url: Url = match env::var("URL") {
        Ok(url) => url.parse().unwrap(),
        Err(_) => read_line("URL?").parse().unwrap(),
    };

    // loop for potential redirections
    let response = loop {
        info!("connect to {url}");
        let mut stream = connect(&url);

        let request = Request::get(url.as_str())
            .header("Host", url.host_str().unwrap())
            .body(vec![])
            .unwrap();

        let mut arg = None;
        let mut send = SendHttp::new(request);

        let response = loop {
            match send.resume(arg.take()) {
                SendHttpResult::Ok(result) => break result.response,
                SendHttpResult::Err(err) => panic!("{err}"),
                SendHttpResult::Io(io) => arg = Some(handle(&mut stream, io).unwrap()),
            }
        };

        if !response.status().is_redirection() {
            break response;
        }

        info!("redirection requested");

        let location = response
            .headers()
            .get(LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();

        url = location;
    };

    println!("-------------------------");
    println!("-------- HEADERS --------");
    println!("-------------------------");
    println!("{:?} {}", response.version(), response.status());

    for (key, val) in response.headers() {
        println!("{key}: {}", val.to_str().unwrap());
    }

    println!("-------------------------");
    println!("--------- BODY ----------");
    println!("-------------------------");

    let body = String::from_utf8_lossy(response.body());
    print!("{body}");
}

fn read_line(prompt: &str) -> String {
    print!("{prompt} ");
    stdout().flush().unwrap();

    let mut line = String::new();
    stdin().read_line(&mut line).unwrap();

    line.trim().to_owned()
}

trait StreamExt: Read + Write {}
impl<T: Read + Write> StreamExt for T {}

fn connect(url: &Url) -> Box<dyn StreamExt> {
    let domain = url.domain().unwrap();
    if url.scheme().eq_ignore_ascii_case("https") {
        let config = ClientConfig::with_platform_verifier().unwrap();
        let server_name = domain.to_string().try_into().unwrap();
        let conn = ClientConnection::new(Arc::new(config), server_name).unwrap();
        let tcp = TcpStream::connect((domain.to_string(), 443)).unwrap();
        let tls = StreamOwned::new(conn, tcp);
        Box::new(tls)
    } else {
        let tcp = TcpStream::connect((domain.to_string(), 80)).unwrap();
        Box::new(tcp)
    }
}
