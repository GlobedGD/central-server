# Globed Central Server

## Building

Central server requires nightly Rust. If you have never used Rust, install rustup from https://rustup.rs/ and then ensure you have the latest version by running
```sh
rustup toolchain install nightly
```

While in the server folder, add an override to tell cargo to always use nightly Rust for building the server:
```sh
rustup override set nightly
```

[Cap 'n Proto](https://capnproto.org/install.html) is recommended but not required. If you have troubles installing it, or are getting `error: Import failed: /capnp/c++.capnp` errors during the server build, you can set the `SERVER_SHARED_PREBUILT_DATA=1` environment variable to remove the need for capnp. **This is only recommended to do if you aren't planning on changing the schemas!**

To build in default configuration, with base features:
```sh
cargo build # add --release for release builds
```

To build with extra features, `--features` can be passed:
```sh
cargo build --features featured-levels,discord,quic
```

Possible feature flags:
* `all` - includes all functional features, does not include `stat-tracking` and `mimalloc`
* `word-filter` - adds a word filter module, allowing you to create a blacklist for room names and usernames
* `featured-levels` - adds a featured levels module, letting moderators send/queue/feature levels and update information to a google spreadsheet
* `discord` - adds the discord bot module, which can send logs and alerts, as well as allowing moderation or maintenance with discord commands
* `quic` - enables QUIC support. requires extra setup in the form of TLS certificates
* `stat-tracking` - **for debugging only** tracks every packet of every connection. on Linux, completed connections can be dumped by sending `SIGUSR1`
* `mimalloc` - replaces the allocator with MiMalloc

## Docker builds

Docker buildx can be used to build an image of the server:
```sh
docker buildx build --target <target> --platform <platform> -t central-server:latest .
# to save the image
docker save -o image.tar central-server:latest
```

`<target>` must be either `runtime-alpine` (static linked musl binary, small alpine image) or `runtime-debian` (glibc linked binary, debian-slim runtime)

`<platform>` must be either `linux/amd64` (x86_64) or `linux/arm64`

These builds include features `all` and `mimalloc`. The Github Actions builds are made with buildx and are identical to the builds produced in the `runtime-debian` image.

## Configuring

Upon running the server for the first time, a `config` folder will be generated, with multiple `.toml` files. Each server module has its own configuration file. TODO rest of this