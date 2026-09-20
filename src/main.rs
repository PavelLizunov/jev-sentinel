use clap::{Parser, Subcommand};
use colored::*;
use jev_sentinel::actions::{
    AlertEvent, AlertReason, AlertSeverity, AlertSource, Notifier, SelfHealingManager,
};
use jev_sentinel::collectors::collect_all;
use jev_sentinel::config::SentinelConfig;
use jev_sentinel::core::models::TargetStatus;
use jev_sentinel::core::JevClient;
use jev_sentinel::web::run_web_server;
use std::path::PathBuf;
use std::sync::Arc;
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
        /// Also dispatch Telegram alerts if configured and severity threshold is met
        #[arg(short, long)]
        alert: bool,
        /// Force dispatch Telegram alert regardless of severity threshold
        #[arg(long)]
        force_alert: bool,
    },
    /// Dispatch a test message to verify Telegram alert connectivity and formatting
    TestAlert {
        #[arg(short, long, default_value = "sentinel.yaml")]
        config: PathBuf,
    },
    /// Run continuous infrastructure monitoring daemon
    Run {
        #[arg(short, long, default_value = "sentinel.yaml")]
        config: PathBuf,
        /// Enable embedded web monitoring dashboard
        #[arg(long)]
        web: bool,
        /// Listen address for web dashboard (e.g. 0.0.0.0:8088)
        #[arg(long)]
        listen: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { path } => {
            init_config(&path)?;
        }
        Commands::Check {
            config,
            alert,
            force_alert,
        } => {
            setup_logging(Level::INFO);
            run_check(&config, alert, force_alert).await?;
        }
        Commands::TestAlert { config } => {
            setup_logging(Level::INFO);
            run_test_alert(&config).await?;
        }
        Commands::Run {
            config,
            web,
            listen,
        } => {
            setup_logging(Level::INFO);
            run_daemon(&config, web, listen).await?;
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
            format!(
                "File '{}' already exists. Aborting to prevent overwrite.",
                path.display()
            )
            .yellow()
        );
        return Ok(());
    }

    let template = include_str!("../sentinel.example.yaml");
    std::fs::write(path, template)?;
    println!(
        "{}",
        format!("Created starter configuration at '{}'!", path.display())
            .green()
            .bold()
    );
    println!("Edit this file to specify your servers, and run 'jev-sentinel check' to verify.");
    Ok(())
}

async fn run_check(config_path: &PathBuf, alert: bool, force_alert: bool) -> anyhow::Result<()> {
    println!(
        "{}",
        "=========================================================".cyan()
    );
    println!(
        "{}",
        "       Jev Sentinel — System 1 Infrastructure Check       "
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "=========================================================".cyan()
    );

    let config = SentinelConfig::from_file(config_path)?;
    println!("Loaded config: {} targets configured", config.targets.len());

    println!("\n{}", "1. Collecting Multi-Node Telemetry...".bold());
    let t0 = std::time::Instant::now();
    let snapshot = collect_all(&config.targets, config.daemon.concurrency).await;
    let elapsed = t0.elapsed();

    println!(
        "\n{:<20} {:<15} {:<12} {:<10}",
        "TARGET", "TYPE", "STATUS", "LATENCY"
    );
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
            TargetStatus::Timeout => "TIMEOUT".yellow().bold(),
            TargetStatus::Unknown => "UNKNOWN".yellow().bold(),
        };

        println!(
            "{:<20} {:<15} {:<12} {:>8.1}ms",
            t.target_name, t.target_type, status_colored, t.latency_ms
        );
    }
    println!("{:-<60}", "");
    println!(
        "Collection finished in {:.2}s ({}/{} targets online)",
        elapsed.as_secs_f64(),
        online_count,
        snapshot.targets.len()
    );

    println!(
        "\n{}",
        "2. Dispatching to TypeSafe Jev System 1 Model...".bold()
    );
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

    println!(
        "\n{}",
        "=========================================================".cyan()
    );
    println!(
        "{}",
        "                 JEV SYSTEM 1 DECISION                   "
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "=========================================================".cyan()
    );

    let health_colored = match decision.system_health.as_str() {
        "healthy" => "HEALTHY".green().bold(),
        "degraded" => "DEGRADED".yellow().bold(),
        "critical" => "CRITICAL".red().bold(),
        _ => "UNKNOWN".white().bold(),
    };

    println!("• System Health:     {}", health_colored);
    match decision.health_confidence {
        Some(c) => println!("• Confidence:        {:.1}%", c * 100.0),
        None => println!("• Confidence:        unavailable"),
    }
    println!("• Risk Score:        {:.2} / 1.00", decision.risk_score);
    println!(
        "• Action Required:   {}",
        if decision.action_required {
            "YES".red().bold()
        } else {
            "NO".green()
        }
    );
    println!(
        "• Suggested Action:  {}",
        decision.suggested_action.yellow().bold()
    );
    println!("• Jev Response Time: {:.3}s", eval_dt.as_secs_f64());
    println!(
        "{}",
        "=========================================================".cyan()
    );

    if alert || force_alert {
        let notifier = Notifier::new_with_fallback(
            config.alerting.telegram.clone(),
            config.jev.egress_proxy.as_deref(),
        );
        if force_alert {
            println!("\n{}", "Dispatching forced alert to Telegram...".bold());
            notifier
                .dispatch_decision_forced(&decision, &snapshot)
                .await?;
            println!(
                "{}",
                "Telegram alert dispatched successfully!".green().bold()
            );
        } else {
            notifier.dispatch_decision(&decision, &snapshot).await?;
        }
    }

    if decision.is_critical() {
        std::process::exit(1);
    }

    Ok(())
}

async fn run_test_alert(config_path: &PathBuf) -> anyhow::Result<()> {
    println!(
        "{}",
        "=========================================================".cyan()
    );
    println!(
        "{}",
        "       Jev Sentinel — Telegram Connectivity Test         "
            .cyan()
            .bold()
    );
    println!(
        "{}",
        "=========================================================".cyan()
    );

    let config = SentinelConfig::from_file(config_path)?;
    let Some(tg) = &config.alerting.telegram else {
        println!(
            "{}",
            "Error: No [alerting.telegram] section configured in configuration file."
                .red()
                .bold()
        );
        anyhow::bail!(
            "Telegram alerting not configured in '{}'",
            config_path.display()
        );
    };

    let token_prefix = if tg.bot_token.len() > 8 {
        format!("{}***", &tg.bot_token[..8])
    } else {
        "***".to_string()
    };
    println!("• Bot Token:    {}", token_prefix);
    println!("• Chat ID:      {}", tg.chat_id);
    println!("• Min Severity: {}", tg.min_severity);
    if let Some(proxy) = &tg.proxy {
        println!("• Proxy:        {}", proxy);
    } else if let Some(proxy) = &config.jev.egress_proxy {
        println!("• Proxy (Jev):  {}", proxy);
    }

    let notifier = Notifier::new_with_fallback(
        config.alerting.telegram.clone(),
        config.jev.egress_proxy.as_deref(),
    );

    println!("\n{}", "Sending test notification to Telegram...".bold());
    let t0 = std::time::Instant::now();
    notifier.send_test_alert().await?;
    let elapsed = t0.elapsed();

    println!(
        "\n{}",
        format!(
            "Successfully delivered Telegram test alert in {:.2}s!",
            elapsed.as_secs_f64()
        )
        .green()
        .bold()
    );
    println!(
        "{}",
        "=========================================================".cyan()
    );

    Ok(())
}

async fn run_daemon(
    config_path: &PathBuf,
    web_override: bool,
    listen_override: Option<String>,
) -> anyhow::Result<()> {
    let mut config = SentinelConfig::from_file(config_path)?;

    // Handle CLI flags
    if web_override {
        config.web.enabled = true;
    }
    if let Some(l) = listen_override {
        config.web.listen = l;
        config.web.enabled = true;
    }

    info!(
        "Starting jev-sentinel daemon (interval: {}s, targets: {}, web: {})",
        config.daemon.interval_seconds,
        config.targets.len(),
        if config.web.enabled {
            &config.web.listen
        } else {
            "disabled"
        }
    );

    let jev_client = Arc::new(JevClient::new(
        config.jev.api_key.clone(),
        Some(config.jev.model.clone()),
        Some(config.jev.base_url.clone()),
        config.jev.egress_proxy.clone(),
        Some(config.jev.timeout_seconds),
    )?);

    let notifier = Arc::new(Notifier::new_with_fallback(
        config.alerting.telegram.clone(),
        config.jev.egress_proxy.as_deref(),
    ));
    let self_healing = SelfHealingManager::new(config.self_healing.clone());

    // Bounded background alert queue: decouples Telegram delivery from daemon polling cycles
    let (alert_tx, mut alert_rx) = tokio::sync::mpsc::channel::<AlertEvent>(128);
    let notifier_worker = Arc::clone(&notifier);
    tokio::spawn(async move {
        while let Some(event) = alert_rx.recv().await {
            if let Err(e) = notifier_worker.dispatch_alert_event(&event).await {
                error!("Background alert dispatch error: {}", e);
            }
        }
    });

    // Optional web dashboard state channels
    let mut dashboard_history =
        jev_sentinel::web::models::DashboardHistory::new(config.daemon.interval_seconds);
    let (status_tx, status_rx) = tokio::sync::watch::channel(Arc::new(dashboard_history.initial()));
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    if config.web.enabled {
        let listen_addr = config.web.listen.clone();
        let s_rx = status_rx.clone();
        let s_tx = status_tx.clone();
        let j_client = jev_client.clone();
        tokio::spawn(async move {
            if let Err(e) = run_web_server(listen_addr, s_rx, s_tx, j_client, shutdown_rx).await {
                error!("Web dashboard server stopped with error: {}", e);
            }
        });
    }

    let mut interval = tokio::time::interval(Duration::from_secs(config.daemon.interval_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let mut cycle_id = 0u64;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("Shutdown signal received, terminating daemon gracefully...");
                let _ = shutdown_tx.send(true);
                break Ok(());
            }
            _ = interval.tick() => {
                cycle_id += 1;

                let t_collect = std::time::Instant::now();
                let snapshot = collect_all(&config.targets, config.daemon.concurrency).await;
                let collect_latency_ms = t_collect.elapsed().as_secs_f64() * 1000.0;

                // 1. Immediately publish fresh telemetry to Web Dashboard so notifications cannot delay observations
                if config.web.enabled {
                    let mut next = dashboard_history.observe(cycle_id, &snapshot, collect_latency_ms);
                    status_tx.send_modify(|current| {
                        next.categories = current.categories.clone();
                        for target in &mut next.targets {
                            target.category = current.targets.iter().find(|t| t.id == target.id).and_then(|t| t.category.clone());
                        }
                        next.view_revision = current.view_revision.parse::<u64>().unwrap_or(0).saturating_add(1).to_string();
                        *current = Arc::new(next);
                    });
                }

                // 2. Autonomous local alerts queued to background worker based on raw observations
                for target in &snapshot.targets {
                    if target.status != TargetStatus::Online {
                        let (sev, reason, title, msg) = match target.status {
                            TargetStatus::Timeout => (
                                AlertSeverity::Warning,
                                AlertReason::ObservationUnavailable,
                                "Observation Unavailable",
                                format!(
                                    "Не удалось проверить цель '{}' ({}): таймаут",
                                    target.target_name, target.target_type
                                ),
                            ),
                            TargetStatus::Unreachable => {
                                // Structural classification using target_type and metric fields rather than error text
                                let status_code = target.metrics.get("status_code").and_then(|v| v.as_u64());
                                if target.target_type == "http_probe" && status_code.is_some_and(|c| c >= 400) {
                                    (
                                        AlertSeverity::Warning,
                                        AlertReason::ServiceCheckFailed,
                                        "Service Check Failed",
                                        format!(
                                            "Проверка HTTP-сервиса '{}' вернула статус {}: {}",
                                            target.target_name,
                                            status_code.unwrap_or(0),
                                            target.error_message.as_deref().unwrap_or("ошибка статуса")
                                        ),
                                    )
                                } else {
                                    (
                                        AlertSeverity::Warning,
                                        AlertReason::ObservationUnavailable,
                                        "Observation Unavailable",
                                        format!(
                                            "Не удалось проверить цель '{}' ({}): {}",
                                            target.target_name,
                                            target.target_type,
                                            target.error_message.as_deref().unwrap_or("недоступен")
                                        ),
                                    )
                                }
                            }
                            TargetStatus::Degraded => (
                                AlertSeverity::Warning,
                                AlertReason::ServiceDegraded,
                                "Service Degraded",
                                format!(
                                    "Проверка сервиса '{}' зафиксировала деградацию: {}",
                                    target.target_name,
                                    target.error_message.as_deref().unwrap_or("ошибка проверки")
                                ),
                            ),
                            TargetStatus::Unknown => (
                                AlertSeverity::Warning,
                                AlertReason::ObservationUnavailable,
                                "Observation Inconclusive",
                                format!(
                                    "Не удалось получить валидные данные от цели '{}': {}",
                                    target.target_name,
                                    target
                                        .error_message
                                        .as_deref()
                                        .unwrap_or("некорректный формат ответа")
                                ),
                            ),
                            TargetStatus::Online => unreachable!(),
                        };
                        let event = AlertEvent {
                            source: AlertSource::LocalRule,
                            target_id: Some(target.target_name.clone()),
                            severity: sev,
                            reason_code: reason,
                            title: title.to_string(),
                            message: msg,
                            timestamp: chrono::Utc::now(),
                        };
                        if let Err(e) = alert_tx.try_send(event) {
                            error!("Alert queue full or closed, dropping event for '{}': {}", target.target_name, e);
                        }
                    } else {
                        // Clear cooldown on recovery so future issues immediately alert
                        notifier.clear_alert_cooldown(&target.target_name);
                    }
                }

                let t_eval = std::time::Instant::now();
                match jev_client.evaluate_snapshot(&snapshot).await {
                    Ok(decision) => {
                        let eval_latency_ms = t_eval.elapsed().as_secs_f64() * 1000.0;
                        info!(
                            "Jev verdict (cycle {}): health='{}', risk={:.2}, action='{}'",
                            cycle_id,
                            decision.system_health,
                            decision.risk_score,
                            decision.suggested_action
                        );

                        if config.web.enabled {
                            status_tx.send_modify(|current| {
                                let next = Arc::make_mut(current);
                                next.set_decision(&decision, eval_latency_ms);
                                next.view_revision = next.view_revision.parse::<u64>().unwrap_or(0).saturating_add(1).to_string();
                            });
                        }

                        // Reset advisor cooldown upon successful recovery
                        notifier.clear_advisor_cooldown();

                        // Route routine Jev alert through background queue with deduplication
                        let is_info = config
                            .alerting
                            .telegram
                            .as_ref()
                            .is_some_and(|tg| tg.min_severity.eq_ignore_ascii_case("info"));

                        if is_info || decision.is_warning() || decision.is_critical() {
                            let conf_text = match decision.health_confidence {
                                Some(c) => format!(". Уверенность: {:.0}%", c * 100.0),
                                None => String::new(),
                            };
                            let event = AlertEvent {
                                source: AlertSource::JevAdvisor,
                                target_id: None,
                                severity: if decision.is_critical() {
                                    AlertSeverity::Critical
                                } else if decision.is_warning() {
                                    AlertSeverity::Warning
                                } else {
                                    AlertSeverity::Info
                                },
                                reason_code: AlertReason::RiskElevated,
                                title: format!(
                                    "Jev System 1: {}",
                                    decision.system_health.to_uppercase()
                                ),
                                message: format!(
                                    "Оценка риска: {:.2} / 1.00. Рекомендация: {}{}",
                                    decision.risk_score, decision.suggested_action, conf_text
                                ),
                                timestamp: chrono::Utc::now(),
                            };
                            if let Err(e) = alert_tx.try_send(event) {
                                error!("Alert queue full or closed for Jev alert: {}", e);
                            }
                        }

                        if let Err(e) = self_healing.evaluate_and_heal(&decision).await {
                            error!("Self-healing error: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Failed to evaluate snapshot via TypeSafe Jev: {}", e);
                        if config.web.enabled {
                            status_tx.send_modify(|current| {
                                let next = Arc::make_mut(current);
                                next.set_error(matches!(e, jev_sentinel::error::SentinelError::InvalidJevResponse(_)));
                                next.view_revision = next.view_revision.parse::<u64>().unwrap_or(0).saturating_add(1).to_string();
                            });
                        }

                        // Informational alert on Jev failure (deduplicated by reason_code: AdvisorUnavailable)
                        let event = AlertEvent {
                            source: AlertSource::LocalRule,
                            target_id: None,
                            severity: AlertSeverity::Warning,
                            reason_code: AlertReason::AdvisorUnavailable,
                            title: "AI Advisor Unavailable".to_string(),
                            message: format!(
                                "AI-рекомендации временно недоступны: {}. Локальный мониторинг активен.",
                                e
                            ),
                            timestamp: chrono::Utc::now(),
                        };
                        if let Err(err) = alert_tx.try_send(event) {
                            error!("Alert queue full or closed for advisor failure: {}", err);
                        }
                    }
                }
            }
        }
    }
}
