# stage 1: build
FROM rust:latest AS builder

WORKDIR /build

# cache dependencies separately from source
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release
RUN rm -rf src

# now the real build
COPY src ./src
RUN touch src/main.rs && cargo build --release

# stage 2: runtime - just the binary
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/kagurarr /usr/local/bin/kagurarr

CMD ["kagurarr"]
