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

## Releasing

The five library crates share the workspace version and are released
together by `.github/workflows/release.yml`.

1. On a branch named `release/X.Y.Z`, bump `version` in `[workspace.package]`
   and in the `openamp-*` entries of `[workspace.dependencies]` in
   `Cargo.toml`. Open a pull request: CI runs `cargo semver-checks` on
   `release*` branches.
2. Optionally run the Release workflow by hand on that branch: it is a dry
   run that packages and verifies every crate.
3. After merging, tag the merge commit and push the tag:

   ```sh
   git tag -a vX.Y.Z -m "openamp-rs X.Y.Z" && git push origin vX.Y.Z
   ```

   The workflow checks that the tag matches the version, runs the tests,
   publishes the crates in dependency order (skipping any already
   published, so a failed run can be re-run) and creates the GitHub
   release.

One-time setup: create a `crates-io` environment in the repository settings
(add required reviewers to approve each upload). The crates publish with
crates.io trusted publishing; configure it on crates.io for each crate
(repository, workflow `release.yml`, environment `crates-io`). Trusted
publishing cannot create a new crate, so for the first release add a
`CARGO_REGISTRY_TOKEN` secret (scope: publish-new, publish-update) to the
`crates-io` environment, and delete it once trusted publishing is set up.
