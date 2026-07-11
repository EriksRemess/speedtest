FROM docker.io/library/rust:1.97-alpine@sha256:ec9c91e77119ce498cd1e87d96d77e0f75b2cee21655a29bc2bf75a51a2b20a4 AS build

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN cargo build --locked --release

FROM scratch

LABEL org.opencontainers.image.title="speedtest" \
      org.opencontainers.image.description="Dependency-free web and iperf3-compatible speed test server"

COPY --from=build /src/target/release/speedtest /speedtest

USER 65532:65532
EXPOSE 8080/tcp 5201/tcp
ENTRYPOINT ["/speedtest"]
