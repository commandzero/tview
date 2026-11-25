#[cfg(feature = "saved-views")]
use super::saved_view::{apply_selected_saved_view, format_saved_view_warning};
use super::{warning_footer, App};
#[cfg(feature = "saved-views")]
use crate::saved_views;
use crate::{cli, command, ingest, theme, view};
#[cfg(feature = "saved-views")]
use std::path::{Path, PathBuf};

type RelationSelector<'a> =
    dyn FnMut(&[ingest::RelationCatalogEntry]) -> anyhow::Result<Option<String>> + 'a;

pub(super) fn prepare_app(
    config: &cli::Config,
    theme_load: theme::ThemeLoad,
    source: ingest::source::InputSource,
    select_relation: Option<&mut RelationSelector<'_>>,
) -> anyhow::Result<Option<App>> {
    #[cfg(feature = "saved-views")]
    let invocation = prepare_saved_view_invocation(config, None)?;
    AppStartup::new(
        config,
        theme_load,
        #[cfg(feature = "saved-views")]
        &invocation,
    )
    .open(source, select_relation, ingest::open_source)
}

pub(super) fn configured_open_options(
    config: &cli::Config,
    #[cfg(feature = "saved-views")] invocation: &saved_views::SavedViewInvocation,
) -> anyhow::Result<ingest::OpenOptions> {
    let parse_options = ingest::ParseOptions {
        encoding: config.encoding.clone(),
        delimiter: config.delimiter,
        quoting: config.quoting,
        quote_char: config.quote_char,
    };
    #[cfg(feature = "saved-views")]
    let saved_source_options = match invocation {
        saved_views::SavedViewInvocation::Enabled {
            selected: Some(selected),
            ..
        } => selected.view.source_options(),
        _ => ingest::SourceOptionOverrides::default(),
    };
    #[cfg(not(feature = "saved-views"))]
    let saved_source_options = ingest::SourceOptionOverrides::default();
    let mut open_options = ingest::OpenOptions::merge(
        ingest::OpenOptions::default(),
        &saved_source_options,
        &config.source_options,
    );
    open_options.preview = config.top_lines.is_some();
    open_options.delimited = parse_options;
    open_options.validate()?;
    #[cfg(feature = "elasticsearch")]
    if config.target.elastic_context().is_some() {
        ingest::validate_elasticsearch_parsing_options(&open_options)?;
        if let Some(table) = &open_options.table {
            ingest::validate_from_target(table)?;
        }
    }
    Ok(open_options)
}

pub(super) fn open_selected_source(
    source: ingest::source::InputSource,
    mut options: ingest::OpenOptions,
    select_relation: Option<&mut RelationSelector<'_>>,
    open_source: impl FnOnce(
        ingest::source::InputSource,
        &ingest::OpenOptions,
    ) -> anyhow::Result<ingest::OpenedSource>,
) -> anyhow::Result<Option<(ingest::OpenedTable, ingest::OpenOptions)>> {
    let mut opened_source = open_source(source, &options)?;
    if !opened_source.has_selected_table() {
        let has_relations = opened_source.selectable_relations().next().is_some();
        #[cfg(feature = "elasticsearch")]
        validate_relation_choice(options.format, select_relation.is_some(), has_relations)?;
        if let Some(selector) = select_relation.filter(|_| has_relations) {
            let Some(selected) = selector(opened_source.list_relations())? else {
                return Ok(None);
            };
            opened_source.open_relation(&selected)?;
            options.table = Some(selected);
        }
    }
    Ok(Some((opened_source.into_implicit_table()?, options)))
}

#[cfg(feature = "elasticsearch")]
fn validate_relation_choice(
    format: ingest::InputFormat,
    has_selector: bool,
    has_relations: bool,
) -> anyhow::Result<()> {
    if format == ingest::InputFormat::Elasticsearch {
        anyhow::ensure!(
            has_selector,
            "direct Elasticsearch output requires --table or --query"
        );
        anyhow::ensure!(
            has_relations,
            "no visible open Elasticsearch indices or data streams are available"
        );
    }
    Ok(())
}

pub(super) struct AppStartup<'a> {
    config: &'a cli::Config,
    theme_load: theme::ThemeLoad,
    #[cfg(feature = "saved-views")]
    invocation: &'a saved_views::SavedViewInvocation,
}

impl<'a> AppStartup<'a> {
    pub(super) fn new(
        config: &'a cli::Config,
        theme_load: theme::ThemeLoad,
        #[cfg(feature = "saved-views")] invocation: &'a saved_views::SavedViewInvocation,
    ) -> Self {
        Self {
            config,
            theme_load,
            #[cfg(feature = "saved-views")]
            invocation,
        }
    }

    pub(super) fn open(
        self,
        source: ingest::source::InputSource,
        select_relation: Option<&mut RelationSelector<'_>>,
        open_source: impl FnOnce(
            ingest::source::InputSource,
            &ingest::OpenOptions,
        ) -> anyhow::Result<ingest::OpenedSource>,
    ) -> anyhow::Result<Option<App>> {
        let options = configured_open_options(
            self.config,
            #[cfg(feature = "saved-views")]
            self.invocation,
        )?;
        let Some((opened, options)) =
            open_selected_source(source.clone(), options, select_relation, open_source)?
        else {
            return Ok(None);
        };
        self.initialize(source, opened, options).map(Some)
    }

    fn initialize(
        self,
        source: ingest::source::InputSource,
        opened: ingest::OpenedTable,
        open_options: ingest::OpenOptions,
    ) -> anyhow::Result<App> {
        let viewport = view::Viewport::new(20, 8);
        let mut view = view::TableView::from_opened_table(opened, viewport)?;
        view = view.with_column_width_mode(self.config.width);
        #[cfg(feature = "saved-views")]
        let saved_view = apply_selected_saved_view(self.config, self.invocation, &mut view);
        let diagnostics = self.diagnostics(
            #[cfg(feature = "saved-views")]
            &mut view,
        );
        let message = warning_footer(&diagnostics);
        self.goto_start(&mut view);
        view.initialize_source_configuration(open_options)?;

        Ok(App {
            source,
            view,
            popup: None,
            filter_prompt: None,
            column_info: None,
            source_modal: None,
            search_query: String::new(),
            keys: command::KeyInterpreter::default(),
            message,
            diagnostics,
            theme: self.theme_load.theme,
            #[cfg(feature = "saved-views")]
            saved_view,
            #[cfg(feature = "saved-views")]
            view_modal: None,
        })
    }

    fn diagnostics(
        &self,
        #[cfg(feature = "saved-views")] view: &mut view::TableView,
    ) -> Vec<String> {
        #[cfg(feature = "saved-views")]
        let mut diagnostics = match self.invocation {
            saved_views::SavedViewInvocation::Enabled { warnings, .. } => warnings
                .iter()
                .map(format_saved_view_warning)
                .collect::<Vec<_>>(),
            saved_views::SavedViewInvocation::Disabled => Vec::new(),
        };
        #[cfg(feature = "saved-views")]
        diagnostics.extend(
            view.take_saved_binding_warnings()
                .iter()
                .map(format_saved_view_warning),
        );
        #[cfg(not(feature = "saved-views"))]
        let mut diagnostics = Vec::new();
        diagnostics.extend(
            self.theme_load
                .warnings
                .iter()
                .map(|warning| format!("theme warning: {}: {}", warning.field, warning.message)),
        );
        diagnostics
    }

    fn goto_start(&self, view: &mut view::TableView) {
        if self.config.top_lines.is_some() {
            return;
        }
        view.goto_user_row(self.config.start_position.row.max(1));
        if let Some(column) = self.config.start_position.column {
            view.goto_user_column(column.max(1));
        }
    }
}

#[cfg(feature = "saved-views")]
pub(super) fn prepare_saved_view_invocation(
    config: &cli::Config,
    config_root: Option<&Path>,
) -> anyhow::Result<saved_views::SavedViewInvocation> {
    use crate::cli::SavedViewSelection as CliSavedViewSelection;
    use crate::saved_views::SavedViewSelection;

    let target_identity = PathBuf::from(config.target.saved_view_filename());
    let context_identity = config
        .target
        .elastic_context()
        .map(|context| context.safe_identity());
    let selection = match &config.saved_view {
        CliSavedViewSelection::Disabled => None,
        CliSavedViewSelection::Auto => Some(match context_identity.as_deref() {
            Some(identity) => SavedViewSelection::AutoSource { identity },
            None => SavedViewSelection::Auto {
                input_path: &target_identity,
            },
        }),
        CliSavedViewSelection::Force(name) => Some(SavedViewSelection::Force { name }),
    };
    saved_views::prepare_saved_view(selection, &target_identity, config_root)
        .map_err(anyhow::Error::msg)
}
