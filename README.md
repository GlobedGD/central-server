# Globed Central Server

## Running

The simplest way to get the server running is [GitHub actions](https://github.com/GlobedGD/central-server/actions) - click the latest workflow and download the `central-server-build` artifact, which will contain three executables for different platforms. Extract the one that matches your platform into a dedicated folder somewhere and run it.

There are currently prebuilts for three platforms, for others you need to build manually or use Docker:
* `central-server-x64` - Linux x64 build, requires glibc 2.30+ (e.g. Ubuntu 20.04 or later)
* `central-server-arm64` - Linux ARM64 build, requires glibc 2.30+
* `central-server.exe` - Windows x64 build

Alternate ways of running the server include:
* [Building yourself](#building)
* [Docker](#docker-builds)

## Configuration

Check the [Server Hosting](https://github.com/GlobedGD/globed2/blob/main/docs/server-hosting.md) page for a proper configuration guide

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
* `all` - includes all functional features, does not include `mimalloc`
* `word-filter` - adds a word filter module, allowing you to create a blacklist for room names and usernames
* `featured-levels` - adds a featured levels module, letting moderators send/queue/feature levels and update information to a google spreadsheet
* `discord` - adds the discord bot module, which can send logs and alerts, as well as allowing moderation or maintenance with discord commands
* `quic` - enables QUIC support. requires extra setup in the form of TLS certificates
* `mimalloc` - replaces the allocator with MiMalloc

## Docker builds

Prebuilt images are available on the GitHub Container Registry, making it very simple to spin up a server like so:
```sh
# volume for storing databases
docker volume create central-server-data

docker run --rm -it  \
    -v central-server-data:/data \
    -p 4340:4340/tcp -p 4340:4340/udp -p 4342:4342/tcp \
    ghcr.io/globedgd/central-server:latest
```

The container stores all sqlite databases under `/data`, and the `config` folder with .toml files will also be generated inside `/data`. The commands above will ensure these are stored persistently, but if you want to configure the server it may be easier to use environment variables or mount the `/data/config` folder separately.


If you want to build the images yourself, docker buildx can be used:
```sh
docker buildx build --target <target> --platform <platform> -t central-server:latest .
# to save the image
docker save -o image.tar central-server:latest
```

`<target>` must be either `runtime-alpine` (static linked musl binary, small alpine image) or `runtime-debian` (glibc linked binary, debian-slim runtime)

`<platform>` must be either `linux/amd64` (x86_64) or `linux/arm64`

These builds include features `all` (plus `mimalloc` on alpine). The GHCR and Actions builds are made with buildx and are identical to the builds produced in the `runtime-debian` image.

## Migrating databases

If you used to host a server for an older version of Globed (pre-v2), check the `db-migrate.py` script to migrate your database.