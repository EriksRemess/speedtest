# speedtest

A tiny, self-hosted network speed test: a Rust standard-library backend, an embedded modern web UI, and an optional iperf3-compatible TCP server. There are no crate, JavaScript, font, image, or CDN dependencies.

## Run

```sh
cargo run --release
```

Open `http://localhost:8080`. From another machine, use the server's LAN address.

The HTTP health endpoint is available at `/health` and returns `{"status":"ok"}`.

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
--no-iperf           Disable the iperf3 server
```

## Compatibility

The iperf3 listener supports TCP forward, reverse (`-R`), and parallel (`-P`) tests. It intentionally does not claim support for UDP, SCTP, bidirectional mode, authentication, or advanced socket-tuning options. One iperf3 test runs at a time; the browser endpoint remains concurrent.

The browser test measures median request latency, average variation between consecutive latency samples (jitter), and streamed download and upload throughput. Treat these as application-path measurements: browser memory, HTTP framing, and runtime overhead are part of the result. Use iperf3 for lower-level network benchmarking.

## Container

The ready-to-run image is published at `ghcr.io/eriksremess/speedtest:latest`. The `latest` tag tracks the repository's default branch.

```sh
podman run --rm --network host ghcr.io/eriksremess/speedtest:latest
```

To build the static, unprivileged image locally instead:

```sh
podman build -t speedtest -f Containerfile .
podman run --rm --network host speedtest
```

Host networking is recommended on Linux so Podman port forwarding does not distort either direction of the measurement. To use a different web port, append `--http-port PORT` to the command.

For a web-only container that does not listen on port 5201:

```sh
podman run --rm --network host ghcr.io/eriksremess/speedtest:latest --no-iperf
```

## Reverse proxy

For streamed uploads through nginx, disable proxy buffering and allow the full timed request body:

```nginx
location / {
    proxy_pass http://127.0.0.1:8080;
    proxy_http_version 1.1;

    client_max_body_size 2g;
    proxy_request_buffering off;
    proxy_buffering off;
}
```
