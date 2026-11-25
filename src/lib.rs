mod app;
pub mod cli;
pub mod command;
pub mod ingest;
pub mod ops;
pub mod output;
mod projection;
#[cfg(feature = "saved-views")]
pub mod saved_views;
pub mod table;
pub mod theme;
pub mod ui;
pub mod view;

pub(crate) use app::FilterPromptView;

/// Run the table viewer with the supplied command-line arguments.
pub fn run(args: cli::Args) -> anyhow::Result<()> {
    use std::io::IsTerminal;

    let config = cli::Config::from_args(args)?;
    let execution = output::resolve_execution_mode(
        config.interactive,
        config.output,
        std::io::stdout().is_terminal(),
    );
    if config.sorted.is_some() || config.top_lines.is_some() {
        anyhow::ensure!(
            execution == output::ExecutionMode::Batch(output::OutputFormat::Table),
            "--sorted and --top-lines require direct table output"
        );
    }
    let theme = theme::load_active_theme(None)?;
    match execution {
        output::ExecutionMode::Batch(format) => app::run_batch(&config, format, theme),
        output::ExecutionMode::Interactive { emit_on_exit } => {
            app::run_terminal(&config, emit_on_exit, theme)
        }
    }
}
