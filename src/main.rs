use std::io::{self, Read};
use std::process::ExitCode;

use chrono::Utc;
use codex_usage_watch::hooks::{
    HookEvent, install_hooks, run_hook, uninstall_hooks, validate_installed_hooks,
};
use codex_usage_watch::{
    CalibrationReport, DiscoveryOptions, IngestOptions, StateStore, TrackerConfig, WindowStatus,
    cached_release_metadata, discover_recent_transcripts, import_history, preview_history,
};

const FAIL_OPEN_JSON: &str = r#"{"continue":true,"suppressOutput":true}"#;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("codex-5h: {error}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => print_help(),
        [flag] if flag == "--help" || flag == "-h" || flag == "help" => print_help(),
        [flag] if flag == "--version" || flag == "-V" || flag == "version" => {
            println!("codex-5h {}", env!("CARGO_PKG_VERSION"));
        }
        [command, event] if command == "hook" => {
            let event = HookEvent::parse(event).ok_or("unknown hook event")?;
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            match run_hook(event, &input, Utc::now()) {
                Ok(output) => println!("{output}"),
                Err(error) => {
                    // Hooks always fail open. Diagnostics stay off stdout because
                    // stdout is a strict JSON protocol surface.
                    eprintln!("codex-5h hook diagnostic: {error}");
                    println!("{FAIL_OPEN_JSON}");
                }
            }
        }
        [command, flag] if command == "install" && flag == "--confirm" => {
            let path = install_hooks(true)?;
            println!("Installed Codex usage-watch hooks in {}", path.display());
        }
        [command, flag] if command == "uninstall" && flag == "--confirm" => {
            let path = uninstall_hooks(true)?;
            println!("Removed Codex usage-watch hooks from {}", path.display());
        }
        [command] if command == "install" => install_hooks(false).map(|_| ())?,
        [command] if command == "uninstall" => uninstall_hooks(false).map(|_| ())?,
        [command] if command == "status" => print_status(false)?,
        [command, flag] if command == "status" && flag == "--json" => print_status(true)?,
        [command] if command == "refresh" => refresh(None)?,
        [command, flag, transcript] if command == "refresh" && flag == "--transcript" => {
            refresh(Some(std::path::Path::new(transcript)))?
        }
        [command] if command == "history" => print_history(false)?,
        [command, flag] if command == "history" && flag == "--json" => print_history(true)?,
        [command] if command == "setup" => run_setup(SetupMode::Interactive)?,
        [command, flag] if command == "setup" && flag == "--preview" => {
            run_setup(SetupMode::PreviewOnly)?
        }
        [command, flag] if command == "setup" && flag == "--skip-import" => {
            run_setup(SetupMode::SkipImport)?
        }
        [command, flag, confirm]
            if command == "setup" && flag == "--import" && confirm == "--confirm" =>
        {
            run_setup(SetupMode::ConfirmedImport)?
        }
        [command] if command == "analyze" => print_analysis(false)?,
        [command, flag] if command == "analyze" && flag == "--json" => print_analysis(true)?,
        [command] if command == "doctor" => print_doctor()?,
        [command, flag] if command == "doctor" && flag == "--compat" => {
            print_compatibility_doctor(false)?
        }
        [command, flag, release]
            if command == "doctor" && flag == "--compat" && release == "--refresh-releases" =>
        {
            print_compatibility_doctor(true)?
        }
        [command, action, value, flag]
            if command == "calibration" && action == "apply" && flag == "--confirm" =>
        {
            let value: f64 = value.parse()?;
            let mut store = StateStore::open(TrackerConfig::from_env()?)?;
            store.apply_calibration(value, Utc::now())?;
            println!("Applied calibration {value:.3} weekly points to future local windows only.");
        }
        [command, destination, flag] if command == "backup" && flag == "--confirm" => {
            let store = StateStore::open(TrackerConfig::from_env()?)?;
            store.backup_database(std::path::Path::new(destination))?;
            println!("Created consistent SQLite backup at {destination}");
        }
        [command, flag] if command == "reset" && flag == "--confirm" => {
            let mut store = StateStore::open(TrackerConfig::from_env()?)?;
            if store.reset_current_window(Utc::now())? {
                println!(
                    "Archived the current local window. The next observation starts a new one."
                );
            } else {
                println!("No current local window exists; nothing changed.");
            }
        }
        _ => {
            return Err("invalid arguments; run codex-5h --help for commands and examples".into());
        }
    }
    Ok(())
}

fn print_help() {
    println!(
        "Codex Usage Watch {}\n\nLocal, non-blocking five-hour usage awareness. Estimates are not official quota or billing data.\n\nUSAGE:\n  codex-5h <COMMAND> [OPTIONS]\n\nCOMMANDS:\n  setup [--preview|--skip-import|--import --confirm]  Consent-first history setup\n  status [--json]                                    Show the current projection\n  refresh [--transcript PATH]                        Bounded refresh (at most 8 recent files)\n  history [--json]                                   Show recent windows and control events\n  analyze [--json]                                   Inspect calibration evidence\n  reset --confirm                                    Archive the current local window\n  doctor [--compat [--refresh-releases]]             Validate installation and compatibility\n  calibration apply POINTS --confirm                 Apply reviewed calibration to future windows\n  backup DESTINATION.sqlite3 --confirm               Create an integrity-checked database backup\n  install --confirm | uninstall --confirm            Add or remove only this tool's hooks\n\nOPTIONS:\n  -h, --help       Print help and exit 0\n  -V, --version    Print version and exit 0\n\nEXAMPLES:\n  codex-5h setup --preview\n  codex-5h status --json\n  codex-5h refresh\n  codex-5h doctor\n\nEXIT STATUS:\n  0 success; 2 invalid arguments, configuration, or runtime failure",
        env!("CARGO_PKG_VERSION")
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetupMode {
    Interactive,
    PreviewOnly,
    SkipImport,
    ConfirmedImport,
}

fn run_setup(mode: SetupMode) -> Result<(), Box<dyn std::error::Error>> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".codex"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".codex"));
    let preview = preview_history(&codex_home.join("sessions"))?;
    println!("Codex Usage Watch setup");
    println!("History location     {}", preview.sessions_root.display());
    println!(
        "Candidate files      {} JSONL transcript(s)",
        preview.candidate_count
    );
    println!(
        "Candidate date range {}",
        preview
            .earliest_modified_at
            .zip(preview.latest_modified_at)
            .map(|(start, end)| format!("{} to {}", start.to_rfc3339(), end.to_rfc3339()))
            .unwrap_or_else(|| "none".to_string())
    );
    println!(
        "Would read           rate-limit windows, timestamps, model, plan, tier, and schema metadata only"
    );
    println!(
        "Would not retain     prompts, responses, tool arguments, source code, or arbitrary payloads"
    );
    if mode == SetupMode::PreviewOnly {
        println!("Import               not started (preview only)");
        return Ok(());
    }
    if mode == SetupMode::SkipImport {
        println!("Import               skipped by choice");
        println!(
            "Five-hour estimate   unknown until an exact-plan baseline or compatible evidence is available"
        );
        println!("Live collection      available after hook installation");
        return Ok(());
    }
    let consent = if mode == SetupMode::ConfirmedImport {
        true
    } else {
        use std::io::Write;
        print!("Import this history now? [y/N] ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    };
    if !consent {
        println!("Import               declined; no transcript contents were read");
        println!("Live collection      available after hook installation");
        return Ok(());
    }
    let mut store = StateStore::open(TrackerConfig::from_env()?)?;
    let summary = import_history(&mut store, &preview, Utc::now());
    println!("Imported files       {}", summary.imported_file_count);
    println!(
        "Structured evidence  {} observation(s)",
        summary.observation_count
    );
    println!(
        "Paired evidence      {} observation(s)",
        summary.paired_observation_count
    );
    println!(
        "Weekly-only evidence {} observation(s)",
        summary.weekly_only_observation_count
    );
    println!("Diagnostics          {}", summary.diagnostic_count);
    println!("Unreadable files     {}", summary.failed_file_count);
    let running_version = detect_codex_version();
    let (identity, supported) =
        store.current_compatibility_identity(Some(&running_version), None, None)?;
    let compatibility = store.check_compatibility(identity, supported, Utc::now())?;
    println!("Compatibility        {:?}", compatibility.result);
    let report = store.analyze_calibration(Utc::now())?;
    if report.sample_count == 0 {
        println!(
            "Personal calibration not identifiable from imported history; future evidence collection will continue"
        );
    }
    println!("Calibration source   {}", report.calibration_id);
    println!("Confidence           {:?}", report.confidence);
    Ok(())
}

fn print_status(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let config = TrackerConfig::from_env()?;
    let mut store = StateStore::open(config.clone())?;
    let display = store.load_or_recover_display(Utc::now())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "contract": "codex-usage-watch.status.v1",
                "display": display,
                "active_calibration_weekly_points": store.active_calibration(),
                "warning_thresholds_percent": config.warning_thresholds(),
            }))?
        );
        return Ok(());
    }
    println!(
        "Five-hour estimate   {}",
        optional_percent(
            display.five_hour_estimate_percent,
            display.five_hour_value_source.as_deref()
        )
    );
    println!(
        "Weekly cost          {}",
        optional_points(display.weekly_points)
    );
    println!(
        "Window               {}",
        display
            .window_started_at
            .zip(display.window_ends_at)
            .map(|(start, end)| format!("{} to {}", start.to_rfc3339(), end.to_rfc3339()))
            .unwrap_or_else(|| "unavailable".to_string())
    );
    println!(
        "Data                 {}",
        match display.status {
            WindowStatus::Fresh => format!("fresh; {}s old", display.data_age_seconds.unwrap_or(0)),
            WindowStatus::Stale | WindowStatus::Expired => "stale".to_string(),
            WindowStatus::Unknown => "unavailable".to_string(),
        }
    );
    println!("Calibration          {:.3}", store.active_calibration());
    Ok(())
}

fn refresh(transcript: Option<&std::path::Path>) -> Result<(), Box<dyn std::error::Error>> {
    let now = Utc::now();
    let mut store = StateStore::open(TrackerConfig::from_env()?)?;
    let paths = if let Some(path) = transcript {
        vec![path.to_path_buf()]
    } else {
        discover_recent_transcripts(
            &codex_home().join("sessions"),
            now,
            DiscoveryOptions {
                lookback_days: 2,
                max_files: 8,
                max_entries_per_day: 256,
            },
        )?
    };
    let mut inserted_observations = 0;
    let mut inserted_diagnostics = 0;
    for path in &paths {
        let outcome = store.ingest_transcript(
            path,
            &IngestOptions {
                now,
                ..IngestOptions::default()
            },
        )?;
        inserted_observations += outcome.inserted_observations;
        inserted_diagnostics += outcome.inserted_diagnostics;
    }
    let display = store.load_or_recover_display(now)?;
    println!(
        "Refresh scope        {} transcript(s), maximum 8",
        paths.len()
    );
    println!("New observations     {inserted_observations}");
    println!("New diagnostics      {inserted_diagnostics}");
    println!(
        "Projection           {:?} · {}",
        display.status,
        store.paths().display.display()
    );
    Ok(())
}

fn print_history(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let store = StateStore::open(TrackerConfig::from_env()?)?;
    let windows = store.recent_windows(20)?;
    let controls = store.recent_control_events(20)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "windows": windows,
                "control_events": controls,
            }))?
        );
        return Ok(());
    }
    println!("Recent local windows (newest first)");
    if windows.is_empty() {
        println!("  none");
    }
    for window in windows {
        println!(
            "  {} · {} · 5h {:.0}% · week +{:.1} · {:?} · {}",
            window.started_at.to_rfc3339(),
            window.lifecycle,
            window.five_hour_estimate_percent,
            window.weekly_points,
            window.calibration_confidence,
            window.calibration_id
        );
    }
    if !controls.is_empty() {
        println!("Control audit");
        for event in controls {
            println!(
                "  {} · {} · {}",
                event.occurred_at.to_rfc3339(),
                event.event_type,
                event.detail
            );
        }
    }
    Ok(())
}

fn print_analysis(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = StateStore::open(TrackerConfig::from_env()?)?;
    let report = store.analyze_calibration(Utc::now())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human_analysis(&report);
    }
    Ok(())
}

fn print_compatibility_doctor(refresh_releases: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = StateStore::open(TrackerConfig::from_env()?)?;
    let codex_version = detect_codex_version();
    let (identity, supported) =
        store.current_compatibility_identity(Some(&codex_version), None, None)?;
    let check = store.check_compatibility(identity, supported, Utc::now())?;
    let release = cached_release_metadata(
        store.paths().directory.as_path(),
        Utc::now(),
        refresh_releases,
    );
    println!("Codex version       {}", check.identity.codex_version);
    println!("Plan                {}", check.identity.plan_type);
    println!(
        "Model               {} ({})",
        check.identity.model_slug, check.model_confidence
    );
    println!("Service tier        {}", check.identity.service_tier);
    println!("Hook schema         {}", check.hook_check);
    println!("Transcript          {}", check.transcript_check);
    println!("Rate-limit schema   {}", check.rate_limit_check);
    println!("Schema identity     {}", check.identity.schema_fingerprint);
    println!("Native adapter      {}", check.projection_check);
    println!(
        "Tracker/plugin      {} / {}",
        check.tracker_version, check.plugin_version
    );
    match release {
        Ok(Some(metadata)) => println!(
            "Release metadata    {} · {} · cached 24h",
            metadata.tag_name, metadata.html_url
        ),
        Ok(None) => println!("Release metadata    disabled/not cached; optional refresh available"),
        Err(error) => println!("Release metadata    unavailable ({error})"),
    }
    println!("Result              {:?}", check.result);
    println!("Requests continue   yes");
    Ok(())
}

fn print_doctor() -> Result<(), Box<dyn std::error::Error>> {
    let mut store = StateStore::open(TrackerConfig::from_env()?)?;
    let schema = store.schema_version()?;
    let display = store.load_or_recover_display(Utc::now())?;
    let executable = std::env::current_exe()?;
    let codex_home = codex_home();
    let hooks_path = codex_home.join("hooks.json");
    validate_installed_hooks(&executable)?;
    let sessions = codex_home.join("sessions");
    let sessions_access = if sessions.exists() {
        std::fs::read_dir(&sessions).is_ok()
    } else {
        true
    };
    println!("Executable          {}", executable.display());
    println!("State directory     {}", store.paths().directory.display());
    println!("Database schema     v{schema} supported");
    println!("Database snapshots  {}", store.snapshot_count()?);
    println!("Observations        {}", store.observation_count()?);
    println!("Diagnostics         {}", store.diagnostic_count()?);
    println!(
        "Display projection  v{} · {:?}",
        display.schema_version, display.status
    );
    println!(
        "Session metadata    {}",
        if sessions_access {
            "accessible"
        } else {
            "unreadable"
        }
    );
    println!(
        "Plugin hooks        installed and valid · {}",
        hooks_path.display()
    );
    println!("Requests continue   yes (all hooks fail open)");
    if !sessions_access {
        return Err("Codex session directory exists but is not readable".into());
    }
    Ok(())
}

fn codex_home() -> std::path::PathBuf {
    std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".codex"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".codex"))
}

fn detect_codex_version() -> String {
    if let Ok(version) = std::env::var("CODEX_VERSION") {
        return version;
    }
    std::process::Command::new("codex")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn print_human_analysis(report: &CalibrationReport) {
    println!("Calibration accuracy report");
    println!(
        "Current calibration   {}",
        report
            .current_calibration
            .map(|value| format!("{value:.3} weekly points"))
            .unwrap_or_else(|| "unavailable for this identity".to_string())
    );
    println!("Calibration ID       {}", report.calibration_id);
    println!(
        "Confidence           {:?} · {}",
        report.confidence, report.confidence_reason
    );
    println!(
        "Proposed calibration  {}",
        report
            .proposed_calibration
            .map(|value| format!("{value:.3} weekly points (review required)"))
            .unwrap_or_else(|| "unavailable".to_string())
    );
    println!(
        "Ground truth          {:?}; {} qualifying windows",
        report.ground_truth_status, report.sample_count
    );
    println!(
        "Weekly-only evidence  {} observations",
        report.weekly_only_observation_count
    );
    println!(
        "Excluded evidence     {} groups/observations",
        report.excluded_group_count
    );
    println!(
        "Data period           {}",
        report
            .data_period_start
            .zip(report.data_period_end)
            .map(|(start, end)| format!("{} to {}", start.to_rfc3339(), end.to_rfc3339()))
            .unwrap_or_else(|| "none".to_string())
    );
    println!(
        "Robust spread         {}",
        report
            .weighted_median
            .zip(report.q1)
            .zip(report.q3)
            .map(|((weighted, q1), q3)| format!(
                "weighted median {weighted:.3}; Q1 {q1:.3}; Q3 {q3:.3}; min {:.3}; max {:.3}; {} outlier(s)",
                report.minimum.unwrap_or(weighted),
                report.maximum.unwrap_or(weighted),
                report.outlier_count
            ))
            .unwrap_or_else(|| "unavailable".to_string())
    );
    println!("Recommendation        {}", report.recommendation_reason);
    println!("Auto-applied          no");
}

fn optional_percent(value: Option<f64>, source: Option<&str>) -> String {
    value
        .map(|value| match source {
            Some("real_server_five_hour") => format!("{value:.0}% used (real server window)"),
            _ => format!("{value:.0}% used (estimated)"),
        })
        .unwrap_or_else(|| "unavailable".to_string())
}

fn optional_points(value: Option<f64>) -> String {
    value
        .map(|value| format!("+{value:.1} points this window"))
        .unwrap_or_else(|| "unavailable".to_string())
}
