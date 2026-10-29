//! `malac-control`: computer and browser control over MCP.

use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use malac_control::discovery::{self, ServiceInfo};
use malac_control::{Config, Control};
use malac_mcp::server::{serve_stdio, serve_websocket};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "malac-control",
    version,
    about = "Computer and browser control over MCP"
)]
struct Cli {
    /// Config file [default: ~/.malac/control.toml]
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve MCP on stdin and stdout
    Stdio,
    /// Serve MCP over WebSocket and advertise it in ~/.malac/control.json
    Serve {
        #[arg(long, default_value = "127.0.0.1")]
        host: IpAddr,
        #[arg(long, default_value_t = 7878)]
        port: u16,
        /// Token clients must present [default: random]
        #[arg(long, env = "MALAC_CONTROL_TOKEN", hide_env_values = true)]
        token: Option<String>,
    },
    /// List configured and system browser profiles
    Profiles,
    /// Show the macOS permissions desktop control needs and who holds them
    Permissions {
        /// Ask for missing permissions and open System Settings
        #[arg(long)]
        request: bool,
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    // First, while this is the only thread: on macOS this may re-execute
    // the process so that it holds its own permissions.
    let owned = malac_control::permissions::own_permissions();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    if let Err(err) = owned {
        tracing::warn!("macOS will ask for permissions for the app that started this: {err:#}");
    }
    let cli = Cli::parse();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("Error: failed to start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(run(cli));
    // Stdin is read on a blocking thread that cannot be cancelled; waiting
    // for it after a signal would hang.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let config = match &cli.config {
        Some(path) => Config::from_file(path)?,
        None => Config::load()?,
    };
    match cli.command {
        Command::Stdio => {
            let control = Arc::new(Control::new(config));
            run_until_shutdown(&control, serve_stdio(control.clone())).await
        }
        Command::Serve { host, port, token } => {
            serve(config, SocketAddr::new(host, port), token).await
        }
        Command::Profiles => {
            print!("{}", malac_control::list_profiles(&config.browser));
            Ok(())
        }
        Command::Permissions { request, json } => {
            let report = malac_control::permissions::report(request);
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_permissions(&report);
            }
            Ok(())
        }
    }
}

fn print_permissions(report: &malac_control::permissions::Report) {
    let state = |granted: Option<bool>| match granted {
        Some(true) => "granted",
        Some(false) => "missing",
        None => "not needed",
    };
    println!("Screen Recording: {}", state(report.screen_recording));
    println!("Accessibility:    {}", state(report.accessibility));
    if let Some(app) = &report.app {
        println!("Granted to:       {} ({})", app.name, app.path);
    }
    println!("{}", report.hint);
}

async fn serve(config: Config, addr: SocketAddr, token: Option<String>) -> Result<()> {
    let token = token.unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string());
    let advertised = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv4Addr::LOCALHOST.into(), addr.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(Ipv6Addr::LOCALHOST.into(), addr.port())
        }
        _ => addr,
    };
    let info = ServiceInfo {
        url: format!("ws://{advertised}"),
        token: token.clone(),
        pid: std::process::id(),
    };
    let _publication = info.publish()?;
    tracing::info!(
        "advertising {} in {}",
        info.url,
        discovery::path()?.display()
    );
    let control = Arc::new(Control::new(config));
    let server = async {
        serve_websocket(control.clone(), addr, Some(token))
            .await
            .with_context(|| format!("cannot serve on {addr}"))
    };
    run_until_shutdown(&control, server).await
}

/// Runs `server` until it stops or the process is told to, then closes the
/// browsers this process launched.
async fn run_until_shutdown(
    control: &Control,
    server: impl Future<Output = Result<()>>,
) -> Result<()> {
    let result = tokio::select! {
        result = server => result,
        result = shutdown_signal() => result,
    };
    control.shutdown().await;
    result
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate = signal(SignalKind::terminate())?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result?,
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    Ok(())
}
