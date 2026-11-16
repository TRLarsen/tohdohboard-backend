# Build:  docker build -t tohdohboard-server .
# Run:    docker run -d -p 127.0.0.1:3001:3001 -v tohdoh-data:/data tohdohboard-server
# Config: mount your own file over /etc/tohdohboard/config.toml (see config.example.toml).

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data tohdoh \
 && mkdir -p /data /etc/tohdohboard \
 && chown tohdoh /data \
 && printf 'bind = "0.0.0.0:3001"\ndata_dir = "/data"\n' > /etc/tohdohboard/config.toml
COPY --from=build /src/target/release/tohdohboard-server /usr/local/bin/
ENV TOHDOH_CONFIG=/etc/tohdohboard/config.toml
USER tohdoh
VOLUME /data
EXPOSE 3001
ENTRYPOINT ["tohdohboard-server"]
