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

[Cap 'n Proto](https://capnproto.org/install.html) is also optionally required. If you have troubles installing it, or are getting `error: Import failed: /capnp/c++.capnp` errors during the server build, you can set the `SERVER_SHARED_PREBUILT_DATA=1` environment variable to remove the need for capnp. **This is only recommended to do if you aren't planning on changing the schemas!**

To build in default configuration, with most features:
```sh
cargo build # add --release for release builds
```

To build with all the extra features (such as featured levels):
```sh
cargo build --features featured-levels
```

## Configuring

Upon running the server for the first time, a `config` folder will be generated, with multiple `.toml` files. Each server module has its own configuration file. TODO rest of this