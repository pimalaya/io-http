//! Full tokio client: implement [`HttpClientAsync`] over a tokio socket
//! and tokio-rustls, then let the trait supply the request surface.
//!
//! This is the "any other runtime" case. No io-http TLS feature is
//! involved: the runtime, the socket and the TLS stack belong to the
//! consumer. What stays in io-http is the protocol thinking, and the
//! request coroutine hands the one policy question back rather than
//! answering it: `WantsRedirect` names the target and lets the
//! implementor decide. Here `run_send` refuses it, exactly like the std
//! client does, and the caller reconnects to the new authority itself,
//! which is where the decision belongs since only it knows whether the
//! target is one it meant to talk to.
//!
//! Run with: `URL=https://example.com/ cargo run --example tokio_client`

use std::{env, error::Error, sync::Arc};

use io_http::{
    client::{HttpClientAsync, HttpClientError},
    coroutine::*,
    rfc9110::{
        request::HttpRequest,
        send::{HttpSendOutput, HttpSendYield},
    },
};
use log::info;
use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use url::Url;

/// Read buffer, sized for header-and-body traffic.
const READ_BUFFER_SIZE: usize = 16 * 1024;

/// How many redirects the caller is willing to follow.
const MAX_REDIRECTS: usize = 8;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();

    let mut url: Url = match env::var("URL") {
        Ok(url) => url.parse()?,
        Err(_) => "https://example.com/".parse()?,
    };

    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let config = Arc::new(ClientConfig::with_platform_verifier()?);
    let connector = TlsConnector::from(config);

    let mut response = None;

    for _ in 0..MAX_REDIRECTS {
        let domain = url.domain().ok_or("URL has no domain")?.to_owned();
        let mut client = HttpClientTokio::connect(&connector, &domain, &url).await?;

        let request = HttpRequest::get(url.clone())
            .header("Host", &domain)
            .header("Connection", "close");

        // NOTE: `send` is one of the trait's default bodies, and the
        // future it returns is Send, so a request can move onto another
        // task. That is why `run_send` is declared as `impl Future<..> +
        // Send` rather than written as an `async fn`: an `async fn` in a
        // trait cannot promise Send, and this spawn would stop
        // compiling.
        let sent = tokio::spawn(async move { client.send(request).await }).await?;

        match sent {
            Ok(HttpSendOutput { response: res, .. }) => {
                response = Some(res);
                break;
            }
            // NOTE: the new authority needs a new socket, so following
            // the redirect is the caller's business rather than the
            // pump's.
            Err(HttpClientError::UnexpectedRedirect { url: target, code }) => {
                info!("redirected to {target} ({code})");
                url = target;
            }
            Err(err) => return Err(err.into()),
        }
    }

    let response = response.ok_or("too many redirects")?;

    println!("{} {}", response.version, *response.status);

    for (key, val) in &response.headers {
        println!("{key}: {val}");
    }

    print!("{}", String::from_utf8_lossy(&response.body));

    Ok(())
}

/// A tokio HTTP client: one TLS socket, and nothing else. Both requests
/// come from [`HttpClientAsync`].
struct HttpClientTokio {
    stream: TlsStream<TcpStream>,
}

impl HttpClientTokio {
    /// Opens the TLS socket the trait then runs coroutines against.
    async fn connect(
        connector: &TlsConnector,
        domain: &str,
        url: &Url,
    ) -> Result<Self, Box<dyn Error>> {
        let port = url.port_or_known_default().unwrap_or(443);
        let tcp = TcpStream::connect((domain, port)).await?;
        let name = domain.to_owned().try_into()?;

        Ok(Self {
            stream: connector.connect(name, tcp).await?,
        })
    }
}

impl HttpClientAsync for HttpClientTokio {
    // NOTE: clippy asks to collapse these into `async fn`s. Refuse: an
    // `async fn` in a trait cannot state that its future is Send, and
    // that Send bound is what lets any request built on these methods
    // move onto a spawned task.
    #[allow(clippy::manual_async_fn)]
    fn run<C, T, E>(
        &mut self,
        mut coroutine: C,
    ) -> impl Future<Output = Result<T, HttpClientError>> + Send
    where
        C: HttpCoroutine<Yield = HttpYield, Return = Result<T, E>> + Send,
        T: Send,
        E: Send,
        HttpClientError: From<E>,
    {
        async move {
            let mut buf = [0u8; READ_BUFFER_SIZE];
            let mut arg: Option<&[u8]> = None;

            loop {
                match coroutine.resume(arg.take()) {
                    HttpCoroutineState::Complete(Ok(out)) => return Ok(out),
                    HttpCoroutineState::Complete(Err(err)) => return Err(err.into()),
                    HttpCoroutineState::Yielded(HttpYield::WantsRead) => {
                        let n = self.stream.read(&mut buf).await?;
                        arg = Some(&buf[..n]);
                    }
                    HttpCoroutineState::Yielded(HttpYield::WantsWrite(bytes)) => {
                        self.stream.write_all(&bytes).await?;
                        arg = None;
                    }
                }
            }
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn run_send<C, E>(
        &mut self,
        mut coroutine: C,
    ) -> impl Future<Output = Result<HttpSendOutput, HttpClientError>> + Send
    where
        C: HttpCoroutine<Yield = HttpSendYield, Return = Result<HttpSendOutput, E>> + Send,
        E: Send,
        HttpClientError: From<E>,
    {
        async move {
            let mut buf = [0u8; READ_BUFFER_SIZE];
            let mut arg: Option<&[u8]> = None;

            loop {
                match coroutine.resume(arg.take()) {
                    HttpCoroutineState::Complete(Ok(out)) => return Ok(out),
                    HttpCoroutineState::Complete(Err(err)) => return Err(err.into()),
                    HttpCoroutineState::Yielded(HttpSendYield::WantsRead) => {
                        let n = self.stream.read(&mut buf).await?;
                        arg = Some(&buf[..n]);
                    }
                    HttpCoroutineState::Yielded(HttpSendYield::WantsWrite(bytes)) => {
                        self.stream.write_all(&bytes).await?;
                        arg = None;
                    }
                    HttpCoroutineState::Yielded(HttpSendYield::WantsRedirect {
                        url,
                        response,
                        ..
                    }) => {
                        return Err(HttpClientError::UnexpectedRedirect {
                            url,
                            code: *response.status,
                        });
                    }
                }
            }
        }
    }
}
