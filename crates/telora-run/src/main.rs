mod cli;
use clap::Parser;

fn main() {
    let code = match cli::execute(cli::Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            if let Some(initialization) = error.downcast_ref::<telora_run::InitializationError>() {
                for diagnostic in &initialization.diagnostics {
                    let _ = cli::diagnostic(diagnostic);
                }
                if initialization.diagnostics.is_empty() {
                    let _ = cli::diagnostic(
                        &serde_json::json!({"schema":"telora.error/v1","record":"error","message":error.to_string()}),
                    );
                }
            } else {
                let _ = cli::diagnostic(
                    &serde_json::json!({"schema":"telora.error/v1","record":"error","message":error.to_string()}),
                );
            }
            1
        }
    };
    if code != 0 {
        std::process::exit(code);
    }
}
