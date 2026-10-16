# Contributing

* Every commit carries a `Signed-off-by:` line (`git commit -s`), as in the
  C OpenAMP projects, certifying the [DCO](https://developercertificate.org/).
* Commits made with AI assistance carry an `Assisted-by:` tag in OpenAMP's
  format: `Assisted-by: AGENT_NAME:MODEL_VERSION [TOOL1] [TOOL2]`.
* Before sending a pull request:

  ```sh
  cargo fmt --all --check
  cargo clippy --workspace --all-features --all-targets -- -D warnings
  cargo test --workspace --all-features
  ./interop-tests/fetch-c-sources.sh
  cargo test --manifest-path interop-tests/Cargo.toml --release
  ```

* `unsafe` code stays in `openamp-metal`'s `IoRegion` and the virtqueue, with
  a `// SAFETY:` comment on every block. Anything read from shared memory is
  validated before use; a misbehaving peer must produce an error, not a
  panic or out-of-bounds access.
* Wire-format changes need an interop test against the C libraries.
