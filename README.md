# speedtest

A tiny, self-hosted network speed test: a Rust standard-library backend, an embedded modern web UI, and an iperf3-compatible TCP server. There are no crate, JavaScript, font, image, or CDN dependencies.

## Run

```sh
cargo run --release
```

Open `http://localhost:8080`. From another machine, use the server's LAN address.

The native endpoint works with regular iperf3 clients:

```sh
iperf3 -c SERVER_ADDRESS
iperf3 -c SERVER_ADDRESS -R
iperf3 -c SERVER_ADDRESS -P 4
```

Options:

```text
--bind ADDRESS       Listen address (default: ::)
--http-port PORT     Web/API port (default: 8080)
--iperf-port PORT    iperf3 port (default: 5201)
```

## Compatibility

The iperf3 listener supports TCP forward, reverse (`-R`), and parallel (`-P`) tests. It intentionally does not claim support for UDP, SCTP, bidirectional mode, authentication, or advanced socket-tuning options. One iperf3 test runs at a time; the browser endpoint remains concurrent.

The browser test measures median request latency, average variation between consecutive latency samples (jitter), and streamed download and upload throughput. Treat these as application-path measurements: browser memory, HTTP framing, and runtime overhead are part of the result. Use iperf3 for lower-level network benchmarking.

## Container

Build and run the static, unprivileged image with Podman:

```sh
podman build -t speedtest -f Containerfile .
podman run --rm -p 8080:8080 -p 5201:5201 speedtest
```

Or run the published image:

```sh
podman run --rm -p 8080:8080 -p 5201:5201 ghcr.io/eriksremess/speedtest:latest
```
