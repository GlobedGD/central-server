FROM rustlang/rust:nightly-alpine AS builder

ARG TARGETARCH
ENV SERVER_SHARED_PREBUILT_DATA=1

WORKDIR /app

RUN apk add --no-cache musl-dev pkgconfig build-base

# map arch to target
RUN case "$TARGETARCH" in \
    amd64) echo "x86_64-unknown-linux-musl" > /target.txt ;; \
    arm64) echo "aarch64-unknown-linux-musl" > /target.txt ;; \
    *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac


# cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --target $(cat /target.txt)
RUN rm -rf src

# copy and build the server
COPY src ./src
COPY Cargo.lock ./
COPY Cargo.toml ./
RUN cargo build --release --features all --target $(cat /target.txt)

# runtime image
FROM scratch
COPY --from=builder /app/target/*/release/central-server /central-server

EXPOSE 4340/tcp
EXPOSE 4340/udp
EXPOSE 4341/udp

ENTRYPOINT ["/central-server"]
