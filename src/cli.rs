use clap::{ArgAction, Parser};

use crate::ingest::{
    source::SourceTarget, InputFormat, JsonPointer, ObjectMode, Quoting, SchemaScan,
    SourceOptionOverrides,
};
use crate::output::{ColorOutput, OutputFormat};
use crate::view::ColumnWidthMode;

#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "tview", version, disable_help_subcommand = true)]
#[cfg_attr(
    all(feature = "sqlite", not(feature = "elasticsearch")),
    command(about = "View delimited, JSON, NDJSON, or local SQLite data.")
)]
#[cfg_attr(
    all(feature = "sqlite", feature = "elasticsearch"),
    command(about = "View delimited, JSON, NDJSON, SQLite, or Elasticsearch data.")
)]
#[cfg_attr(
    all(feature = "elasticsearch", not(feature = "sqlite")),
    command(about = "View delimited, JSON, NDJSON, or Elasticsearch data.")
)]
#[cfg_attr(
    not(any(feature = "sqlite", feature = "elasticsearch")),
    command(about = "View delimited, JSON, or NDJSON data.")
)]
pub struct Args {
    /// Local file, standard input marker '-', or remote source URL.
    pub filename: String,

    /// Run the interactive viewer. Combine with --output to serialize the final view on quit.
    #[arg(short = 'i', long = "interactive", action = ArgAction::SetTrue)]
    pub interactive: bool,

    /// Output serialization format.
    #[arg(short = 'o', long = "output", value_enum)]
    pub output: Option<OutputFormat>,

    /// Apply saved view sorting for direct table output [default: true].
    #[arg(long, action = ArgAction::Set)]
    pub sorted: Option<bool>,

    /// Preview at most this many data rows, excluding the header and summary.
    #[arg(short = 'n', long = "top-lines")]
    pub top_lines: Option<std::num::NonZeroUsize>,

    /// Color policy for serialized output.
    #[arg(long = "color", value_enum, default_value_t = ColorOutput::Auto)]
    pub color: ColorOutput,

    /// Encoding, if required.
    #[arg(short = 'e', long = "encoding")]
    pub encoding: Option<String>,

    /// CSV delimiter. Not typically necessary since automatic delimiter sniffing is used.
    #[arg(short = 'd', long = "delimiter")]
    pub delimiter: Option<String>,

    /// CSV quoting style, using Python csv.QUOTE_* names.
    #[arg(long = "quoting")]
    pub quoting: Option<String>,

    /// Initial cursor display position as y or y,x.
    #[arg(short = 's', long = "start_pos")]
    pub start_pos: Option<String>,

    /// Column width: 'max', 'mode', or an integer fixed width.
    #[arg(short = 'w', long = "width", default_value = "mode")]
    pub width: String,

    /// Force full handling of double-width characters for large files.
    #[arg(long = "double_width", action = ArgAction::SetTrue)]
    pub double_width: bool,

    /// Quote character.
    #[arg(short = 'q', long = "quote-char", default_value = "\"")]
    pub quote_char: String,

    /// Input format. Automatic selection uses the filename and bounded content probing.
    #[arg(long = "format", value_parser = parse_input_format)]
    pub format: Option<InputFormat>,

    /// RFC 6901 JSON Pointer selecting the table within each JSON document.
    #[arg(long = "json-path", value_parser = parse_json_pointer)]
    pub json_path: Option<JsonPointer>,

    /// Interpret a selected structured object as auto-detected, one record, or keyed entries.
    #[arg(long = "object-mode", value_parser = parse_object_mode)]
    pub object_mode: Option<ObjectMode>,

    /// JSON schema discovery policy.
    #[arg(long = "schema-scan", value_parser = parse_schema_scan)]
    pub schema_scan: Option<SchemaScan>,

    /// Source-native relation or target to open.
    #[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
    #[arg(long = "table")]
    pub table: Option<String>,

    /// Complete native query in the language selected by the source format.
    #[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
    #[arg(long = "query", conflicts_with = "table")]
    pub query: Option<String>,

    /// Force a saved view by canonical name.
    #[cfg(feature = "saved-views")]
    #[arg(long = "view", conflicts_with = "no_view")]
    pub view: Option<String>,

    /// Disable saved view discovery and application for this invocation.
    #[cfg(feature = "saved-views")]
    #[arg(long = "no-view", action = ArgAction::SetTrue)]
    pub no_view: bool,

    /// Extra positional arguments, including classic +y:x start positions.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub extra: Vec<String>,
}

impl Args {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub target: SourceTarget,
    pub interactive: bool,
    pub output: Option<OutputFormat>,
    pub color: ColorOutput,
    pub sorted: Option<bool>,
    pub top_lines: Option<std::num::NonZeroUsize>,
    pub encoding: Option<String>,
    pub delimiter: Option<u8>,
    pub quoting: Option<Quoting>,
    pub start_position: StartPosition,
    pub width: ColumnWidthMode,
    pub double_width: bool,
    pub quote_char: u8,
    pub source_options: SourceOptionOverrides,
    #[cfg(feature = "saved-views")]
    pub saved_view: SavedViewSelection,
}

impl Config {
    pub fn from_args(args: Args) -> Result<Self, CliError> {
        let delimited_option_selected = args.encoding.is_some()
            || args.delimiter.is_some()
            || args.quoting.is_some()
            || args.quote_char != "\"";
        let explicit_format = args.format;
        if explicit_format.is_some_and(format_rejects_delimited_options)
            && delimited_option_selected
        {
            return Err(CliError::IncompatibleOptions {
                format: explicit_format.expect("checked format"),
                option: "delimited parsing options",
            });
        }
        if args.json_path.is_some()
            && (explicit_format == Some(InputFormat::Delimited) || delimited_option_selected)
        {
            return Err(CliError::IncompatibleOptions {
                format: InputFormat::Delimited,
                option: "--json-path",
            });
        }
        if matches!(
            explicit_format,
            Some(InputFormat::Delimited | InputFormat::Ndjson)
        ) && matches!(
            args.object_mode,
            Some(ObjectMode::Record | ObjectMode::Entries)
        ) {
            return Err(CliError::IncompatibleOptions {
                format: explicit_format.expect("checked format"),
                option: "--object-mode",
            });
        }
        #[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
        if args.table.is_some()
            && matches!(
                explicit_format,
                Some(InputFormat::Delimited | InputFormat::Json | InputFormat::Ndjson)
            )
        {
            return Err(CliError::IncompatibleOptions {
                format: explicit_format.expect("checked format"),
                option: "--table",
            });
        }
        #[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
        if args.query.is_some()
            && matches!(
                explicit_format,
                Some(InputFormat::Delimited | InputFormat::Json | InputFormat::Ndjson)
            )
        {
            return Err(CliError::IncompatibleOptions {
                format: explicit_format.expect("checked format"),
                option: "--query",
            });
        }
        #[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
        let table = args.table;
        #[cfg(not(any(feature = "sqlite", feature = "elasticsearch")))]
        let table = None;
        #[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
        let native_query = args.query;
        #[cfg(not(any(feature = "sqlite", feature = "elasticsearch")))]
        let native_query = None;
        // Explicit delimited parsing options must outrank a saved structured
        // format. Keep the format automatic so SQLite signatures still win
        // during source probing.
        let resolved_cli_format =
            explicit_format.or_else(|| delimited_option_selected.then_some(InputFormat::Auto));
        Ok(Self {
            target: SourceTarget::from_cli_value(&args.filename),
            interactive: args.interactive,
            output: args.output,
            color: args.color,
            sorted: args.sorted,
            top_lines: args.top_lines,
            encoding: args.encoding,
            delimiter: args.delimiter.as_deref().map(parse_byte_char).transpose()?,
            quoting: args.quoting.as_deref().map(parse_quoting).transpose()?,
            start_position: parse_start_position(args.start_pos.as_deref(), &args.extra)?,
            width: parse_width(&args.width)?,
            double_width: args.double_width,
            quote_char: parse_ascii_char(&args.quote_char, "quote character")?,
            source_options: SourceOptionOverrides {
                format: resolved_cli_format,
                json_path: args.json_path,
                object_mode: args.object_mode,
                schema_scan: args.schema_scan,
                table,
                native_query,
                ..SourceOptionOverrides::default()
            },
            #[cfg(feature = "saved-views")]
            saved_view: SavedViewSelection::from_args(args.view, args.no_view),
        })
    }
}

fn format_rejects_delimited_options(format: InputFormat) -> bool {
    match format {
        InputFormat::Json | InputFormat::Ndjson => true,
        #[cfg(feature = "sqlite")]
        InputFormat::Sqlite => true,
        #[cfg(feature = "elasticsearch")]
        InputFormat::Elasticsearch => true,
        InputFormat::Auto | InputFormat::Delimited => false,
    }
}

#[cfg(feature = "saved-views")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedViewSelection {
    Auto,
    Force(String),
    Disabled,
}

#[cfg(feature = "saved-views")]
impl SavedViewSelection {
    fn from_args(view: Option<String>, no_view: bool) -> Self {
        if no_view {
            Self::Disabled
        } else if let Some(view) = view {
            Self::Force(crate::saved_views::normalize_view_name(&view))
        } else {
            Self::Auto
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StartPosition {
    pub row: usize,
    pub column: Option<usize>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CliError {
    #[error("invalid start position '{value}'")]
    InvalidStartPosition { value: String },
    #[error("invalid column width '{value}'")]
    InvalidWidth { value: String },
    #[error("invalid quoting style '{value}'")]
    InvalidQuoting { value: String },
    #[error("invalid {what} '{value}'")]
    InvalidChar { what: &'static str, value: String },
    #[error("{option} cannot be used with {format} input")]
    IncompatibleOptions {
        format: InputFormat,
        option: &'static str,
    },
}

fn parse_input_format(value: &str) -> Result<InputFormat, String> {
    value
        .parse::<InputFormat>()
        .map_err(|error| error.to_string())
}

fn parse_json_pointer(value: &str) -> Result<JsonPointer, String> {
    value
        .parse::<JsonPointer>()
        .map_err(|error| error.to_string())
}

fn parse_schema_scan(value: &str) -> Result<SchemaScan, String> {
    value
        .parse::<SchemaScan>()
        .map_err(|error| error.to_string())
}

fn parse_object_mode(value: &str) -> Result<ObjectMode, String> {
    value
        .parse::<ObjectMode>()
        .map_err(|error| error.to_string())
}

fn parse_start_position(normal: Option<&str>, extra: &[String]) -> Result<StartPosition, CliError> {
    if let Some(value) = normal {
        return parse_normal_start_position(value);
    }

    if let Some(value) = extra.iter().find(|value| value.starts_with('+')) {
        return parse_classic_start_position(value);
    }

    Ok(StartPosition::default())
}

fn parse_normal_start_position(value: &str) -> Result<StartPosition, CliError> {
    let mut parts = value.split(',');
    let row = parse_optional_usize(parts.next().unwrap_or_default(), value)?;
    let column = parts
        .next()
        .map(|part| parse_optional_usize(part, value))
        .transpose()?;
    if parts.next().is_some() {
        return Err(CliError::InvalidStartPosition {
            value: value.to_owned(),
        });
    }
    Ok(StartPosition { row, column })
}

fn parse_classic_start_position(value: &str) -> Result<StartPosition, CliError> {
    let value_without_plus = value.trim_start_matches('+');
    let mut parts = value_without_plus.split(':');
    let row = parse_optional_usize(parts.next().unwrap_or_default(), value)?;
    let column = parts
        .next()
        .map(|part| parse_optional_usize(part, value))
        .transpose()?
        .or(Some(0));
    if parts.next().is_some() {
        return Err(CliError::InvalidStartPosition {
            value: value.to_owned(),
        });
    }
    Ok(StartPosition { row, column })
}

fn parse_optional_usize(part: &str, full_value: &str) -> Result<usize, CliError> {
    if part.is_empty() {
        return Ok(0);
    }
    part.parse().map_err(|_| CliError::InvalidStartPosition {
        value: full_value.to_owned(),
    })
}

fn parse_width(value: &str) -> Result<ColumnWidthMode, CliError> {
    match value {
        "mode" => Ok(ColumnWidthMode::Mode),
        "max" => Ok(ColumnWidthMode::Max),
        _ => {
            let width = value.parse::<u16>().map_err(|_| CliError::InvalidWidth {
                value: value.to_owned(),
            })?;
            if width == 0 {
                return Err(CliError::InvalidWidth {
                    value: value.to_owned(),
                });
            }
            Ok(ColumnWidthMode::Fixed(width))
        }
    }
}

fn parse_quoting(value: &str) -> Result<Quoting, CliError> {
    match value {
        "QUOTE_MINIMAL" => Ok(Quoting::Minimal),
        "QUOTE_NONNUMERIC" => Ok(Quoting::NonNumeric),
        "QUOTE_ALL" => Ok(Quoting::All),
        "QUOTE_NONE" => Ok(Quoting::None),
        _ => Err(CliError::InvalidQuoting {
            value: value.to_owned(),
        }),
    }
}

fn parse_byte_char(value: &str) -> Result<u8, CliError> {
    if value == r"\t" {
        return Ok(b'\t');
    }
    parse_ascii_char(value, "delimiter")
}

fn parse_ascii_char(value: &str, what: &'static str) -> Result<u8, CliError> {
    let ch = parse_char(value, what)?;
    if ch.is_ascii() {
        Ok(ch as u8)
    } else {
        Err(CliError::InvalidChar {
            what,
            value: value.to_owned(),
        })
    }
}

fn parse_char(value: &str, what: &'static str) -> Result<char, CliError> {
    let mut chars = value.chars();
    let Some(ch) = chars.next() else {
        return Err(CliError::InvalidChar {
            what,
            value: value.to_owned(),
        });
    };
    if chars.next().is_none() {
        Ok(ch)
    } else {
        Err(CliError::InvalidChar {
            what,
            value: value.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn preview_options_require_explicit_valid_values() {
        for args in [
            vec!["tview", "--sorted", "data.csv"],
            vec!["tview", "--sorted", "yes", "data.csv"],
            vec!["tview", "-n", "0", "data.csv"],
            vec!["tview", "-n", "-1", "data.csv"],
            vec!["tview", "-n", "1.5", "data.csv"],
            vec!["tview", "-n", "99999999999999999999999999", "data.csv"],
            vec!["tview", "data.csv", "-n"],
        ] {
            assert!(Args::try_parse_from(args).is_err());
        }
        for flag in ["-n", "--top-lines"] {
            let config = parse(&["tview", "--sorted", "false", flag, "30", "data.csv"]);
            assert_eq!(config.sorted, Some(false));
            assert_eq!(config.top_lines.unwrap().get(), 30);
        }
        assert_eq!(parse(&["tview", "data.csv"]).sorted, None);
        assert_eq!(parse(&["tview", "data.csv"]).top_lines, None);
    }

    fn parse(args: &[&str]) -> Config {
        let args = Args::try_parse_from(args).expect("parse args");
        Config::from_args(args).expect("config")
    }

    fn parse_config_error(args: &[&str]) -> CliError {
        let args = Args::try_parse_from(args).expect("parse args");
        Config::from_args(args).expect_err("config error")
    }

    #[test]
    fn default_width_is_mode() {
        let config = parse(&["tview", "sample/data_ohlcv.csv"]);
        assert_eq!(config.width, ColumnWidthMode::Mode);
    }

    #[test]
    fn parses_composable_runtime_and_output_options() {
        let automatic = parse(&["tview", "sample/data_ohlcv.csv"]);
        assert!(!automatic.interactive);
        assert_eq!(automatic.output, None);
        assert_eq!(automatic.color, ColorOutput::Auto);

        let interactive = parse(&["tview", "-i", "sample/data_ohlcv.csv"]);
        assert!(interactive.interactive);
        assert_eq!(interactive.output, None);

        let direct = parse(&["tview", "-o", "table", "sample/data_ohlcv.csv"]);
        assert!(!direct.interactive);
        assert_eq!(direct.output, Some(OutputFormat::Table));

        let composed = parse(&[
            "tview",
            "--interactive",
            "--output",
            "table",
            "--color",
            "always",
            "sample/data_ohlcv.csv",
        ]);
        assert!(composed.interactive);
        assert_eq!(composed.output, Some(OutputFormat::Table));
        assert_eq!(composed.color, ColorOutput::Always);
    }

    #[test]
    fn rejects_runtime_names_and_unknown_output_formats() {
        assert!(
            Args::try_parse_from(["tview", "--output", "tui", "sample/data_ohlcv.csv"]).is_err()
        );
        assert!(
            Args::try_parse_from(["tview", "--output", "markdown", "sample/data_ohlcv.csv"])
                .is_err()
        );
        assert!(
            Args::try_parse_from(["tview", "--color", "sometimes", "sample/data_ohlcv.csv"])
                .is_err()
        );
    }

    #[test]
    fn rejects_zero_fixed_width() {
        assert_eq!(
            parse_config_error(&["tview", "--width", "0", "sample/data_ohlcv.csv"]),
            CliError::InvalidWidth {
                value: "0".to_owned()
            }
        );
    }

    #[test]
    fn rejects_non_ascii_quote_character() {
        assert_eq!(
            parse_config_error(&["tview", "--quote-char", "“", "sample/data_ohlcv.csv"]),
            CliError::InvalidChar {
                what: "quote character",
                value: "“".to_owned()
            }
        );
    }

    #[test]
    fn parses_readme_start_position() {
        let config = parse(&[
            "tview",
            "sample/data_ohlcv.csv",
            "--start_pos",
            "6,5",
            "--encoding",
            "utf-8",
        ]);
        assert_eq!(
            config.start_position,
            StartPosition {
                row: 6,
                column: Some(5)
            }
        );
        assert_eq!(config.encoding.as_deref(), Some("utf-8"));
    }

    #[test]
    fn rejects_start_position_with_extra_components() {
        assert_eq!(
            parse_config_error(&["tview", "--start_pos", "1,2,3", "sample/data_ohlcv.csv"]),
            CliError::InvalidStartPosition {
                value: "1,2,3".to_owned()
            }
        );
        assert_eq!(
            parse_config_error(&["tview", "sample/data_ohlcv.csv", "+1:2:3"]),
            CliError::InvalidStartPosition {
                value: "+1:2:3".to_owned()
            }
        );
    }

    #[test]
    fn parses_classic_start_position() {
        let config = parse(&["tview", "sample/data_ohlcv.csv", "+6:5"]);
        assert_eq!(
            config.start_position,
            StartPosition {
                row: 6,
                column: Some(5)
            }
        );
    }

    #[test]
    fn parses_classic_row_only_start_position() {
        let config = parse(&["tview", "sample/data_ohlcv.csv", "+6:"]);
        assert_eq!(
            config.start_position,
            StartPosition {
                row: 6,
                column: Some(0)
            }
        );
    }

    #[test]
    fn parses_mysql_pager_shape() {
        let config = parse(&["tview", "-d", r"\t", "--quoting", "QUOTE_NONE", "-"]);
        assert_eq!(config.target, SourceTarget::Stdin);
        assert_eq!(config.delimiter, Some(b'\t'));
        assert_eq!(config.quoting, Some(Quoting::None));
        assert_eq!(config.source_options.format, Some(InputFormat::Auto));
    }

    #[test]
    fn parses_source_open_options() {
        let config = parse(&[
            "tview",
            "--format",
            "json",
            "--json-path",
            "/hits/hits",
            "--schema-scan",
            "full",
            "--object-mode",
            "entries",
            "response.data",
        ]);
        assert_eq!(config.source_options.format, Some(InputFormat::Json));
        assert_eq!(
            config
                .source_options
                .json_path
                .as_ref()
                .expect("path")
                .segments(),
            ["hits", "hits"]
        );
        assert_eq!(config.source_options.schema_scan, Some(SchemaScan::Full));
        assert_eq!(config.source_options.object_mode, Some(ObjectMode::Entries));
    }

    #[test]
    fn validates_object_mode_values_and_explicit_row_stream_conflicts() {
        assert!(Args::try_parse_from(["tview", "--object-mode", "rows", "response.json"]).is_err());
        for format in ["delimited", "ndjson"] {
            assert_eq!(
                parse_config_error(&[
                    "tview",
                    "--format",
                    format,
                    "--object-mode",
                    "entries",
                    "data"
                ]),
                CliError::IncompatibleOptions {
                    format: format.parse().expect("format"),
                    option: "--object-mode",
                }
            );
        }
    }

    #[test]
    fn help_documents_format_neutral_object_mode_values() {
        let help = Args::command().render_long_help().to_string();
        assert!(help.contains("--object-mode <OBJECT_MODE>"));
        assert!(help.contains("selected structured object"));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_feature_exposes_its_cli_surface() {
        let config = parse(&[
            "tview",
            "--format",
            "sqlite",
            "--table",
            "users",
            "application.db",
        ]);
        assert_eq!(config.source_options.format, Some(InputFormat::Sqlite));
        assert_eq!(config.source_options.table.as_deref(), Some("users"));
        let query = parse(&[
            "tview",
            "--format",
            "sqlite",
            "--query",
            "SELECT * FROM users",
            "application.db",
        ]);
        assert_eq!(
            query.source_options.native_query.as_deref(),
            Some("SELECT * FROM users")
        );

        let help = Args::command().render_long_help().to_string();
        assert!(help.contains("--table <TABLE>"));
        assert!(help.contains("--query <QUERY>"));
        assert!(help.contains("SQLite"));
    }

    #[cfg(any(feature = "sqlite", feature = "elasticsearch"))]
    #[test]
    fn native_query_and_table_conflict_at_argument_parsing() {
        assert!(Args::try_parse_from([
            "tview",
            "--table",
            "users",
            "--query",
            "SELECT * FROM users",
            "application.db"
        ])
        .is_err());
    }

    #[cfg(feature = "elasticsearch")]
    #[test]
    fn elasticsearch_feature_exposes_remote_target_and_query() {
        let config = parse(&[
            "tview",
            "https://elastic.example:9200",
            "--format",
            "elasticsearch",
            "--query",
            "FROM logs-* | LIMIT 10",
        ]);
        assert!(config.target.as_url().is_some());
        assert_eq!(
            config.source_options.format,
            Some(InputFormat::Elasticsearch)
        );
        assert_eq!(
            config.source_options.native_query.as_deref(),
            Some("FROM logs-* | LIMIT 10")
        );
    }

    #[cfg(all(feature = "elasticsearch", not(feature = "sqlite")))]
    #[test]
    fn elasticsearch_only_help_does_not_claim_sqlite_support() {
        let help = Args::command().render_long_help().to_string();
        assert!(help.contains("Elasticsearch"));
        assert!(!help.contains("SQLite"));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn table_can_be_combined_with_explicit_auto_format() {
        let config = parse(&[
            "tview",
            "--format",
            "auto",
            "--table",
            "users",
            "application.db",
        ]);

        assert_eq!(config.source_options.format, Some(InputFormat::Auto));
        assert_eq!(config.source_options.table.as_deref(), Some("users"));
    }

    #[cfg(not(any(feature = "sqlite", feature = "elasticsearch")))]
    #[test]
    fn native_source_features_remove_their_cli_surface() {
        assert!(Args::try_parse_from(["tview", "--format", "sqlite", "application.db"]).is_err());
        assert!(Args::try_parse_from(["tview", "--table", "users", "application.db"]).is_err());
        assert!(Args::try_parse_from(["tview", "--query", "SELECT 1", "application.db"]).is_err());

        let help = Args::command().render_long_help().to_string();
        assert!(!help.contains("--table <TABLE>"));
        assert!(!help.contains("local SQLite"));
    }

    #[test]
    fn object_mode_does_not_imply_a_format_for_stdin() {
        let config = parse(&["tview", "--object-mode", "entries", "-"]);
        assert_eq!(config.target, SourceTarget::Stdin);
        assert_eq!(config.source_options.format, None);
        assert_eq!(config.source_options.object_mode, Some(ObjectMode::Entries));
    }

    #[test]
    fn elasticsearch_json_path_cli_opens_only_hit_rows() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("sample/json/elasticsearch-response.json");
        let fixture_arg = fixture.to_string_lossy().into_owned();
        let config = parse(&[
            "tview",
            "--format",
            "json",
            "--json-path",
            "/hits/hits",
            &fixture_arg,
        ]);
        let options = crate::ingest::OpenOptions::merge(
            crate::ingest::OpenOptions::default(),
            &crate::ingest::SourceOptionOverrides::default(),
            &config.source_options,
        );
        let table =
            crate::ingest::open_source(crate::ingest::source::InputSource::Path(fixture), &options)
                .expect("open fixture")
                .into_implicit_table()
                .expect("table");
        let identities = table
            .definition
            .columns
            .iter()
            .filter_map(|column| match &column.source_identity {
                crate::table::ColumnSourceIdentity::StructuredPath(pointer) => {
                    Some(pointer.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(identities.contains(&"/_source/user/id"));
        assert!(!identities.iter().any(|path| path.contains("took")));
        assert!(!identities.iter().any(|path| path.contains("total")));
    }

    #[test]
    fn rejects_invalid_json_pointer_during_argument_parsing() {
        assert!(
            Args::try_parse_from(["tview", "--json-path", "hits/hits", "response.json"]).is_err()
        );
    }

    #[test]
    fn rejects_json_path_with_implicit_delimited_options() {
        let error = Config::from_args(
            Args::try_parse_from([
                "tview",
                "--json-path",
                "/rows",
                "--delimiter",
                "|",
                "response.data",
            ])
            .expect("arguments"),
        )
        .expect_err("incompatible options");

        assert!(matches!(
            error,
            CliError::IncompatibleOptions {
                format: InputFormat::Delimited,
                option: "--json-path"
            }
        ));
        assert_eq!(
            error.to_string(),
            "--json-path cannot be used with delimited input"
        );
    }

    #[test]
    fn explicit_delimited_options_override_saved_format_with_auto_resolution() {
        let config = parse(&["tview", "--delimiter", "|", "data.unknown"]);
        assert_eq!(config.source_options.format, Some(InputFormat::Auto));

        let options = crate::ingest::OpenOptions::merge(
            crate::ingest::OpenOptions::default(),
            &crate::ingest::SourceOptionOverrides {
                format: Some(InputFormat::Json),
                ..crate::ingest::SourceOptionOverrides::default()
            },
            &config.source_options,
        );
        assert_eq!(options.format, InputFormat::Auto);
    }

    #[test]
    fn rejects_delimited_options_with_structured_format() {
        assert_eq!(
            parse_config_error(&["tview", "--format", "json", "--delimiter", ",", "data.json"]),
            CliError::IncompatibleOptions {
                format: InputFormat::Json,
                option: "delimited parsing options"
            }
        );
        assert_eq!(
            parse_config_error(&[
                "tview",
                "--format",
                "delimited",
                "--json-path",
                "/rows",
                "data.csv"
            ]),
            CliError::IncompatibleOptions {
                format: InputFormat::Delimited,
                option: "--json-path"
            }
        );
    }

    #[cfg(feature = "saved-views")]
    #[test]
    fn parses_saved_view_selection_flags() {
        let config = parse(&["tview", "--view", "cat-shards.yml", "sample/data.csv"]);
        assert_eq!(
            config.saved_view,
            SavedViewSelection::Force("cat-shards".to_owned())
        );

        let config = parse(&["tview", "--no-view", "sample/data.csv"]);
        assert_eq!(config.saved_view, SavedViewSelection::Disabled);

        assert!(Args::try_parse_from([
            "tview",
            "--view",
            "cat-shards",
            "--no-view",
            "sample/data.csv"
        ])
        .is_err());
    }
}
