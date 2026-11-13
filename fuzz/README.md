# Fuzzing

Coverage-guided fuzzing of the response parsers with [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) (libFuzzer).

The `response` target feeds arbitrary bytes, split into arbitrary reads and followed by EOF, to `Http11Send`, `Http10Send` and `Http11ChunksReadStream`, with fuzzed head and body limits. Its oracle is that whatever a server sends, the coroutines answer or fail, never panic, never keep asking for reads after EOF, and never return a body larger than their limit.

The same reads go to `SseFrameParser`, the two limits serving as its line and event limits. Its oracle is that no event exceeds a limit, and that the events and the failure are the same whether the stream arrives split or whole.

cargo-fuzz needs a nightly toolchain, which the `fuzz` devShell of the repository flake carries along with cargo-fuzz itself. Bound a run with the libFuzzer flags after the `--` separator:

```sh
nix develop .#fuzz --command bash -c "cargo fuzz run response -- -max_total_time=60"
```

New interesting inputs land in `fuzz/corpus/response/` and crashes in `fuzz/artifacts/response/` (both gitignored), from where a crash is replayed with `cargo fuzz run response fuzz/artifacts/response/<file>`.
