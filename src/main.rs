mod http;
mod iperf;

use std::env;
use std::io;
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener};
use std::thread;

#[derive(Clone, Copy)]
struct Config {
    http_addr: SocketAddr,
    iperf_addr: Option<SocketAddr>,
}

fn main() -> io::Result<()> {
    let config = parse_args()?;
    let iperf_listener = config
        .iperf_addr
        .map(|addr| bind(addr, "iperf3"))
        .transpose()?;
    let http_listener = bind(config.http_addr, "HTTP")?;

    println!("speedtest");
    println!("  web     http://{}", display_addr(config.http_addr));
    if let (Some(addr), Some(listener)) = (config.iperf_addr, iperf_listener) {
        println!(
            "  iperf3  iperf3 -c {} -p {}",
            client_host(addr.ip()),
            addr.port()
        );

        thread::Builder::new()
            .name("iperf3-listener".into())
            .spawn(move || {
                if let Err(error) = iperf::serve(listener) {
                    eprintln!("iperf3 server stopped: {error}");
                }
            })?;
    } else {
        println!("  iperf3  disabled");
    }

    http::serve(http_listener)
}

fn bind(addr: SocketAddr, service: &str) -> io::Result<TcpListener> {
    TcpListener::bind(addr).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot bind {service} listener on {addr}: {error}"),
        )
    })
}

fn parse_args() -> io::Result<Config> {
    let mut http_port = 8080;
    let mut iperf_port = 5201;
    let mut iperf_enabled = true;
    let mut bind = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => bind = value(&mut args, "--bind")?.parse().map_err(invalid)?,
            "--http-port" => {
                http_port = value(&mut args, "--http-port")?.parse().map_err(invalid)?
            }
            "--iperf-port" => {
                iperf_port = value(&mut args, "--iperf-port")?.parse().map_err(invalid)?
            }
            "--no-iperf" => iperf_enabled = false,
            "-h" | "--help" => {
                println!(
                    "Usage: speedtest [--bind ADDRESS] [--http-port PORT] [--iperf-port PORT] [--no-iperf]\n\nDefaults: --bind :: --http-port 8080 --iperf-port 5201"
                );
                std::process::exit(0);
            }
            _ => return Err(invalid(format!("unknown argument: {arg}"))),
        }
    }

    Ok(Config {
        http_addr: SocketAddr::new(bind, http_port),
        iperf_addr: iperf_enabled.then(|| SocketAddr::new(bind, iperf_port)),
    })
}

fn value(args: &mut impl Iterator<Item = String>, name: &str) -> io::Result<String> {
    args.next()
        .ok_or_else(|| invalid(format!("missing value for {name}")))
}

fn invalid(error: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, error.to_string())
}

fn display_addr(addr: SocketAddr) -> String {
    if addr.ip().is_unspecified() {
        format!("localhost:{}", addr.port())
    } else {
        addr.to_string()
    }
}

fn client_host(ip: IpAddr) -> String {
    if ip.is_unspecified() {
        "<server>".into()
    } else {
        ip.to_string()
    }
}
