use super::interactive::select_table_modal;
#[cfg(feature = "saved-views")]
use super::startup::prepare_saved_view_invocation;
use super::startup::{configured_open_options, AppStartup};
use super::App;
use crate::{cli, ingest, theme, ui};
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use std::time::Duration;

pub(super) fn prepare_context_app(
    config: &cli::Config,
    theme_load: theme::ThemeLoad,
    source: ingest::source::InputSource,
    terminal: &mut ui::terminal::TerminalSession,
) -> anyhow::Result<Option<App>> {
    #[cfg(feature = "saved-views")]
    let invocation = prepare_saved_view_invocation(config, None)?;
    configured_open_options(
        config,
        #[cfg(feature = "saved-views")]
        &invocation,
    )?;
    if !resolve_context(&source, terminal, &theme_load.theme)? {
        return Ok(None);
    }
    let theme = theme_load.theme.clone();
    let mut select_relation = |relations: &[ingest::RelationCatalogEntry]| {
        select_table_modal(terminal, &theme, relations)
    };
    AppStartup::new(
        config,
        theme_load,
        #[cfg(feature = "saved-views")]
        &invocation,
    )
    .open(source, Some(&mut select_relation), ingest::open_source)
}

fn resolve_context(
    source: &ingest::source::InputSource,
    terminal: &mut ui::terminal::TerminalSession,
    theme: &theme::ResolvedTheme,
) -> anyhow::Result<bool> {
    let receiver = spawn_context_resolver(source);
    let status = format!("Loading {} (q/Esc to cancel)", source.display_name());
    draw_context_status(terminal, theme, &status)?;
    loop {
        match receiver.try_recv() {
            Ok(result) => return result.map(|()| true),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                anyhow::bail!("Elasticsearch context preparation worker stopped");
            }
            Err(std::sync::mpsc::TryRecvError::Empty)
                if context_open_cancelled(terminal, theme, &status)? =>
            {
                // Do not join a blocked resolver when cancelling startup.
                return Ok(false);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
}

fn context_open_cancelled(
    terminal: &mut ui::terminal::TerminalSession,
    theme: &theme::ResolvedTheme,
    status: &str,
) -> anyhow::Result<bool> {
    if !poll(Duration::from_millis(50))? {
        return Ok(false);
    }
    match read()? {
        Event::Key(event) => return Ok(cancels_context_open(event)),
        Event::Resize(_, _) => draw_context_status(terminal, theme, status)?,
        _ => {}
    }
    Ok(false)
}

fn draw_context_status(
    terminal: &mut ui::terminal::TerminalSession,
    theme: &theme::ResolvedTheme,
    status: &str,
) -> anyhow::Result<()> {
    terminal.terminal_mut().draw(|frame| {
        ui::render_footer_with_theme(Some(status), frame.area(), frame.buffer_mut(), theme);
    })?;
    Ok(())
}

fn spawn_context_resolver(
    source: &ingest::source::InputSource,
) -> std::sync::mpsc::Receiver<anyhow::Result<()>> {
    let target = source
        .elastic_context()
        .expect("context startup requires a context target")
        .clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    // Resolution may block on a command or credential store. The target clone
    // shares its connection cache with App::source and all later reloads.
    std::thread::spawn(move || {
        let result = ingest::elastic_context::resolve_connection(&target).map(|_| ());
        let _ = sender.send(result);
    });
    receiver
}

fn cancels_context_open(event: KeyEvent) -> bool {
    match event.code {
        KeyCode::Esc | KeyCode::Char('q' | 'Q') => true,
        KeyCode::Char('c') => event.modifiers.contains(KeyModifiers::CONTROL),
        _ => false,
    }
}
