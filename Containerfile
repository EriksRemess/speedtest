FROM docker.io/library/rust:1.100-alpine@sha256:de1e0058543a81ed7b046629167db587c67a94bdf37cce14eb27f0ec83baaffb AS build

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY web ./web
RUN cargo build --locked --release

FROM scratch

LABEL org.opencontainers.image.title="speedtest" \
      org.opencontainers.image.description="Dependency-free web speed test with optional iperf3 compatibility"

COPY --from=build /src/target/release/speedtest /speedtest

USER 65532:65532
EXPOSE 8080/tcp 5201/tcp
ENTRYPOINT ["/speedtest"]
