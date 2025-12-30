FROM --platform=$BUILDPLATFORM rustlang/rust:nightly AS builder

ARG TARGETARCH
ENV SERVER_SHARED_PREBUILT_DATA=1

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config ca-certificates curl xz-utils
RUN rm -rf /var/lib/apt/lists/*

# download zig
RUN curl -L https://ziglang.org/builds/zig-x86_64-linux-0.16.0-dev.1859+212968c57.tar.xz | tar -xJ && mv zig-x86_64-linux-0.16.0-dev.1859+212968c57 /zig
ENV PATH="/zig:${PATH}"

# map arch to target
RUN case "$TARGETARCH" in \
    amd64) echo "x86_64-unknown-linux-musl" > /target.txt ;; \
    arm64) echo "aarch64-unknown-linux-musl" > /target.txt ;; \
    *) echo "unsupported architecture" >&2; exit 1 ;; \
    esac

# install target
RUN rustup target add $(cat /target.txt)

# install zigbuild
RUN cargo install --locked cargo-zigbuild

# copy and build the server
COPY src ./src
COPY Cargo.toml Cargo.lock ./
RUN cargo zigbuild --release --features all --target $(cat /target.txt)

# runtime image
FROM scratch
COPY --from=builder /app/target/*/release/central-server /central-server

EXPOSE 4340/tcp
EXPOSE 4340/udp
EXPOSE 4341/udp

ENTRYPOINT ["/central-server"]
