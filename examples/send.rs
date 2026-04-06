use std::{
    env,
    io::{Read, Write, stdin, stdout},
    net::TcpStream,
    sync::Arc,
};

use io_http::{
    rfc9110::request::HttpRequest,
    rfc9112::send::{Http11Send, Http11SendResult},
};
use io_socket::runtimes::std_stream::handle;
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
    let response = 'outer: loop {
        info!("connect to {url}");
        let mut stream = connect(&url);

        let request = HttpRequest::get(url.clone())
            .header("Host", url.host_str().unwrap())
            .body(vec![]);

        let mut arg = None;
        let mut send = Http11Send::new(request);

        loop {
            match send.resume(arg.take()) {
                Http11SendResult::Ok { response, .. } => break 'outer response,
                Http11SendResult::Err { err } => panic!("{err}"),
                Http11SendResult::Io { input } => arg = Some(handle(&mut stream, input).unwrap()),
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

    let body = String::from_utf8_lossy(&response.body);
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
        let tcp = TcpStream::connect((domain, 443)).unwrap();
        let tls = StreamOwned::new(conn, tcp);
        Box::new(tls)
    } else {
        let tcp = TcpStream::connect((domain, 80)).unwrap();
        Box::new(tcp)
    }
}
