# Rust API

The workspace's crates are documented with rustdoc, built into this book
under `api/`.

- [`archiveview`](../api/archiveview/index.html): the `Plot` widget to embed
  in an egui app (see [Embed the plot](../how-to/embed.md)), the standalone
  `ArchiveApp`, views, config and snapshot mode
- [`archiver`](../api/archiver/index.html): the Archiver Appliance retrieval
  client: URLs, requests and tolerant JSON decoding
- [`mockarchiver`](../api/mockarchiver/index.html): the mock appliance, for
  tests

The crates are not published to crates.io and their API is not yet stable.

In a local build the links work when the book is built with
`scripts/build-docs.sh` (see [Build from source](../how-to/build.md#build-these-docs)),
not with a plain `mdbook serve`.
