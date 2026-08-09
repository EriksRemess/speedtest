# speedtest

A tiny, self-hosted network speed test: a Rust standard-library backend, an embedded modern web UI, and an optional iperf3-compatible TCP server. There are no crate, JavaScript, font, image, or CDN dependencies.

## Quick start

The ready-to-run image is published at `ghcr.io/eriksremess/speedtest:latest`. The `latest` tag tracks the repository's default branch.

On Linux, host networking is recommended so container port forwarding does not affect the measurement.

With Podman:

```sh
podman run --rm --network host ghcr.io/eriksremess/speedtest:latest
```

With Docker:

```sh
docker run --rm --network host ghcr.io/eriksremess/speedtest:latest
```

Open `http://localhost:8080`. From another machine, use the server's LAN address.

By default, speedtest binds to `::`. With host networking, this can make both ports reachable from other machines. Use `--bind 127.0.0.1` for local-only access.

If host networking is unavailable, publish both ports instead. Replace `docker` with `podman` if preferred:

```sh
docker run --rm -p 8080:8080 -p 5201:5201 \
  ghcr.io/eriksremess/speedtest:latest --bind 0.0.0.0
```

## Docker Compose

Save this as `compose.yaml` on a Linux host:

```yaml
services:
  speedtest:
    image: ghcr.io/eriksremess/speedtest:latest
    network_mode: host
```

Start the service with:

```sh
docker compose up -d
```

Stop it with:

```sh
docker compose down
```

## Run from source

With a Rust toolchain and a checkout of this repository:

```sh
cargo run --release
```

The same default ports and bind address described above apply.

## Use iperf3

The iperf3-compatible server supports common TCP client commands:

```sh
iperf3 -c SERVER_ADDRESS
iperf3 -c SERVER_ADDRESS -R
iperf3 -c SERVER_ADDRESS -P 4
```

It supports forward, reverse (`-R`), and parallel (`-P`) tests. UDP, SCTP, bidirectional mode, authentication, and advanced socket-tuning options are not supported. One iperf3 test runs at a time; the browser test remains available concurrently.

## Configuration

```text
--bind ADDRESS       Listen address (default: ::)
--http-port PORT     Web/API port (default: 8080)
--iperf-port PORT    iperf3 port (default: 5201)
--no-iperf           Disable the iperf3 server
```

Pass options directly to the binary or after the container image name. When using Cargo, put them after `--`:

```sh
cargo run --release -- --no-iperf
```

With Docker, the equivalent container command is:

```sh
docker run --rm --network host \
  ghcr.io/eriksremess/speedtest:latest --no-iperf
```

The same arguments work after the image name with Podman. In `compose.yaml`, use `command: ["--no-iperf"]` under the service.

## What it measures

The browser test measures median request latency, average variation between consecutive latency samples (jitter), and streamed download and upload throughput. These are application-path measurements, so browser memory, HTTP framing, and runtime overhead are part of the result. Use iperf3 for lower-level network benchmarking.

## Build the container image

The minimal image runs as a non-root user. Build it locally with either engine:

```sh
podman build -t speedtest -f Containerfile .
docker build -t speedtest -f Containerfile .
```

Run the local image with the same options shown in the quick start. Replace `podman` with `docker` if preferred:

```sh
podman run --rm --network host speedtest
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

## Health check

The HTTP health endpoint is available at `/health` and returns `{"status":"ok"}`.
