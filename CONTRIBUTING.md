# Contributing guide

Thank you for investing your time in contributing to I/O HTTP.

Whether you are a human or an AI agent, read these in order before touching the code:

1. the [Pimalaya README](https://github.com/pimalaya) for what the project is and how its repositories stack;
2. the [Pimalaya CONTRIBUTING](https://github.com/pimalaya/.github/blob/master/CONTRIBUTING.md) guide, which chains to the shared architecture and guidelines;
3. the inline header documentation, starting with src/lib.rs (or src/main.rs): it is the architecture document of this crate;
4. the docs/ folder for the development history and living plans.

Everything below documents only what differs from the Pimalaya standards.

## Feature matrix

io-http follows the standard three-layer split, plus a vendored switch compiling the TLS dependencies from source:

```sh
cargo build --no-default-features                        # coroutines only, no std leak
cargo build --no-default-features --features client      # light client, no TLS deps
cargo build                                              # full client (rustls-ring by default)
cargo build --no-default-features --features rustls-aws  # full client, aws-lc-rs crypto
cargo build --no-default-features --features native-tls  # full client, platform TLS
cargo build --features vendored                          # vendored TLS dependencies
```

## Examples

Two examples pump the coroutines themselves and need no cargo feature; the last one uses the full client and requires the default TLS feature:

```sh
URL=http://example.com/ cargo run --example std_http10
URL=https://example.com/ cargo run --example tokio_http11
URL=https://example.com/ cargo run --example std_http11
```
