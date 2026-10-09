use std::io;
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

use tracing::{info, warn};

use crate::server::main_loop::HeadlessServer;
use crate::utils::socket_paths::client_socket_path;
use crate::{server::app, utils::config};

/// Run the headless server. This is the entry point called from main.rs.
pub fn run_server(
    logging_options: crate::utils::logging::LoggingOptions,
    config_override: Option<fn(&mut config::Config)>,
) -> io::Result<()> {
    init_logging(&logging_options);
    crate::platform::raise_server_nofile_limit();

    let mut loaded_config = config::Config::load();
    if let Some(apply) = config_override {
        apply(&mut loaded_config.config);
    }
    let (api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let event_hub = crate::server::api::EventHub::default();
    let should_quit = Arc::new(AtomicBool::new(false));

    // Start the JSON API socket server.
    let _api_server = match crate::server::api::start_server_with_stop_control(
        api_tx.clone(),
        event_hub.clone(),
        should_quit.clone(),
    ) {
        Ok(server) => server,
        Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
            eprintln!("error: herdr server is already running");
            eprintln!(
                "api socket: {}",
                crate::protocol::api::socket_path().display()
            );
            std::process::exit(1);
        }
        Err(err) => return Err(err),
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io::Error::other)?;

    let result = rt.block_on(async {
        // Create the App (with AppState, event channels, etc.).
        let mut app = app::App::new(
            &loaded_config.config,
            app::AppPolicy::PRODUCTION,
            config::config_diagnostic_summary(&loaded_config.diagnostics),
            api_rx,
            event_hub,
        );
        seed_startup_workspace_if_empty(&mut app);

        // Create the headless server.
        let mut server = match HeadlessServer::new(
            app,
            &loaded_config.diagnostics,
            Some(api_tx.clone()),
            Some(_api_server),
            should_quit,
        ) {
            Ok(server) => server,
            Err(err) if err.kind() == io::ErrorKind::AddrInUse => {
                eprintln!("error: herdr server is already running");
                eprintln!("client socket: {}", client_socket_path().display());
                std::process::exit(1);
            }
            Err(err) => return Err(err),
        };

        info!(
            api_socket = %crate::protocol::api::socket_path().display(),
            client_socket = %client_socket_path().display(),
            "herdr server started"
        );
        print_ready_message(&crate::protocol::api::socket_path(), &client_socket_path());

        crate::utils::logging::startup("server", logging_options.dev);
        server.run().await
    });

    rt.shutdown_timeout(Duration::from_millis(100));
    crate::utils::logging::shutdown("server");
    result
}

fn seed_startup_workspace_if_empty(app: &mut app::App) {
    let Some(cwd) = take_startup_cwd() else {
        return;
    };

    if !app.state.workspaces.is_empty() {
        info!(
            cwd = %cwd.display(),
            "restored session already has workspaces; ignoring startup cwd"
        );
        return;
    }

    match app.create_workspace_with_options(cwd.clone(), true) {
        Ok(_) => {
            info!(cwd = %cwd.display(), "created startup workspace");
        }
        Err(err) => {
            warn!(cwd = %cwd.display(), err = %err, "failed to create startup workspace");
            app.state.mode = app::Mode::Navigate;
        }
    }
}

fn take_startup_cwd() -> Option<PathBuf> {
    let cwd = std::env::var_os(crate::utils::env::STARTUP_CWD_ENV_VAR)?;
    std::env::remove_var(crate::utils::env::STARTUP_CWD_ENV_VAR);
    (!cwd.is_empty()).then(|| PathBuf::from(cwd))
}

fn print_ready_message(api_socket: &Path, client_socket: &Path) {
    eprintln!("herdr server running; you can use any herdr CLI command in another terminal.");
    eprintln!("api socket: {}", api_socket.display());
    eprintln!("client socket: {}", client_socket.display());
    eprintln!(
        "logs: {}",
        crate::utils::paths::data_dir()
            .join("herdr-server.log")
            .display()
    );
    eprintln!("did you mean to open the Herdr TUI? run `herdr`; you do not need `herdr server`.");
}

/// Initialize logging for the server process.
fn init_logging(options: &crate::utils::logging::LoggingOptions) {
    crate::utils::logging::init_file_logging_at(
        crate::utils::paths::data_dir(),
        "herdr-server.log",
        options,
    );
}

#[cfg(test)]
#[path = "tests/startup_env_test.rs"]
mod env_tests;
