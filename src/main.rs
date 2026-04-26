//! Monitor and error-reporter of a `PellX` pellets burner.
//!
//! Intended to be run on a Raspberry Pi-equivalent device connected via GPIO
//! to terminals on the controller board of a `PellX` burner.
//!
//! Terminals 1 and 2 are electrically connected when the burner is operating
//! normally, and the circuit is broken when it is in an error state
//! (including on power failures).
//!
//! A notification is sent when this is detected. These can be sent as Slack
//! messages, as short emails via Batsign, and/or by invocation of an
//! external command (like `notify-send`, `wall` or `sendmail`).

mod backend;
mod cli;
mod config;
mod context;
mod defaults;
mod logging;
mod message;
mod notify;
mod settings;
mod source;
mod time;

use clap::Parser;
use std::env;
use std::error::Error;
use std::path;
use std::process;
use std::thread;
use std::time as std_time;

/// Prints a banner to the terminal with some information about the program.
fn print_banner() {
    println!(
        "{} v{} | copyright (c) 2026 {}\n$ git clone {}\n",
        defaults::program_metadata::NAME,
        defaults::program_metadata::VERSION,
        defaults::program_metadata::AUTHORS,
        defaults::program_metadata::SOURCE_REPOSITORY
    );
}

/// Program entry point.
fn main() -> process::ExitCode {
    let cli = cli::Cli::parse();

    print_banner();

    if cli.version {
        let license = defaults::program_metadata::LICENSE.replace('-', " ");
        println!("This project is licensed under {license}.");
        return process::ExitCode::SUCCESS;
    }

    let config_file = match resolve_config_file(&cli) {
        Outcome::Success(path) => path,
        Outcome::EarlyExitCode(code) => return code,
    };

    let config = match load_config_file(&config_file) {
        Ok(config) => config,
        Err(err) => {
            logging::tseprintln!(
                cli.disable_timestamps,
                "Failed to load configuration file {}: {err}.",
                config_file.display()
            );

            let mut src = err.source();

            while let Some(e) = src {
                eprintln!("\n  caused by: {e}");
                src = e.source();
            }

            return process::ExitCode::from(defaults::exit_codes::FAILED_TO_LOAD_CONFIG_FILE);
        }
    };

    let mut settings = settings::Settings::default();
    settings.apply_config(config.as_ref());
    settings.apply_cli(&cli);

    match settings.sanity_check() {
        Ok(()) => (),
        Err(errors) => {
            for error in errors {
                logging::tseprintln!(settings.disable_timestamps, "Error: {error}");
            }

            if cli.save {
                // Allow errors if we're passing --save
            } else if settings.dry_run {
                logging::tseprintln!(
                    settings.disable_timestamps,
                    "Continuing anyway because --dry-run is enabled."
                );
            } else {
                return process::ExitCode::from(defaults::exit_codes::CONFIG_SANITY_CHECK_FAILED);
            }
        }
    }

    for warning in settings.warnings_check() {
        logging::tseprintln!(settings.disable_timestamps, "Warning: {warning}");
    }

    if cli.save {
        return match settings.save(&config_file) {
            Ok(()) => {
                logging::tsprintln!(
                    settings.disable_timestamps,
                    "Configuration saved successfully to {}.",
                    config_file.display()
                );
                process::ExitCode::SUCCESS
            }
            Err(err) => {
                logging::tseprintln!(
                    settings.disable_timestamps,
                    "Failed to save configuration to {}: {err}.",
                    config_file.display()
                );
                process::ExitCode::from(defaults::exit_codes::FAILED_TO_SAVE_CONFIG_FILE)
            }
        };
    }

    let source = match init_source(&settings) {
        Outcome::Success(source) => source,
        Outcome::EarlyExitCode(code) => return code,
    };

    let notifiers = build_notifiers(&settings);

    if notifiers.is_empty() {
        logging::tseprintln!(settings.disable_timestamps, "No notifiers configured!");

        if !config_file.exists() {
            logging::tseprintln!(
                settings.disable_timestamps,
                "Pass --save to create a new configuration file.",
            );
        }

        return process::ExitCode::from(defaults::exit_codes::NO_NOTIFIERS_CONFIGURED);
    }

    logging::tsprintln!(settings.disable_timestamps, "Initialization complete.");
    display_settings(&settings);

    logging::tsprintln!(settings.disable_timestamps, "Entering monitor loop...");
    run_loop(notifiers, source, &settings)
}

/// Attempts to resolve the path to the configuration file.
///
/// A file specified at the command line takes priority. If none was supplied,
/// attempts to resolve a configuration directory is done, within which a
/// `pellxd.toml` file is expected.
///
/// The configuration directory is derived in `resolve_config_directory`.
///
/// # Parameters
/// - `cli`: The parsed command line arguments.
///
/// # Returns
/// - `Outcome::Success(path::PathBuf)` if the configuration file was
///   successfully resolved.
/// - `Outcome::EarlyExitCode(process::ExitCode)` if resolution failed and
///   the program should exit with the specified shell exit code.
fn resolve_config_file(cli: &cli::Cli) -> Outcome<path::PathBuf> {
    match cli.config.as_deref() {
        Some(path) if path.exists() || cli.save => Outcome::Success(path.to_path_buf()),
        Some(path) => {
            logging::tseprintln!(
                cli.disable_timestamps,
                "Specified configuration file {} does not exist.",
                path.display()
            );
            Outcome::EarlyExitCode(process::ExitCode::from(
                defaults::exit_codes::CONFIG_FILE_DOES_NOT_EXIST,
            ))
        }
        None => match &resolve_config_directory() {
            Ok(path) if !path.exists() => {
                logging::tseprintln!(
                    cli.disable_timestamps,
                    "Directory {} does not exist.",
                    path.display()
                );
                Outcome::EarlyExitCode(process::ExitCode::from(
                    defaults::exit_codes::CONFIG_DIRECTORY_NOT_FOUND,
                ))
            }
            Ok(path) if !path.is_dir() => {
                logging::tseprintln!(
                    cli.disable_timestamps,
                    "{} is not a directory.",
                    path.display()
                );
                Outcome::EarlyExitCode(process::ExitCode::from(
                    defaults::exit_codes::CONFIG_DIRECTORY_NOT_A_DIRECTORY,
                ))
            }
            Ok(path) => {
                let mut dir = path.clone();
                dir.push(defaults::program_metadata::CONFIG_FILENAME);
                Outcome::Success(dir)
            }
            Err(err) => {
                logging::tseprintln!(
                    cli.disable_timestamps,
                    "Failed to resolve configuration directory: {err}."
                );
                Outcome::EarlyExitCode(process::ExitCode::from(
                    defaults::exit_codes::FAILED_TO_RESOLVE_CONFIG_DIRECTORY,
                ))
            }
        },
    }
}

/// Attempts to resolve the path to a configuration directory.
///
/// The environment variable `PELLXD_CONFIG_ROOT` takes priority. If this is
/// not set and the program is running as root, `/etc/pellxd` is used.
/// Failing that, the environment variable `XDG_CONFIG_HOME` is used, if set.
/// If it is not, it finally falls back to `$HOME/.config`.
///
/// # Errors
/// Errors if no candidate location is set in the environment (e.g. running
/// as a non-root user with no `$HOME`, `$XDG_CONFIG_HOME`, or
/// `$PELLXD_CONFIG_ROOT` set).
fn resolve_config_directory() -> Result<path::PathBuf, String> {
    if let Some(path) = env::var_os("PELLXD_CONFIG_ROOT").map(path::PathBuf::from) {
        return Ok(path);
    }

    if users::get_current_uid() == 0 {
        return Ok(path::PathBuf::from("/etc").join(defaults::program_metadata::NAME));
    }

    if let Some(path) = env::var_os("XDG_CONFIG_HOME").map(path::PathBuf::from) {
        return Ok(path);
    }

    if let Some(path) = env::var_os("HOME").map(path::PathBuf::from) {
        return Ok(path.join(".config"));
    }

    Err("could not determine directory based on UID nor from environment variables".to_string())
}

/// Attempts to load the configuration file at the specified path.
///
/// # Errors
/// Errors if the configuration file exists but could not be loaded.
fn load_config_file(config_path: &path::Path) -> Result<Option<config::Config>, confy::ConfyError> {
    if !config_path.exists() {
        return Ok(None);
    }

    match confy::load_path(config_path) {
        Ok(config) => Ok(Some(config)),
        Err(err) => Err(err),
    }
}

/// Initializes the input source specified in the passed settings.
///
/// # Parameters
/// - `settings`: The program settings from which to derive which input source
///   to initialize (and with what arguments).
///
/// # Returns
/// - `Outcome::Success(Box<dyn source::InputSource>)` if the input source
///   was successfully initialized.
/// - `Outcome::EarlyExitCode(process::ExitCode)` if initialization failed and
///   the program should exit with the specified shell exit code.
fn init_source(settings: &settings::Settings) -> Outcome<Box<dyn source::InputSource>> {
    let mut source: Box<dyn source::InputSource> = match settings.monitor.source {
        source::ChoiceOfInputSource::Gpio => {
            Box::new(source::GpioInputSource::new(settings.gpio.pin))
        }
        source::ChoiceOfInputSource::Dummy => Box::new(source::DummyInputSource::new(
            settings.dummy_source.modulus,
            settings.dummy_source.threshold,
        )),
    };

    logging::tsprintln!(settings.disable_timestamps, "Input: {}", source.name());

    if let Err(err) = source.init() {
        logging::tseprintln!(
            settings.disable_timestamps,
            "Failed to initialize source: {err}",
        );
        return Outcome::EarlyExitCode(process::ExitCode::from(
            defaults::exit_codes::FAILED_TO_INITIALIZE_INPUT_SOURCE,
        ));
    }

    match source.sanity_check() {
        Ok(()) => Outcome::Success(source),
        Err(errors) => {
            for error in errors {
                logging::tseprintln!(settings.disable_timestamps, "Error: {error}");
            }

            Outcome::EarlyExitCode(process::ExitCode::from(
                defaults::exit_codes::INPUT_SOURCE_SANITY_CHECK_FAILED,
            ))
        }
    }
}

/// Constructs notifiers based on the passed settings.
///
/// # Parameters
/// - `settings`: The program settings from which to derive which notifiers to
///   construct (and with what arguments).
///
/// # Returns
/// A vector of boxed `dyn notify::StatefulNotifier`s, to be used as notifiers
/// for pushing notifications in the main loop.
fn build_notifiers(settings: &settings::Settings) -> Vec<Box<dyn notify::StatefulNotifier>> {
    let mut notifiers: Vec<Box<dyn notify::StatefulNotifier>> = Vec::new();
    let agent = ureq::Agent::new_with_defaults();

    let (slack_enabled, batsign_enabled, command_enabled, println_enabled) = match (
        settings.slack.enabled,
        settings.batsign.enabled,
        settings.command.enabled,
        settings.println.enabled,
    ) {
        (false, false, false, false) if settings.dry_run => {
            logging::tseprintln!(
                settings.disable_timestamps,
                "Enabling all notifier backends as no backends are configured but --dry-run is enabled."
            );
            (true, true, true, true)
        }
        other => other,
    };

    if slack_enabled {
        for (i, url) in settings.slack.urls.iter().enumerate() {
            let backend = backend::SlackBackend::new(
                i,
                agent.clone(),
                url,
                settings.slack.show_response,
                settings.slack.strings.clone(),
            );

            let n = notify::Notifier::new(backend, settings.dry_run);
            notifiers.push(Box::new(n));
        }
    }

    if batsign_enabled {
        for (i, url) in settings.batsign.urls.iter().enumerate() {
            let backend = backend::BatsignBackend::new(
                i,
                agent.clone(),
                url,
                settings.batsign.show_response,
                settings.batsign.strings.clone(),
            );

            let n = notify::Notifier::new(backend, settings.dry_run);
            notifiers.push(Box::new(n));
        }
    }

    if command_enabled {
        for (i, command) in settings.command.commands.iter().enumerate() {
            let backend = backend::CommandBackend::new(
                i,
                command,
                settings.command.show_response,
                settings.command.strings.clone(),
            );
            let n = notify::Notifier::new(backend, settings.dry_run);
            notifiers.push(Box::new(n));
        }
    }

    if println_enabled {
        let backend = backend::PrintlnBackend::new(0, settings.println.strings.clone());
        let n = notify::Notifier::new(backend, settings.dry_run);

        notifiers.push(Box::new(n));
    }

    notifiers
}

/// Prints settings related to the monitor loop to the terminal.
fn display_settings(settings: &settings::Settings) {
    println!();
    println!("{:#?}", settings.monitor);
    println!();

    match settings.monitor.source {
        source::ChoiceOfInputSource::Gpio => {
            println!("{:#?}", settings.gpio);
        }
        source::ChoiceOfInputSource::Dummy => {
            println!("{:#?}", settings.dummy_source);
        }
    }

    println!();
}

/// Main loop of the program.
///
/// Continuously polls the input source for changes and sends notifications
/// through the provided notifiers on changes. Does not return.
///
/// # Parameters
/// - `notifiers`: A vector of notifiers to send notifications through.
/// - `source`: The input source to poll for values.
/// - `settings`: The program settings.
fn run_loop(
    mut notifiers: Vec<Box<dyn notify::StatefulNotifier>>,
    mut source: Box<dyn source::InputSource>,
    settings: &settings::Settings,
) -> process::ExitCode {
    let mut ctx = context::Context::new();

    loop {
        ctx.now = time::Timestamp::now();

        let at_least_one_notifier_is_due_for_retry = notifiers
            .iter()
            .any(|n| n.state().has_due_retry(ctx.now.instant));

        if at_least_one_notifier_is_due_for_retry {
            notify::send_retries(&mut notifiers, settings, &ctx.now.instant);
            ctx.now = time::Timestamp::now(); // new snapshot after costly send_retries
        }

        let reading = source.read();
        let reading_changed = reading != ctx.previous_reading;

        if settings.debug {
            println!(
                "{:>2}: {reading:?}/{:?} => {reading_changed}",
                ctx.loop_iteration, ctx.previous_reading
            );
        }

        if reading_changed {
            // Reset
            match reading {
                source::Reading::Low => {
                    ctx.went_low_at = Some(ctx.now);
                    ctx.time_of_startup = None;
                    ctx.startup_succeeded = false;
                }
                source::Reading::High => {
                    ctx.went_high_at = Some(ctx.now);
                }
            }

            // Update
            ctx.previous_reading = reading;
            ctx.time_of_state_change = Some(ctx.now);
        }

        match reading {
            source::Reading::Low => handle_low_reading(&mut notifiers, &mut ctx, settings),
            source::Reading::High => {
                handle_high_reading(&mut notifiers, &mut ctx, settings, reading_changed);
            }
        }

        end_loop(&mut ctx, *settings.monitor.loop_interval);
    }
}

/// Helper function to handle a `LOW` reading from the input source.
///
/// This is called as part of the main loop, when `source::Reading::Low` is read.
///
/// # Parameters
/// - `notifiers`: The notifiers to send potential notifications through.
/// - `ctx`: The context of the main loop, containing state and timestamps.
/// - `settings`: The program settings.
fn handle_low_reading(
    notifiers: &mut Vec<Box<dyn notify::StatefulNotifier>>,
    ctx: &mut context::Context,
    settings: &settings::Settings,
) {
    if ctx.time_of_state_change.is_none() || ctx.startup_succeeded {
        // Early return.
        // Either program was just started and state has never changed from low
        // or startup succeeded and all is well
        return;
    }

    // We are low, but we don't know if we have completely started up yet
    let Some(time_of_startup) = ctx.time_of_startup else {
        // First loop after going low, can't have started up yet
        // (provided startup_duration > 0)
        if settings.verbose {
            println!();
            logging::tsprintln!(settings.disable_timestamps, "-- NEW LOW --");
            println!();
        }

        ctx.time_of_startup = Some(ctx.now);
        return;
    };

    if time_of_startup.instant.elapsed() >= *settings.monitor.startup_window {
        // Startup succeeded, can notify success
        if settings.verbose {
            println!();
        }

        let result = notify::send_to_all(
            notifiers,
            settings,
            ctx,
            notify::MessageType::StartupSuccess,
        );

        if result.success != result.total {
            logging::tseprintln!(
                settings.disable_timestamps,
                "Failed to send some startup success notifications: {}/{} succeeded",
                result.success,
                result.total
            );
        }

        if settings.verbose {
            println!();
        }

        ctx.startup_succeeded = true;
    }
}

/// Helper function to handle a `HIGH` reading from the input source.
///
/// This is called as part of the main loop, when `source::Reading::High` is read.
///
/// # Parameters
/// - `notifiers`: The notifiers to send potential notifications through.
/// - `ctx`: The context of the main loop, containing state and timestamps.
/// - `settings`: The program settings.
/// - `reading_changed`: A boolean indicating if the value read from the
///   input source changed since the last loop iteration.
fn handle_high_reading(
    notifiers: &mut Vec<Box<dyn notify::StatefulNotifier>>,
    ctx: &mut context::Context,
    settings: &settings::Settings,
    reading_changed: bool,
) {
    let is_first_iteration = ctx.loop_iteration == 0;
    if reading_changed || is_first_iteration {
        if settings.verbose {
            println!();
            logging::tsprintln!(settings.disable_timestamps, "-- NEW HIGH --");
        }

        if let Some(t) = ctx.time_of_startup
            && t.instant.elapsed() < *settings.monitor.startup_window
        {
            // We went high again before startup duration elapsed, this is a startup failure
            let result =
                notify::send_to_all(notifiers, settings, ctx, notify::MessageType::StartupFailed);

            if result.success != result.total {
                logging::tseprintln!(
                    settings.disable_timestamps,
                    "Failed to send some startup failure notifications: {}/{} succeeded",
                    result.success,
                    result.total
                );
            }
        } else {
            // We just randomly went HIGH for no reason, this is an alert
            let result = notify::send_to_all(notifiers, settings, ctx, notify::MessageType::Alert);

            if result.success != result.total {
                logging::tseprintln!(
                    settings.disable_timestamps,
                    "Failed to send some alert notifications: {}/{} succeeded",
                    result.success,
                    result.total
                );
            }
        }

        if settings.verbose {
            println!();
        }
    } else {
        // We have been HIGH for a while, it may be time for a reminder
        let at_least_one_notifier_due_for_reminder = notifiers
            .iter()
            .any(|n| n.state().has_due_reminder(ctx.now.instant));

        if at_least_one_notifier_due_for_reminder {
            if settings.verbose {
                println!();
            }

            let result =
                notify::send_to_all(notifiers, settings, ctx, notify::MessageType::Reminder);

            if result.success != result.total {
                logging::tseprintln!(
                    settings.disable_timestamps,
                    "Failed to send some reminder notifications: {}/{} succeeded",
                    result.success,
                    result.total
                );
            }

            if settings.verbose {
                println!();
            }
        }
    }
}

/// Helper function to end a loop iteration.
///
/// This is called at the end of each iteration of the main loop.
///
/// # Parameters
/// - `ctx`: The context of the main loop, containing state and timestamps.
/// - `interval`: The duration to sleep for as part of the end of the loop.
fn end_loop(ctx: &mut context::Context, interval: std_time::Duration) {
    ctx.loop_iteration += 1;
    thread::sleep(interval);
}

/// Helper enum to represent the outcome of an operation *or* an early exit
/// shell code.
///
/// This is used in functions that would normally return `Result<T, process::ExitCode>`
/// but where semantically the error case is not necessarily an error.
enum Outcome<T> {
    /// The operation succeeded with the embedded value of some type `T`.
    Success(T),

    /// The operation resulted in a condition where the calling function should
    /// exit early with the embedded shell exit code.
    EarlyExitCode(process::ExitCode),
}
