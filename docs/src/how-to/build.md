# Build from source

You need a stable Rust toolchain ([rustup](https://rustup.rs)).

```bash
git clone https://github.com/gilesknap/archiveview
cd archiveview
cargo build --release
cargo test --workspace
```

The binaries are in `target/release`. To try a change against the mock
archiver:

```bash
cargo run --bin archiveview-mockarchiver &
cargo run --bin archiveview -- --archiver http://127.0.0.1:17668 MOCK:SINE MOCK:NOISE
```

## Build for RHEL 8 and other older Linux

With [zig](https://ziglang.org) and
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) installed,
link against glibc 2.28 as the release builds do:

```bash
cargo zigbuild --release --workspace --target x86_64-unknown-linux-gnu.2.28
```

## Build these docs

The docs are an [mdBook](https://rust-lang.github.io/mdBook/):

```bash
cargo install mdbook
scripts/build-docs.sh serve --open
```

This builds the Rust API docs into the book under `api/`, then serves it;
`mdbook serve` rebuilds and reloads the page as you edit. Run it again to
pick up changes to the API docs. `scripts/build-docs.sh` alone builds the
book into `docs/book`.

## Checks CI runs

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI then snapshots the app against the mock archiver and `bobview-testioc`;
see [Take snapshots](snapshots.md#what-ci-does).

## How the docs are published

The `Docs` workflow builds the book and the API docs on every pull request.
On a push to `main` or a `v*` tag it publishes them to the `gh-pages` branch
in a folder named after the branch or tag, writes `switcher.json` (the list
of versions for the menu in the top bar) and leaves the other versions in
place. The site's root redirects to `main/`.
