use std::path::PathBuf;

use clap::Args;

#[derive(Args)]
pub struct EesArgs {
    /// Root directory for native actor component state.
    #[arg(long, value_name = "PATH")]
    store: Option<PathBuf>,
    /// Request home used by the configured IMOS actor.
    #[arg(long, value_name = "PATH")]
    home: PathBuf,
    /// Logical name of the configured IMOS actor.
    #[arg(long, default_value = "imos", value_name = "NAME")]
    name: String,
    /// Write progress and recoverable protocol diagnostics to stderr.
    #[arg(short = 'e', long)]
    events_to_stderr: bool,
}

pub fn run(arguments: &EesArgs, explicit_context: bool) -> Result<i32, String> {
    if explicit_context {
        return Err("-C cannot be used with telora ees".into());
    }
    let store = arguments
        .store
        .clone()
        .map(Ok)
        .unwrap_or_else(telora_ees::configured_store_path)
        .map_err(|error| error.to_string())?;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the EES runtime: {error}"))?
        .block_on(async {
            let service = telora_ees::Service::open(telora_ees::imos_manifest(
                &arguments.name,
                store,
                &arguments.home,
            ))
            .await
            .map_err(|error| format!("cannot initialize EES: {error:#}"))?;
            let outcome = telora_ees::stdio::serve(service, arguments.events_to_stderr)
                .await
                .map_err(|error| format!("EES failed: {error:#}"))?;
            Ok(
                if outcome == telora_ees::stdio::ServeOutcome::ProtocolError {
                    1
                } else {
                    0
                },
            )
        })
}
