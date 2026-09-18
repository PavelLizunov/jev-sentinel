use clap::{Parser, Subcommand};
use colored::*;
use jev_sentinel::actions::{Notifier, SelfHealingManager};
use jev_sentinel::collectors::collect_all;
use jev_sentinel::config::SentinelConfig;
use jev_sentinel::core::models::TargetStatus;
use jev_sentinel::core::JevClient;
use std::path::PathBuf;
use std::time::Duration;
use tracing::{error, info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(
    name = "jev-sentinel",
    author = "Homelab & Open Source Contributors",
    version = "0.1.0",
    about = "Universal System 1 infrastructure watchdog and self-healing daemon powered by TypeSafe Jev"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a starter sentinel.yaml configuration file
    Init {
        #[arg(short, long, default_value = "sentinel.yaml")]
        path: PathBuf,
    },
    /// Run a single observation round and print the TypeSafe Jev System 1 evaluation
    Check {
        #[arg(short, long, default_value = "sentinel.yaml")]
        config: PathBuf,
        /// Also dispatch Telegram alerts if configured
        #[arg(short, long)]
        alert: bool,
    },
    /// Run continuous infrastructure monitoring daemon
    Run {
        #[arg(short, long, default_value = "sentinel.yaml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path } => {
            init_config(&path)?;
        }
        Commands::Check { config, alert } => {
            setup_logging(Level::INFO);
            run_check(&config, alert).await?;
        }
        Commands::Run { config } => {
            setup_logging(Level::INFO);
            run_daemon(&config).await?;
        }
    }

    Ok(())
}

fn setup_logging(level: Level) {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .compact()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

fn init_config(path: &PathBuf) -> anyhow::Result<()> {
    if path.exists() {
        println!(
            "{}",
            format!("File '{}' already exists. Aborting to prevent overwrite.", path.display()).yellow()
        );
        return Ok(());
    }

    let template = include_str!("../sentinel.example.yaml");
    std::fs::write(path, template)?;
    println!(
        "{}",
        format!("Created starter configuration at '{}'!", path.display()).green().bold()
    );
    println!("Edit this file to specify your servers, and run 'jev-sentinel check' to verify.");
    Ok(())
}

async fn run_check(config_path: &PathBuf, alert: bool) -> anyhow::Result<()> {
    println!("{}", "=========================================================".cyan());
    println!("{}", "       Jev Sentinel — System 1 Infrastructure Check       ".cyan().bold());
    println!("{}", "=========================================================".cyan());

    let config = SentinelConfig::from_file(config_path)?;
    println!("Loaded config: {} targets configured", config.targets.len());

    println!("\n{}", "1. Collecting Multi-Node Telemetry...".bold());
    let t0 = std::time::Instant::now();
    let snapshot = collect_all(&config.targets, config.daemon.concurrency).await;
    let elapsed = t0.elapsed();

    println!("\n{:<20} {:<15} {:<12} {:<10}", "TARGET", "TYPE", "STATUS", "LATENCY");
    println!("{:-<60}", "");

    let mut online_count = 0;
    for t in &snapshot.targets {
        let status_colored = match t.status {
            TargetStatus::Online => {
                online_count += 1;
                "ONLINE".green().bold()
            }
            TargetStatus::Degraded => "DEGRADED".yellow().bold(),
            TargetStatus::Unreachable => "UNREACHABLE".red().bold(),
            TargetStatus::Timeout => "TIMEOUT".red().bold(),
        };

        println!(
            "{:<20} {:<15} {:<12} {:>8.1}ms",
            t.target_name,
            t.target_type,
            status_colored,
            t.latency_ms
        );
    }
    println!("{:-<60}", "");
    println!(
        "Collection finished in {:.2}s ({}/{} targets online)",
        elapsed.as_secs_f64(),
        online_count,
        snapshot.targets.len()
    );

    println!("\n{}", "2. Dispatching to TypeSafe Jev System 1 Model...".bold());
    let jev_client = JevClient::new(
        config.jev.api_key.clone(),
        Some(config.jev.model.clone()),
        Some(config.jev.base_url.clone()),
        config.jev.egress_proxy.clone(),
        Some(config.jev.timeout_seconds),
    )?;

    let t_eval = std::time::Instant::now();
    let decision = match jev_client.evaluate_snapshot(&snapshot).await {
        Ok(d) => d,
        Err(e) => {
            println!("{}", format!("Jev API Error: {}", e).red().bold());
            return Err(e.into());
        }
    };
    let eval_dt = t_eval.elapsed();

    println!("\n{}", "=========================================================".cyan());
    println!("{}", "                 JEV SYSTEM 1 DECISION                   ".cyan().bold());
    println!("{}", "=========================================================".cyan());

    let health_colored = match decision.system_health.as_str() {
        "healthy" => "HEALTHY".green().bold(),
        "degraded" => "DEGRADED".yellow().bold(),
        "critical" => "CRITICAL".red().bold(),
        _ => "UNKNOWN".white().bold(),
    };

    println!("• System Health:     {}", health_colored);
    println!("• Confidence:        {:.1}%", decision.health_confidence * 100.0);
    println!("• Risk Score:        {:.2} / 1.00", decision.risk_score);
    println!(
        "• Action Required:   {}",
        if decision.action_required {
            "YES".red().bold()
        } else {
            "NO".green()
        }
    );
    println!("• Suggested Action:  {}", decision.suggested_action.yellow().bold());
    println!("• Jev Response Time: {:.3}s", eval_dt.as_secs_f64());
    println!("{}", "=========================================================".cyan());

    if alert {
        let notifier = Notifier::new(config.alerting.telegram.clone());
        notifier.dispatch_decision(&decision, &snapshot).await?;
    }

    if decision.is_critical() {
        std::process::exit(1);
    }

    Ok(())
}

async fn run_daemon(config_path: &PathBuf) -> anyhow::Result<()> {
    let config = SentinelConfig::from_file(config_path)?;
    info!(
        "Starting jev-sentinel daemon (interval: {}s, targets: {})",
        config.daemon.interval_seconds,
        config.targets.len()
    );

    let jev_client = JevClient::new(
        config.jev.api_key.clone(),
        Some(config.jev.model.clone()),
        Some(config.jev.base_url.clone()),
        config.jev.egress_proxy.clone(),
        Some(config.jev.timeout_seconds),
    )?;

    let notifier = Notifier::new(config.alerting.telegram.clone());
    let self_healing = SelfHealingManager::new(config.self_healing.clone());

    let mut interval = tokio::time::interval(Duration::from_secs(config.daemon.interval_seconds));

    loop {
        interval.tick().await;

        let snapshot = collect_all(&config.targets, config.daemon.concurrency).await;
        match jev_client.evaluate_snapshot(&snapshot).await {
            Ok(decision) => {
                info!(
                    "Jev verdict: health='{}', risk={:.2}, action='{}'",
                    decision.system_health, decision.risk_score, decision.suggested_action
                );

                if let Err(e) = notifier.dispatch_decision(&decision, &snapshot).await {
                    error!("Notifier error: {}", e);
                }

                if let Err(e) = self_healing.evaluate_and_heal(&decision).await {
                    error!("Self-healing error: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to evaluate snapshot via TypeSafe Jev: {}", e);
            }
        }
    }
}
