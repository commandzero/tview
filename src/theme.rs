use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ratatui::style::{Color, Modifier, Style};
use serde::Deserialize;
use yaml_serde::{Mapping, Value};

const CONFIG_FILE: &str = "config.yml";
const THEME_DIR: &str = "themes";
const DEFAULT_THEME_NAME: &str = "cmdzro";

const REQUIRED_STYLE_TOKENS: &[&str] = &[
    "table.location",
    "table.current_cell",
    "table.divider",
    "table.header",
    "table.cell",
    "table.selected",
    "table.hidden_marker",
    "popup.background",
    "popup.border",
    "popup.title",
    "popup.body",
    "popup.disabled",
    "popup.active",
    "popup.action",
    "search.highlight",
    "message.footer",
];

const OPTIONAL_STYLE_TOKENS: &[&str] = &[
    "table.header_glyph",
    "table.header_selected",
    "popup.section_title",
    "popup.option_selected",
    "message.info",
    "message.warning",
    "message.error",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeLoad {
    pub theme: ResolvedTheme,
    pub warnings: Vec<ThemeWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeWarning {
    pub field: String,
    pub message: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ThemeError {
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ColorMode {
    #[default]
    Auto,
    Ansi16,
    Ansi256,
    Hex32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalColorMode {
    Ansi16,
    Ansi256,
    TrueColor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTheme {
    name: String,
    mode: ColorMode,
    resolved_mode: TerminalColorMode,
    styles: BTreeMap<String, Style>,
    palette: BTreeMap<String, ConfiguredColor>,
    identifier_colors: Vec<String>,
}

impl ResolvedTheme {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn mode(&self) -> ColorMode {
        self.mode
    }

    pub fn resolved_mode(&self) -> TerminalColorMode {
        self.resolved_mode
    }

    pub fn style(&self, token: &str) -> Style {
        self.styles.get(token).copied().unwrap_or_default()
    }

    pub fn style_or(&self, token: &str, fallback: &str) -> Style {
        self.styles
            .get(token)
            .or_else(|| self.styles.get(fallback))
            .copied()
            .unwrap_or_default()
    }

    pub fn conditional_style(&self, color_ref: &str) -> Option<Style> {
        if let Some(identifier) = parse_identifier_ref(color_ref) {
            return match identifier.colors {
                IdentifierColorRefColors::Auto => self.identifier_style(identifier.index),
                IdentifierColorRefColors::Colors(colors) => {
                    self.identifier_style_with_colors(identifier.index, &colors)
                }
            };
        }
        if let Some(gradient) = parse_gradient_ref(color_ref) {
            let color = self.gradient_color(gradient).ok()?;
            return Some(Style::default().fg(color));
        }
        let color = self.resolve_color_ref(color_ref).ok()?;
        Some(Style::default().fg(color))
    }

    fn identifier_style(&self, index: usize) -> Option<Style> {
        let rgb = self.identifier_rgb(index, &self.identifier_colors).ok()?;
        Some(Style::default().fg(color_for_terminal(
            ResolvedColor::Rgb {
                r: rgb.0,
                g: rgb.1,
                b: rgb.2,
                a: 255,
            },
            self.resolved_mode,
        )))
    }

    fn identifier_style_with_colors(&self, index: usize, colors: &[String]) -> Option<Style> {
        let rgb = self.identifier_rgb(index, colors).ok()?;
        Some(Style::default().fg(color_for_terminal(
            ResolvedColor::Rgb {
                r: rgb.0,
                g: rgb.1,
                b: rgb.2,
                a: 255,
            },
            self.resolved_mode,
        )))
    }

    fn identifier_rgb(&self, index: usize, colors: &[String]) -> Result<(u8, u8, u8), ThemeError> {
        let family_count = if colors.is_empty() {
            DEFAULT_IDENTIFIER_COLORS.len()
        } else {
            colors.len()
        };
        let family = index % family_count;
        let shade = (index / family_count) % IDENTIFIER_SHADES;
        let color = if colors.is_empty() {
            DEFAULT_IDENTIFIER_COLORS[family]
        } else {
            colors[family].as_str()
        };
        let target = self.color_ref_rgb(color)?;
        let start = dark_identifier_rgb(target);
        let ratio = shade as f64 / (IDENTIFIER_SHADES - 1) as f64;
        Ok(interpolate_rgb(start, target, ratio))
    }

    fn gradient_color(&self, gradient: GradientColorRef) -> Result<Color, ThemeError> {
        if gradient.colors.is_empty() {
            return Err(ThemeError::Invalid(
                "gradient color ref requires colors".to_owned(),
            ));
        }
        let rgb =
            self.interpolate_gradient_rgb(&gradient.colors, gradient.bucket, gradient.steps)?;
        Ok(color_for_terminal(
            ResolvedColor::Rgb {
                r: rgb.0,
                g: rgb.1,
                b: rgb.2,
                a: 255,
            },
            self.resolved_mode,
        ))
    }

    fn interpolate_gradient_rgb(
        &self,
        colors: &[String],
        bucket: usize,
        steps: usize,
    ) -> Result<(u8, u8, u8), ThemeError> {
        if colors.len() == 1 || steps <= 1 {
            return self.color_ref_rgb(&colors[0]);
        }
        let max_bucket = steps - 1;
        let position = bucket.min(max_bucket) as f64 / max_bucket as f64;
        let scaled = position * (colors.len() - 1) as f64;
        let left_idx = scaled.floor() as usize;
        let right_idx = scaled.ceil() as usize;
        let left = self.color_ref_rgb(&colors[left_idx])?;
        let right = self.color_ref_rgb(&colors[right_idx])?;
        Ok(interpolate_rgb(left, right, scaled - left_idx as f64))
    }

    fn color_ref_rgb(&self, color_ref: &str) -> Result<(u8, u8, u8), ThemeError> {
        let configured = if self.palette.contains_key(color_ref) {
            ConfiguredColor::Alias(color_ref.to_owned())
        } else {
            parse_configured_color(color_ref)
                .unwrap_or_else(|| ConfiguredColor::Alias(color_ref.to_owned()))
        };
        let resolved = self.resolve_configured_color(&configured, &mut BTreeSet::new())?;
        Ok(resolved_color_rgb(resolved))
    }

    fn resolve_color_ref(&self, color_ref: &str) -> Result<Color, ThemeError> {
        let configured = if self.palette.contains_key(color_ref) {
            ConfiguredColor::Alias(color_ref.to_owned())
        } else {
            parse_configured_color(color_ref)
                .unwrap_or_else(|| ConfiguredColor::Alias(color_ref.to_owned()))
        };
        let resolved = self.resolve_configured_color(&configured, &mut BTreeSet::new())?;
        Ok(color_for_terminal(resolved, self.resolved_mode))
    }

    fn resolve_configured_color(
        &self,
        color: &ConfiguredColor,
        seen: &mut BTreeSet<String>,
    ) -> Result<ResolvedColor, ThemeError> {
        match color {
            ConfiguredColor::Ansi16(color) => Ok(ResolvedColor::Ansi16(*color)),
            ConfiguredColor::Ansi256(color) => Ok(ResolvedColor::Ansi256(*color)),
            ConfiguredColor::Rgb { r, g, b, a } => Ok(ResolvedColor::Rgb {
                r: *r,
                g: *g,
                b: *b,
                a: *a,
            }),
            ConfiguredColor::Alias(alias) => {
                if !seen.insert(alias.clone()) {
                    return Err(ThemeError::Invalid(format!("cyclic color alias '{alias}'")));
                }
                let Some(target) = self.palette.get(alias) else {
                    return Err(ThemeError::Invalid(format!(
                        "unknown color alias '{alias}'"
                    )));
                };
                self.resolve_configured_color(target, seen)
            }
        }
    }
}

pub fn load_active_theme(config_root: Option<&Path>) -> Result<ThemeLoad, ThemeError> {
    let terminal_mode = terminal_color_mode_from_env();
    let Some(root) = config_root.map(Path::to_path_buf).or_else(tview_config_dir) else {
        return Ok(ThemeLoad {
            theme: default_theme_for_terminal(terminal_mode),
            warnings: Vec::new(),
        });
    };

    let selected = selected_theme_from_config(&root)?;
    let discovery = discover_themes_in_root(&root, selected.as_deref(), terminal_mode);
    let selected_name = selected.as_deref().unwrap_or(DEFAULT_THEME_NAME);
    if selected.is_none() {
        return Ok(ThemeLoad {
            theme: default_theme_for_terminal(terminal_mode),
            warnings: discovery.warnings,
        });
    }
    if let Some(theme) = discovery.themes.get(selected_name) {
        return Ok(ThemeLoad {
            theme: theme.clone(),
            warnings: discovery.warnings,
        });
    }
    if let Some(warning) = discovery.selected_failure {
        return Err(ThemeError::Invalid(format!(
            "selected theme '{selected_name}' could not be loaded: {}: {}",
            warning.field, warning.message
        )));
    }
    if selected_name == DEFAULT_THEME_NAME {
        return Ok(ThemeLoad {
            theme: default_theme_for_terminal(terminal_mode),
            warnings: discovery.warnings,
        });
    }
    Err(ThemeError::Invalid(format!(
        "selected theme '{selected_name}' was not found"
    )))
}

pub fn terminal_color_mode_from_env() -> TerminalColorMode {
    terminal_color_mode_from_values(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("COLORTERM").ok().as_deref(),
    )
}

fn terminal_color_mode_from_values(
    term: Option<&str>,
    colorterm: Option<&str>,
) -> TerminalColorMode {
    let colorterm = colorterm.unwrap_or_default().to_ascii_lowercase();
    if colorterm.contains("truecolor") || colorterm.contains("24bit") {
        return TerminalColorMode::TrueColor;
    }

    let term = term.unwrap_or_default().to_ascii_lowercase();
    if term.contains("truecolor") || term.contains("24bit") || term.contains("direct") {
        TerminalColorMode::TrueColor
    } else if term.contains("256color") {
        TerminalColorMode::Ansi256
    } else {
        TerminalColorMode::Ansi16
    }
}

pub fn tview_config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .map(|root| root.join("tview"))
}

fn selected_theme_from_config(root: &Path) -> Result<Option<String>, ThemeError> {
    let path = root.join(CONFIG_FILE);
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(ThemeError::Invalid(format!(
                "{}: failed to read config: {err}",
                path.display()
            )));
        }
    };
    parse_config_theme(&contents)
        .map_err(|message| ThemeError::Invalid(format!("{}: {message}", path.display())))
}

pub fn parse_config_theme(input: &str) -> Result<Option<String>, String> {
    let raw: Option<RawTviewConfig> =
        yaml_serde::from_str(input).map_err(|err| format!("invalid config yaml: {err}"))?;
    Ok(raw.and_then(|config| config.theme))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTviewConfig {
    theme: Option<String>,
}

#[derive(Debug, Default)]
struct ThemeDiscovery {
    themes: BTreeMap<String, ResolvedTheme>,
    warnings: Vec<ThemeWarning>,
    selected_failure: Option<ThemeWarning>,
}

fn discover_themes_in_root(
    root: &Path,
    selected: Option<&str>,
    terminal_mode: TerminalColorMode,
) -> ThemeDiscovery {
    let mut discovery = ThemeDiscovery::default();
    let theme_dir = root.join(THEME_DIR);
    let Ok(entries) = fs::read_dir(&theme_dir) else {
        return discovery;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| matches!(theme_extension(path), Some("yml" | "yaml")))
        .collect::<Vec<_>>();
    paths.sort_by(|left, right| {
        theme_stem(left)
            .cmp(&theme_stem(right))
            .then_with(|| theme_extension_priority(left).cmp(&theme_extension_priority(right)))
            .then_with(|| left.cmp(right))
    });

    let mut seen = BTreeSet::new();
    for path in paths {
        let Some(stem) = theme_stem(&path) else {
            continue;
        };
        let canonical_name = stem.to_owned();
        if !seen.insert(canonical_name.clone()) {
            let message = if theme_extension(&path) == Some("yaml") {
                format!(
                    "duplicate theme '{}': .yml takes precedence over .yaml",
                    canonical_name
                )
            } else {
                format!(
                    "duplicate theme '{}': first discovered file takes precedence",
                    canonical_name
                )
            };
            discovery.warnings.push(ThemeWarning {
                field: path.display().to_string(),
                message,
            });
            continue;
        }
        match fs::read_to_string(&path)
            .map_err(|err| err.to_string())
            .and_then(|contents| parse_theme_yaml(&contents, terminal_mode))
        {
            Ok(mut theme) => {
                if theme.name != canonical_name {
                    discovery.warnings.push(ThemeWarning {
                        field: path.display().to_string(),
                        message: format!(
                            "theme name '{}' does not match file name '{}'; using file name",
                            theme.name, canonical_name
                        ),
                    });
                }
                theme.name = canonical_name.clone();
                discovery.themes.insert(canonical_name, theme);
            }
            Err(message) => {
                let warning = ThemeWarning {
                    field: path.display().to_string(),
                    message: format!("failed to load theme: {message}"),
                };
                if selected.is_some_and(|selected| selected == canonical_name) {
                    discovery.selected_failure = Some(warning.clone());
                }
                discovery.warnings.push(warning);
            }
        }
    }
    discovery
}

fn theme_stem(path: &Path) -> Option<&str> {
    path.file_stem().and_then(|stem| stem.to_str())
}

fn theme_extension(path: &Path) -> Option<&str> {
    path.extension().and_then(|extension| extension.to_str())
}

fn theme_extension_priority(path: &Path) -> u8 {
    match theme_extension(path) {
        Some("yml") => 0,
        Some("yaml") => 1,
        _ => 2,
    }
}

pub fn default_theme() -> ResolvedTheme {
    default_theme_for_terminal(TerminalColorMode::TrueColor)
}

pub fn default_theme_for_terminal(terminal_mode: TerminalColorMode) -> ResolvedTheme {
    let palette = BTreeMap::from([
        (
            "text".to_owned(),
            ConfiguredColor::Rgb {
                r: 175,
                g: 175,
                b: 175,
                a: 255,
            },
        ),
        ("muted".to_owned(), ConfiguredColor::Ansi256(242)),
        ("dim".to_owned(), ConfiguredColor::Ansi256(240)),
        (
            "background".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::Black),
        ),
        (
            "popup_bg".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::Black),
        ),
        ("ui_blue".to_owned(), ConfiguredColor::Ansi256(19)),
        ("blue".to_owned(), ConfiguredColor::Ansi16(AnsiColor::Blue)),
        ("dark_blue".to_owned(), ConfiguredColor::Ansi256(19)),
        (
            "black".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::Black),
        ),
        ("gray".to_owned(), ConfiguredColor::Ansi16(AnsiColor::Gray)),
        ("cyan".to_owned(), ConfiguredColor::Ansi16(AnsiColor::Cyan)),
        (
            "dark_cyan".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::DarkCyan),
        ),
        (
            "green".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::DarkGreen),
        ),
        (
            "magenta".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::Magenta),
        ),
        (
            "yellow".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::Yellow),
        ),
        (
            "error".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::DarkRed),
        ),
        (
            "white".to_owned(),
            ConfiguredColor::Ansi16(AnsiColor::White),
        ),
    ]);
    let mut theme = ResolvedTheme {
        name: DEFAULT_THEME_NAME.to_owned(),
        mode: ColorMode::Auto,
        resolved_mode: terminal_mode,
        styles: BTreeMap::new(),
        palette,
        identifier_colors: default_identifier_colors(),
    };
    for (token, spec) in [
        ("table.location", StyleSpec::new().fg("gray").bg("black")),
        (
            "table.current_cell",
            StyleSpec::new().fg("cyan").bg("dark_blue"),
        ),
        ("table.divider", StyleSpec::new().fg("gray")),
        (
            "table.header",
            StyleSpec::new().fg("dark_cyan").modifier(Modifier::BOLD),
        ),
        (
            "table.header_selected",
            StyleSpec::new().fg("cyan").modifier(Modifier::BOLD),
        ),
        ("table.header_glyph", StyleSpec::new().fg("muted")),
        ("table.cell", StyleSpec::new().fg("text")),
        (
            "table.selected",
            StyleSpec::new().fg("text").bg("dark_blue"),
        ),
        ("table.hidden_marker", StyleSpec::new().fg("muted")),
        (
            "popup.background",
            StyleSpec::new().fg("text").bg("dark_blue"),
        ),
        ("popup.border", StyleSpec::new().fg("cyan").bg("dark_blue")),
        (
            "popup.title",
            StyleSpec::new()
                .fg("gray")
                .bg("dark_blue")
                .modifier(Modifier::BOLD),
        ),
        ("popup.body", StyleSpec::new().fg("text").bg("dark_blue")),
        ("popup.disabled", StyleSpec::new().fg("dim").bg("dark_blue")),
        (
            "popup.section_title",
            StyleSpec::new()
                .fg("gray")
                .bg("dark_blue")
                .modifier(Modifier::BOLD),
        ),
        (
            "popup.active",
            StyleSpec::new()
                .fg("gray")
                .bg("dark_blue")
                .modifier(Modifier::BOLD),
        ),
        (
            "popup.option_selected",
            StyleSpec::new()
                .fg("cyan")
                .bg("dark_blue")
                .modifier(Modifier::BOLD),
        ),
        (
            "popup.action",
            StyleSpec::new()
                .fg("cyan")
                .bg("dark_blue")
                .modifier(Modifier::BOLD),
        ),
        (
            "search.highlight",
            StyleSpec::new().fg("yellow").modifier(Modifier::UNDERLINED),
        ),
        (
            "message.footer",
            StyleSpec::new().fg("yellow").bg("ui_blue"),
        ),
        ("message.info", StyleSpec::new().fg("yellow").bg("ui_blue")),
        (
            "message.warning",
            StyleSpec::new().fg("yellow").bg("ui_blue"),
        ),
        ("message.error", StyleSpec::new().fg("white").bg("error")),
    ] {
        theme.styles.insert(
            token.to_owned(),
            resolve_style_spec(&theme, &spec).expect("default theme resolves"),
        );
    }
    theme
}

pub fn parse_theme_yaml(
    input: &str,
    terminal_mode: TerminalColorMode,
) -> Result<ResolvedTheme, String> {
    let raw: RawTheme =
        yaml_serde::from_str(input).map_err(|err| format!("invalid theme yaml: {err}"))?;
    let mode = raw
        .mode
        .as_deref()
        .map(parse_mode)
        .transpose()?
        .unwrap_or(ColorMode::Auto);
    let palette = raw
        .palette
        .into_iter()
        .map(|(name, value)| {
            parse_color_or_alias_value(value).and_then(|color| {
                if name.trim().is_empty() {
                    Err("palette alias names must not be empty".to_owned())
                } else {
                    Ok((name, color))
                }
            })
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let style_specs = collect_style_specs(raw.styles)?;
    let identifier_colors = match raw.identifiers.colors {
        None => default_identifier_colors(),
        Some(colors) => validate_identifier_colors(colors)?,
    };

    let mut theme = ResolvedTheme {
        name: raw.name,
        mode,
        resolved_mode: resolve_terminal_mode(mode, terminal_mode),
        styles: BTreeMap::new(),
        palette,
        identifier_colors,
    };
    validate_theme_identifier_colors(&theme)?;
    for token in REQUIRED_STYLE_TOKENS {
        let spec = style_specs
            .get(*token)
            .ok_or_else(|| format!("missing required style token '{token}'"))?;
        let style = resolve_style_spec(&theme, spec).map_err(|err| err.to_string())?;
        theme.styles.insert((*token).to_owned(), style);
    }
    for token in OPTIONAL_STYLE_TOKENS {
        if let Some(spec) = style_specs.get(*token) {
            let style = resolve_style_spec(&theme, spec).map_err(|err| err.to_string())?;
            theme.styles.insert((*token).to_owned(), style);
        }
    }
    Ok(theme)
}

fn resolve_terminal_mode(mode: ColorMode, terminal_mode: TerminalColorMode) -> TerminalColorMode {
    match mode {
        ColorMode::Auto => terminal_mode,
        ColorMode::Ansi16 => TerminalColorMode::Ansi16,
        ColorMode::Ansi256 => match terminal_mode {
            TerminalColorMode::Ansi16 => TerminalColorMode::Ansi16,
            TerminalColorMode::Ansi256 | TerminalColorMode::TrueColor => TerminalColorMode::Ansi256,
        },
        ColorMode::Hex32 => terminal_mode,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTheme {
    name: String,
    mode: Option<String>,
    #[serde(default)]
    palette: BTreeMap<String, String>,
    #[serde(default)]
    identifiers: RawThemeIdentifiers,
    #[serde(default)]
    styles: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawThemeIdentifiers {
    colors: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct StyleSpec {
    fg: Option<String>,
    bg: Option<String>,
    modifiers: Vec<Modifier>,
}

impl StyleSpec {
    fn new() -> Self {
        Self::default()
    }

    fn fg(mut self, value: &str) -> Self {
        self.fg = Some(value.to_owned());
        self
    }

    fn bg(mut self, value: &str) -> Self {
        self.bg = Some(value.to_owned());
        self
    }

    fn modifier(mut self, value: Modifier) -> Self {
        self.modifiers.push(value);
        self
    }
}

fn collect_style_specs(styles: Value) -> Result<BTreeMap<String, StyleSpec>, String> {
    let mut specs = BTreeMap::new();
    if matches!(styles, Value::Null) {
        return Ok(specs);
    }
    collect_style_specs_at(Vec::new(), styles, &mut specs)?;
    Ok(specs)
}

fn collect_style_specs_at(
    path: Vec<String>,
    value: Value,
    specs: &mut BTreeMap<String, StyleSpec>,
) -> Result<(), String> {
    let Value::Mapping(mapping) = value else {
        let token = path.join(".");
        let field = if token.is_empty() {
            "styles".to_owned()
        } else {
            format!("styles.{token}")
        };
        return Err(format!("{field} must be a mapping"));
    };
    if is_style_leaf(&mapping) {
        let token = path.join(".");
        validate_style_token(&token)?;
        specs.insert(token, style_spec_from_mapping(&mapping, &path)?);
    }
    for (key, child) in mapping {
        let key = yaml_key_string(key)?;
        if is_style_field(&key) {
            continue;
        }
        let mut child_path = path.clone();
        child_path.push(key);
        collect_style_specs_at(child_path, child, specs)?;
    }
    Ok(())
}

fn is_style_leaf(mapping: &Mapping) -> bool {
    ["fg", "bg", "modifiers"]
        .into_iter()
        .any(|key| mapping_get_string(mapping, key).is_some())
}

fn is_style_field(key: &str) -> bool {
    matches!(key, "fg" | "bg" | "modifiers")
}

fn yaml_key_string(key: Value) -> Result<String, String> {
    match key {
        Value::String(key) if !key.trim().is_empty() => Ok(key),
        Value::String(_) => Err("style path keys must not be empty".to_owned()),
        _ => Err("style path keys must be strings".to_owned()),
    }
}

fn validate_style_token(token: &str) -> Result<(), String> {
    if REQUIRED_STYLE_TOKENS.contains(&token) || OPTIONAL_STYLE_TOKENS.contains(&token) {
        Ok(())
    } else {
        Err(format!("unsupported style token '{token}'"))
    }
}

fn style_spec_from_mapping(mapping: &Mapping, path: &[String]) -> Result<StyleSpec, String> {
    let token = path.join(".");
    let fg = mapping_get_string(mapping, "fg")
        .map(|value| yaml_string_ref(value, &format!("styles.{token}.fg")))
        .transpose()?;
    let bg = mapping_get_string(mapping, "bg")
        .map(|value| yaml_string_ref(value, &format!("styles.{token}.bg")))
        .transpose()?;
    let modifiers = mapping_get_string(mapping, "modifiers")
        .map(|value| yaml_modifier_sequence(value, &format!("styles.{token}.modifiers")))
        .transpose()?
        .unwrap_or_default();
    Ok(StyleSpec { fg, bg, modifiers })
}

fn mapping_get_string<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.iter().find_map(|(candidate, mapped)| {
        matches!(candidate, Value::String(candidate) if candidate == key).then_some(mapped)
    })
}

fn yaml_string_ref(value: &Value, field: &str) -> Result<String, String> {
    match value {
        Value::String(value) if !value.trim().is_empty() => Ok(value.clone()),
        Value::String(_) => Err(format!("{field} must not be empty")),
        _ => Err(format!("{field} must be a string")),
    }
}

fn yaml_modifier_sequence(value: &Value, field: &str) -> Result<Vec<Modifier>, String> {
    let Value::Sequence(values) = value else {
        return Err(format!("{field} must be an array of strings"));
    };
    values
        .iter()
        .enumerate()
        .map(|(idx, value)| {
            let modifier = yaml_string_ref(value, &format!("{field}.{idx}"))?;
            modifier_from_name(&modifier)
        })
        .collect()
}

fn resolve_style_spec(theme: &ResolvedTheme, spec: &StyleSpec) -> Result<Style, ThemeError> {
    let mut style = Style::default();
    if let Some(fg) = &spec.fg {
        style = style.fg(theme.resolve_color_ref(fg)?);
    }
    if let Some(bg) = &spec.bg {
        style = style.bg(theme.resolve_color_ref(bg)?);
    }
    for modifier in &spec.modifiers {
        style = style.add_modifier(*modifier);
    }
    Ok(style)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfiguredColor {
    Ansi16(AnsiColor),
    Ansi256(u8),
    Rgb { r: u8, g: u8, b: u8, a: u8 },
    Alias(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedColor {
    Ansi16(AnsiColor),
    Ansi256(u8),
    Rgb { r: u8, g: u8, b: u8, a: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnsiColor {
    Black,
    DarkRed,
    DarkGreen,
    DarkYellow,
    DarkBlue,
    DarkMagenta,
    DarkCyan,
    Gray,
    DarkGray,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

fn parse_color_or_alias_value(value: String) -> Result<ConfiguredColor, String> {
    if value.trim().is_empty() {
        return Err("color value must not be empty".to_owned());
    }
    Ok(parse_configured_color(&value).unwrap_or(ConfiguredColor::Alias(value)))
}

fn parse_configured_color(value: &str) -> Option<ConfiguredColor> {
    parse_ansi_color(value)
        .map(ConfiguredColor::Ansi16)
        .or_else(|| parse_palette_color(value).map(ConfiguredColor::Ansi256))
        .or_else(|| parse_hex_color(value))
}

fn parse_ansi_color(value: &str) -> Option<AnsiColor> {
    let normalized = value.trim().to_ascii_lowercase().replace(['_', ' '], "-");
    Some(match normalized.as_str() {
        "black" => AnsiColor::Black,
        "dark-red" | "darkred" => AnsiColor::DarkRed,
        "dark-green" | "darkgreen" => AnsiColor::DarkGreen,
        "dark-yellow" | "darkyellow" | "brown" => AnsiColor::DarkYellow,
        "dark-blue" | "darkblue" => AnsiColor::DarkBlue,
        "dark-magenta" | "darkmagenta" => AnsiColor::DarkMagenta,
        "dark-cyan" | "darkcyan" => AnsiColor::DarkCyan,
        "gray" | "grey" => AnsiColor::Gray,
        "dark-gray" | "darkgray" | "dark-grey" | "darkgrey" => AnsiColor::DarkGray,
        "red" | "bright-red" | "brightred" => AnsiColor::Red,
        "green" | "bright-green" | "brightgreen" => AnsiColor::Green,
        "yellow" | "bright-yellow" | "brightyellow" => AnsiColor::Yellow,
        "blue" | "bright-blue" | "brightblue" => AnsiColor::Blue,
        "magenta" | "bright-magenta" | "brightmagenta" => AnsiColor::Magenta,
        "cyan" | "bright-cyan" | "brightcyan" => AnsiColor::Cyan,
        "white" | "bright-white" | "brightwhite" => AnsiColor::White,
        _ => return None,
    })
}

fn parse_palette_color(value: &str) -> Option<u8> {
    let value = value.trim();
    if let Some(inner) = value
        .strip_prefix("palette(")
        .and_then(|value| value.strip_suffix(')'))
    {
        return inner.parse::<u8>().ok();
    }
    value.parse::<u8>().ok()
}

fn parse_hex_color(value: &str) -> Option<ConfiguredColor> {
    let hex = value.trim().strip_prefix('#')?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    let a = if hex.len() == 8 {
        u8::from_str_radix(&hex[6..8], 16).ok()?
    } else {
        255
    };
    Some(ConfiguredColor::Rgb { r, g, b, a })
}

fn color_for_terminal(color: ResolvedColor, mode: TerminalColorMode) -> Color {
    match (color, mode) {
        (ResolvedColor::Ansi16(color), TerminalColorMode::Ansi16) => ansi_color(color),
        (ResolvedColor::Ansi16(color), TerminalColorMode::Ansi256) => {
            let (r, g, b) = ansi_rgb(color);
            Color::Indexed(nearest_xterm_256(r, g, b))
        }
        (ResolvedColor::Ansi16(color), TerminalColorMode::TrueColor) => {
            let (r, g, b) = ansi_rgb(color);
            Color::Rgb(r, g, b)
        }
        (ResolvedColor::Ansi256(color), TerminalColorMode::Ansi16) => {
            ansi_color(nearest_ansi16(xterm_256_rgb(color)))
        }
        (ResolvedColor::Ansi256(color), _) => Color::Indexed(color),
        (ResolvedColor::Rgb { r, g, b, .. }, TerminalColorMode::TrueColor) => Color::Rgb(r, g, b),
        (ResolvedColor::Rgb { r, g, b, .. }, TerminalColorMode::Ansi256) => {
            Color::Indexed(nearest_xterm_256(r, g, b))
        }
        (ResolvedColor::Rgb { r, g, b, .. }, TerminalColorMode::Ansi16) => {
            ansi_color(nearest_ansi16((r, g, b)))
        }
    }
}

fn resolved_color_rgb(color: ResolvedColor) -> (u8, u8, u8) {
    match color {
        ResolvedColor::Ansi16(color) => ansi_rgb(color),
        ResolvedColor::Ansi256(color) => xterm_256_rgb(color),
        ResolvedColor::Rgb { r, g, b, .. } => (r, g, b),
    }
}

fn interpolate_rgb(left: (u8, u8, u8), right: (u8, u8, u8), ratio: f64) -> (u8, u8, u8) {
    let ratio = ratio.clamp(0.0, 1.0);
    (
        interpolate_channel(left.0, right.0, ratio),
        interpolate_channel(left.1, right.1, ratio),
        interpolate_channel(left.2, right.2, ratio),
    )
}

fn interpolate_channel(left: u8, right: u8, ratio: f64) -> u8 {
    (left as f64 + (right as f64 - left as f64) * ratio).round() as u8
}

const IDENTIFIER_SHADES: usize = 16;
const DEFAULT_IDENTIFIER_COLORS: &[&str] = &["bright-green", "magenta", "cyan", "white"];

fn default_identifier_colors() -> Vec<String> {
    DEFAULT_IDENTIFIER_COLORS
        .iter()
        .copied()
        .map(str::to_owned)
        .collect()
}

fn dark_identifier_rgb(target: (u8, u8, u8)) -> (u8, u8, u8) {
    const DARK_RATIO: f64 = 0.5;
    let dim = dim_ansi_rgb(nearest_ansi16(target));
    max_rgb(
        (target.0 as f64 * DARK_RATIO).round() as u8,
        (target.1 as f64 * DARK_RATIO).round() as u8,
        (target.2 as f64 * DARK_RATIO).round() as u8,
        dim,
    )
}

fn max_rgb(r: u8, g: u8, b: u8, floor: (u8, u8, u8)) -> (u8, u8, u8) {
    (r.max(floor.0), g.max(floor.1), b.max(floor.2))
}

fn dim_ansi_rgb(color: AnsiColor) -> (u8, u8, u8) {
    match color {
        AnsiColor::Black => (0, 0, 0),
        AnsiColor::DarkRed | AnsiColor::Red => (100, 0, 0),
        AnsiColor::DarkGreen | AnsiColor::Green => (0, 100, 0),
        AnsiColor::DarkYellow | AnsiColor::Yellow => (100, 100, 0),
        AnsiColor::DarkBlue | AnsiColor::Blue => (0, 0, 100),
        AnsiColor::DarkMagenta | AnsiColor::Magenta => (100, 0, 100),
        AnsiColor::DarkCyan | AnsiColor::Cyan => (0, 100, 100),
        AnsiColor::Gray | AnsiColor::DarkGray | AnsiColor::White => (100, 100, 100),
    }
}

fn ansi_color(color: AnsiColor) -> Color {
    match color {
        AnsiColor::Black => Color::Black,
        AnsiColor::DarkRed => Color::Indexed(1),
        AnsiColor::DarkGreen => Color::Indexed(2),
        AnsiColor::DarkYellow => Color::Indexed(3),
        AnsiColor::DarkBlue => Color::Indexed(4),
        AnsiColor::DarkMagenta => Color::Indexed(5),
        AnsiColor::DarkCyan => Color::Indexed(6),
        AnsiColor::Gray => Color::Gray,
        AnsiColor::DarkGray => Color::DarkGray,
        AnsiColor::Red => Color::Red,
        AnsiColor::Green => Color::Green,
        AnsiColor::Yellow => Color::Yellow,
        AnsiColor::Blue => Color::Blue,
        AnsiColor::Magenta => Color::Magenta,
        AnsiColor::Cyan => Color::Cyan,
        AnsiColor::White => Color::White,
    }
}

fn ansi_rgb(color: AnsiColor) -> (u8, u8, u8) {
    match color {
        AnsiColor::Black => (0, 0, 0),
        AnsiColor::DarkRed => (192, 0, 0),
        AnsiColor::DarkGreen => (0, 192, 0),
        AnsiColor::DarkYellow => (192, 192, 0),
        AnsiColor::DarkBlue => (0, 50, 255),
        AnsiColor::DarkMagenta => (192, 0, 192),
        AnsiColor::DarkCyan => (0, 192, 192),
        AnsiColor::Gray => (160, 160, 160),
        AnsiColor::DarkGray => (100, 100, 100),
        AnsiColor::Red => (255, 0, 0),
        AnsiColor::Green => (0, 255, 0),
        AnsiColor::Yellow => (255, 255, 0),
        AnsiColor::Blue => (0, 100, 255),
        AnsiColor::Magenta => (255, 0, 255),
        AnsiColor::Cyan => (0, 255, 255),
        AnsiColor::White => (255, 255, 255),
    }
}

fn nearest_ansi16(rgb: (u8, u8, u8)) -> AnsiColor {
    [
        AnsiColor::Black,
        AnsiColor::DarkRed,
        AnsiColor::DarkGreen,
        AnsiColor::DarkYellow,
        AnsiColor::DarkBlue,
        AnsiColor::DarkMagenta,
        AnsiColor::DarkCyan,
        AnsiColor::Gray,
        AnsiColor::DarkGray,
        AnsiColor::Red,
        AnsiColor::Green,
        AnsiColor::Yellow,
        AnsiColor::Blue,
        AnsiColor::Magenta,
        AnsiColor::Cyan,
        AnsiColor::White,
    ]
    .into_iter()
    .min_by_key(|candidate| color_distance(rgb, ansi_rgb(*candidate)))
    .unwrap_or(AnsiColor::White)
}

fn nearest_xterm_256(r: u8, g: u8, b: u8) -> u8 {
    let rgb = (r, g, b);
    let cube = nearest_xterm_cube_color(r, g, b);
    let gray = nearest_xterm_gray_color(r, g, b);
    if color_distance(rgb, xterm_256_rgb(cube)) <= color_distance(rgb, xterm_256_rgb(gray)) {
        cube
    } else {
        gray
    }
}

fn nearest_xterm_cube_color(r: u8, g: u8, b: u8) -> u8 {
    let r = nearest_xterm_cube_level(r);
    let g = nearest_xterm_cube_level(g);
    let b = nearest_xterm_cube_level(b);
    16 + (36 * r) + (6 * g) + b
}

fn nearest_xterm_cube_level(value: u8) -> u8 {
    match value {
        0..=47 => 0,
        48..=114 => 1,
        _ => ((value as u16 - 35) / 40).min(5) as u8,
    }
}

fn nearest_xterm_gray_color(r: u8, g: u8, b: u8) -> u8 {
    let average = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let gray = if average <= 8 {
        0
    } else {
        ((average - 8 + 5) / 10).min(23)
    };
    232 + gray as u8
}

fn xterm_256_rgb(color: u8) -> (u8, u8, u8) {
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match color {
        0..=15 => ansi_rgb(match color {
            0 => AnsiColor::Black,
            1 => AnsiColor::DarkRed,
            2 => AnsiColor::DarkGreen,
            3 => AnsiColor::DarkYellow,
            4 => AnsiColor::DarkBlue,
            5 => AnsiColor::DarkMagenta,
            6 => AnsiColor::DarkCyan,
            7 => AnsiColor::Gray,
            8 => AnsiColor::DarkGray,
            9 => AnsiColor::Red,
            10 => AnsiColor::Green,
            11 => AnsiColor::Yellow,
            12 => AnsiColor::Blue,
            13 => AnsiColor::Magenta,
            14 => AnsiColor::Cyan,
            _ => AnsiColor::White,
        }),
        16..=231 => {
            let idx = color - 16;
            (
                CUBE[(idx / 36) as usize],
                CUBE[((idx % 36) / 6) as usize],
                CUBE[(idx % 6) as usize],
            )
        }
        232..=255 => {
            let level = 8 + (color - 232) * 10;
            (level, level, level)
        }
    }
}

fn color_distance(left: (u8, u8, u8), right: (u8, u8, u8)) -> u32 {
    let dr = i32::from(left.0) - i32::from(right.0);
    let dg = i32::from(left.1) - i32::from(right.1);
    let db = i32::from(left.2) - i32::from(right.2);
    (dr * dr + dg * dg + db * db) as u32
}

fn parse_mode(value: &str) -> Result<ColorMode, String> {
    match value {
        "auto" => Ok(ColorMode::Auto),
        "ansi16" | "16" => Ok(ColorMode::Ansi16),
        "ansi256" | "256" => Ok(ColorMode::Ansi256),
        "hex32" | "truecolor" => Ok(ColorMode::Hex32),
        _ => Err(format!("unknown color mode '{value}'")),
    }
}

fn modifier_from_name(value: &str) -> Result<Modifier, String> {
    match value {
        "bold" => Ok(Modifier::BOLD),
        "italic" => Ok(Modifier::ITALIC),
        "underline" => Ok(Modifier::UNDERLINED),
        "reversed" => Ok(Modifier::REVERSED),
        "dim" => Ok(Modifier::DIM),
        _ => Err(format!("unknown style modifier '{value}'")),
    }
}

fn validate_identifier_colors(colors: Vec<String>) -> Result<Vec<String>, String> {
    if colors.is_empty() || colors.iter().any(|color| color.trim().is_empty()) {
        return Err("identifiers.colors requires at least one color".to_owned());
    }
    Ok(colors)
}

fn validate_theme_identifier_colors(theme: &ResolvedTheme) -> Result<(), String> {
    for (idx, color) in theme.identifier_colors.iter().enumerate() {
        theme
            .color_ref_rgb(color)
            .map_err(|err| format!("identifiers.colors.{idx}: {err}"))?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConditionalColorRule {
    Match { entries: Vec<MatchEntry> },
    Range { entries: Vec<RangeEntry> },
    FixedGradient { stops: Vec<GradientStop> },
    AutoGradient { colors: Vec<String>, steps: usize },
    Identifiers { colors: IdentifierColors },
}

impl Eq for ConditionalColorRule {}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchEntry {
    pub value: ConditionalValue,
    pub color: String,
}

impl Eq for MatchEntry {}

#[derive(Debug, Clone, PartialEq)]
pub struct RangeEntry {
    pub lt: Option<f64>,
    pub lte: Option<f64>,
    pub gt: Option<f64>,
    pub gte: Option<f64>,
    pub color: String,
}

impl Eq for RangeEntry {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierColors {
    Auto,
    Colors(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientStop {
    pub value: f64,
    pub color: String,
}

impl Eq for GradientStop {}

#[derive(Debug, Clone, PartialEq)]
pub enum ConditionalValue {
    Bool(bool),
    Number(f64),
    String(String),
}

impl Eq for ConditionalValue {}

impl ConditionalColorRule {
    pub fn color_for(
        &self,
        raw: &str,
        rendered: &str,
        numeric: Option<f64>,
        column_min_max: Option<(f64, f64)>,
    ) -> Option<String> {
        self.color_ref_for(raw, rendered, numeric, column_min_max)
            .map(Cow::into_owned)
    }

    pub fn color_ref_for<'a>(
        &'a self,
        raw: &str,
        rendered: &str,
        numeric: Option<f64>,
        column_min_max: Option<(f64, f64)>,
    ) -> Option<Cow<'a, str>> {
        match self {
            ConditionalColorRule::Match { entries } => entries
                .iter()
                .find(|entry| conditional_value_matches(&entry.value, raw, rendered, numeric))
                .map(|entry| Cow::Borrowed(entry.color.as_str())),
            ConditionalColorRule::Range { entries } => {
                let value = numeric?;
                entries
                    .iter()
                    .find(|entry| {
                        entry.lt.is_none_or(|bound| value < bound)
                            && entry.lte.is_none_or(|bound| value <= bound)
                            && entry.gt.is_none_or(|bound| value > bound)
                            && entry.gte.is_none_or(|bound| value >= bound)
                    })
                    .map(|entry| Cow::Borrowed(entry.color.as_str()))
            }
            ConditionalColorRule::FixedGradient { stops } => {
                let value = numeric?;
                stops
                    .iter()
                    .enumerate()
                    .find(|(idx, stop)| {
                        value >= stop.value
                            && stops.get(idx + 1).is_none_or(|next| value < next.value)
                    })
                    .map(|(_, stop)| Cow::Borrowed(stop.color.as_str()))
            }
            ConditionalColorRule::AutoGradient { colors, steps } => {
                let value = numeric?;
                let (min, max) = column_min_max?;
                if colors.is_empty() || max <= min {
                    return colors.first().map(|color| Cow::Borrowed(color.as_str()));
                }
                let steps = (*steps).max(1);
                let ratio = ((value - min) / (max - min)).clamp(0.0, 1.0);
                let bucket = (ratio * steps as f64).floor().min((steps - 1) as f64) as usize;
                Some(Cow::Owned(gradient_color_ref(colors, bucket, steps)))
            }
            ConditionalColorRule::Identifiers { .. } => None,
        }
    }
}

pub fn identifier_color_ref(index: usize, colors: &IdentifierColors) -> String {
    match colors {
        IdentifierColors::Auto => format!("identifier({index})"),
        IdentifierColors::Colors(colors) => {
            format!("identifier({index};{})", encode_color_list(colors))
        }
    }
}

pub(crate) fn gradient_color_ref(colors: &[String], bucket: usize, steps: usize) -> String {
    format!("gradient({bucket};{steps};{})", encode_color_list(colors))
}

fn encode_color_list(colors: &[String]) -> String {
    let mut encoded = String::new();
    for color in colors {
        encoded.push_str(&color.len().to_string());
        encoded.push(':');
        encoded.push_str(color);
    }
    encoded
}

fn parse_encoded_color_list(value: &str) -> Option<Vec<String>> {
    let mut colors = Vec::new();
    let mut cursor = 0;
    while cursor < value.len() {
        let colon = value[cursor..].find(':')? + cursor;
        let len = value[cursor..colon].parse::<usize>().ok()?;
        let start = colon + 1;
        let end = start.checked_add(len)?;
        let color = value.get(start..end)?;
        if color.is_empty() {
            return None;
        }
        colors.push(color.to_owned());
        cursor = end;
    }
    Some(colors)
}

fn conditional_value_matches(
    expected: &ConditionalValue,
    raw: &str,
    rendered: &str,
    numeric: Option<f64>,
) -> bool {
    match expected {
        ConditionalValue::Bool(expected) => {
            parse_bool(raw).is_some_and(|actual| actual == *expected)
                || parse_bool(rendered).is_some_and(|actual| actual == *expected)
        }
        ConditionalValue::Number(expected) => numeric.is_some_and(|actual| actual == *expected),
        ConditionalValue::String(expected) => {
            raw.eq_ignore_ascii_case(expected) || rendered == expected
        }
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" | "1" => Some(true),
        "false" | "no" | "n" | "0" => Some(false),
        _ => None,
    }
}

struct IdentifierColorRef {
    index: usize,
    colors: IdentifierColorRefColors,
}

enum IdentifierColorRefColors {
    Auto,
    Colors(Vec<String>),
}

fn parse_identifier_ref(value: &str) -> Option<IdentifierColorRef> {
    let inner = value.strip_prefix("identifier(")?.strip_suffix(')')?;
    if let Some((index, colors)) = inner.split_once(';') {
        let colors = parse_encoded_color_list(colors)?;
        return Some(IdentifierColorRef {
            index: index.parse().ok()?,
            colors: if colors.is_empty() {
                IdentifierColorRefColors::Auto
            } else {
                IdentifierColorRefColors::Colors(colors)
            },
        });
    }
    let mut parts = inner.split(',');
    let index = parts.next()?.parse().ok()?;
    let colors = parts
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Some(IdentifierColorRef {
        index,
        colors: if colors.is_empty() {
            IdentifierColorRefColors::Auto
        } else {
            IdentifierColorRefColors::Colors(colors)
        },
    })
}

struct GradientColorRef {
    bucket: usize,
    steps: usize,
    colors: Vec<String>,
}

fn parse_gradient_ref(value: &str) -> Option<GradientColorRef> {
    let inner = value.strip_prefix("gradient(")?.strip_suffix(')')?;
    if inner.contains(';') {
        let mut parts = inner.splitn(3, ';');
        let bucket = parts.next()?.parse().ok()?;
        let steps = parts.next()?.parse().ok()?;
        let colors = parse_encoded_color_list(parts.next()?)?;
        return (!colors.is_empty()).then_some(GradientColorRef {
            bucket,
            steps,
            colors,
        });
    }
    let mut parts = inner.split(',');
    let bucket = parts.next()?.parse().ok()?;
    let steps = parts.next()?.parse().ok()?;
    let colors = parts
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    (!colors.is_empty()).then_some(GradientColorRef {
        bucket,
        steps,
        colors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_theme(extra: &str) -> String {
        full_theme_with_text_and_style_overrides("\"#AFAFAFFF\"", extra, &[])
    }

    fn full_theme_with_style_overrides(extra: &str, overrides: &[(&str, &str)]) -> String {
        full_theme_with_text_and_style_overrides("\"#AFAFAFFF\"", extra, overrides)
    }

    fn full_theme_with_text_and_style_overrides(
        text: &str,
        extra: &str,
        overrides: &[(&str, &str)],
    ) -> String {
        let mut theme = format!(
            r##"
name: custom
mode: hex32
palette:
  text: {text}
  blue_ui: palette(19)
  yellow: yellow
  error: dark-red
  bg: black
"##,
        );
        theme.push_str(extra);
        theme.push_str("styles:\n");
        for token in REQUIRED_STYLE_TOKENS {
            theme.push_str(&format!("  \"{token}\":\n"));
            if let Some((_, override_spec)) = overrides
                .iter()
                .find(|(override_token, _)| *override_token == *token)
            {
                for line in override_spec.lines() {
                    theme.push_str("    ");
                    theme.push_str(line);
                    theme.push('\n');
                }
            } else {
                theme.push_str("    fg: text\n");
            }
        }
        theme
    }

    #[test]
    fn config_theme_parses_top_level_theme() {
        assert_eq!(
            parse_config_theme("theme: ops\n").expect("parse"),
            Some("ops".to_owned())
        );
    }

    #[test]
    fn config_theme_accepts_yaml_quoted_strings() {
        assert_eq!(
            parse_config_theme("theme: 'ops#dark' # comment\n").expect("parse"),
            Some("ops#dark".to_owned())
        );
    }

    #[test]
    fn selecting_builtin_cmdzro_without_theme_file_uses_default_theme() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join(CONFIG_FILE), "theme: cmdzro\n").expect("config file");

        let load = load_active_theme(Some(dir.path())).expect("theme load");

        assert_eq!(load.theme.name(), "cmdzro");
        assert!(load.warnings.is_empty());
    }

    #[test]
    fn selecting_invalid_cmdzro_theme_file_errors() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join(CONFIG_FILE), "theme: cmdzro\n").expect("config file");
        let theme_dir = dir.path().join(THEME_DIR);
        fs::create_dir_all(&theme_dir).expect("theme dir");
        fs::write(theme_dir.join("cmdzro.yml"), "name: cmdzro\nstyles: nope\n")
            .expect("theme file");

        let err = load_active_theme(Some(dir.path())).expect_err("theme error");

        assert!(err
            .to_string()
            .contains("selected theme 'cmdzro' could not be loaded"));
    }

    #[test]
    fn detects_terminal_color_mode_from_term_values() {
        assert_eq!(
            terminal_color_mode_from_values(Some("xterm-256color"), None),
            TerminalColorMode::Ansi256
        );
        assert_eq!(
            terminal_color_mode_from_values(Some("xterm"), Some("truecolor")),
            TerminalColorMode::TrueColor
        );
        assert_eq!(
            terminal_color_mode_from_values(Some("xterm-direct"), None),
            TerminalColorMode::TrueColor
        );
        assert_eq!(
            terminal_color_mode_from_values(Some("vt100"), None),
            TerminalColorMode::Ansi16
        );
    }

    #[test]
    fn default_theme_uses_cmdzro_constraints() {
        let theme = default_theme();
        assert_eq!(theme.name(), "cmdzro");
        assert_eq!(theme.resolved_mode(), TerminalColorMode::TrueColor);
        assert_eq!(
            theme.style("table.cell").fg,
            Some(Color::Rgb(175, 175, 175))
        );
        assert_eq!(
            theme.style("table.location").fg,
            Some(Color::Rgb(160, 160, 160))
        );
        assert_eq!(theme.style("table.location").bg, Some(Color::Rgb(0, 0, 0)));
        assert_eq!(
            theme.style("table.current_cell").fg,
            Some(Color::Rgb(0, 255, 255))
        );
        assert_eq!(
            theme.style("table.current_cell").bg,
            Some(Color::Indexed(19))
        );
        assert_eq!(
            theme.style("table.divider").fg,
            Some(Color::Rgb(160, 160, 160))
        );
        assert_eq!(
            theme.style("table.header").fg,
            Some(Color::Rgb(0, 192, 192))
        );
        assert_eq!(
            theme.style("table.header_selected").fg,
            Some(Color::Rgb(0, 255, 255))
        );
        assert_eq!(theme.style("table.selected").bg, Some(Color::Indexed(19)));
        assert!(!theme
            .style("table.selected")
            .add_modifier
            .contains(Modifier::REVERSED));
        assert_eq!(theme.style("popup.background").bg, Some(Color::Indexed(19)));
        assert_eq!(
            theme.style("popup.border").fg,
            Some(Color::Rgb(0, 255, 255))
        );
        assert_eq!(
            theme.style("popup.title").fg,
            Some(Color::Rgb(160, 160, 160))
        );
        assert_eq!(
            theme.style("popup.action").fg,
            Some(Color::Rgb(0, 255, 255))
        );
        assert_eq!(
            theme
                .conditional_style("identifier(0)")
                .expect("identifier")
                .fg,
            Some(Color::Rgb(0, 128, 0))
        );
        assert_eq!(
            theme
                .conditional_style("identifier(1)")
                .expect("identifier")
                .fg,
            Some(Color::Rgb(128, 0, 128))
        );
        assert_eq!(
            theme
                .conditional_style("identifier(4)")
                .expect("identifier")
                .fg,
            Some(Color::Rgb(0, 136, 0))
        );
        assert_ne!(theme.style("table.cell").fg, Some(Color::Blue));
        assert_eq!(
            theme.style("search.highlight").fg,
            Some(Color::Rgb(255, 255, 0))
        );
        assert_eq!(theme.style("search.highlight").bg, None);
        assert!(theme
            .style("search.highlight")
            .add_modifier
            .contains(Modifier::UNDERLINED));
        assert_eq!(theme.style("message.error").bg, Some(Color::Rgb(192, 0, 0)));
    }

    #[test]
    fn default_theme_falls_back_to_ansi16_when_terminal_requires_it() {
        let theme = default_theme_for_terminal(TerminalColorMode::Ansi16);
        assert_eq!(theme.resolved_mode(), TerminalColorMode::Ansi16);
        assert_eq!(
            theme.style("table.current_cell").bg,
            Some(Color::Indexed(4))
        );
    }

    #[test]
    fn parses_colors_aliases_and_modifiers() {
        let theme = parse_theme_yaml(
            &full_theme_with_style_overrides(
                "",
                &[("table.header", "fg: yellow\nmodifiers: [bold, underline]")],
            ),
            TerminalColorMode::TrueColor,
        )
        .expect("theme");

        assert_eq!(theme.mode(), ColorMode::Hex32);
        assert_eq!(
            theme.style("table.header").fg,
            Some(Color::Rgb(255, 255, 0))
        );
        assert!(theme
            .style("table.header")
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(parse_ansi_color("darkgreen"), Some(AnsiColor::DarkGreen));
        assert_eq!(parse_ansi_color("brightcyan"), Some(AnsiColor::Cyan));
    }

    #[test]
    fn theme_identifier_colors_override_default_families() {
        let theme = parse_theme_yaml(
            &full_theme(
                r##"
identifiers:
  colors: ["#ff0000ff", "#00ff00ff"]
"##,
            ),
            TerminalColorMode::TrueColor,
        )
        .expect("theme");

        assert_eq!(
            theme
                .conditional_style("identifier(0)")
                .expect("identifier")
                .fg,
            Some(Color::Rgb(128, 0, 0))
        );
        assert_eq!(
            theme
                .conditional_style("identifier(1)")
                .expect("identifier")
                .fg,
            Some(Color::Rgb(0, 128, 0))
        );
        assert_eq!(
            theme
                .conditional_style("identifier(2)")
                .expect("identifier")
                .fg,
            Some(Color::Rgb(136, 0, 0))
        );
    }

    #[test]
    fn theme_identifier_colors_validate_color_refs() {
        let err = parse_theme_yaml(
            &full_theme(
                r##"
identifiers:
  colors: [missing-alias]
"##,
            ),
            TerminalColorMode::TrueColor,
        )
        .expect_err("invalid theme");

        assert!(err.contains("identifiers.colors.0"));
        assert!(err.contains("unknown color alias 'missing-alias'"));
    }

    #[test]
    fn explicit_identifier_ref_overrides_theme_families() {
        let theme = default_theme();

        assert_eq!(
            theme
                .conditional_style("identifier(0,#ff0000ff,#00ff00ff)")
                .expect("identifier")
                .fg,
            Some(Color::Rgb(128, 0, 0))
        );
        assert_eq!(
            theme
                .conditional_style("identifier(1,#ff0000ff,#00ff00ff)")
                .expect("identifier")
                .fg,
            Some(Color::Rgb(0, 128, 0))
        );
    }

    #[test]
    fn generated_identifier_refs_allow_commas_in_color_aliases() {
        let theme = parse_theme_yaml(
            &full_theme(
                r##"
  "red,alias": "#ff0000ff"
  "green,alias": "#00ff00ff"
"##,
            ),
            TerminalColorMode::TrueColor,
        )
        .expect("theme");
        let colors =
            IdentifierColors::Colors(vec!["red,alias".to_owned(), "green,alias".to_owned()]);

        assert_eq!(
            theme
                .conditional_style(&identifier_color_ref(0, &colors))
                .expect("identifier")
                .fg,
            Some(Color::Rgb(128, 0, 0))
        );
        assert_eq!(
            theme
                .conditional_style(&identifier_color_ref(1, &colors))
                .expect("identifier")
                .fg,
            Some(Color::Rgb(0, 128, 0))
        );
    }

    #[test]
    fn blue_identifier_shades_start_from_dim_blue() {
        assert_eq!(dark_identifier_rgb((0, 0, 255)), (0, 0, 128));
    }

    #[test]
    fn parses_hex32_and_falls_back_to_ansi256() {
        let theme = parse_theme_yaml(
            &full_theme_with_text_and_style_overrides("\"#25A39AFF\"", "", &[]),
            TerminalColorMode::Ansi256,
        )
        .expect("theme");

        assert!(matches!(
            theme.style("table.cell").fg,
            Some(Color::Indexed(_))
        ));
    }

    #[test]
    fn nearest_xterm_256_uses_cube_or_grayscale_candidates() {
        assert_eq!(nearest_xterm_256(0, 0, 0), 16);
        assert_eq!(nearest_xterm_256(255, 255, 255), 231);
        assert_eq!(nearest_xterm_256(128, 128, 128), 244);
    }

    #[test]
    fn detects_unknown_style_token_and_alias_cycle() {
        let invalid = r#"
name: bad
palette:
  a: b
  b: a
styles:
  nope:
    fg: a
"#;
        assert!(parse_theme_yaml(invalid, TerminalColorMode::TrueColor)
            .expect_err("invalid")
            .contains("unsupported style token"));
    }

    #[test]
    fn root_styles_type_error_names_styles_without_empty_token() {
        let invalid = r#"
name: bad
styles: nope
"#;

        assert_eq!(
            parse_theme_yaml(invalid, TerminalColorMode::TrueColor).expect_err("invalid"),
            "styles must be a mapping"
        );
    }

    #[test]
    fn discovery_warns_when_theme_name_does_not_match_file_stem() {
        let dir = tempfile::tempdir().expect("tempdir");
        let theme_dir = dir.path().join(THEME_DIR);
        fs::create_dir_all(&theme_dir).expect("theme dir");
        fs::write(theme_dir.join("from-file.yml"), full_theme("")).expect("theme file");

        let discovery = discover_themes_in_root(dir.path(), None, TerminalColorMode::TrueColor);

        assert!(discovery.themes.contains_key("from-file"));
        assert!(discovery.warnings.iter().any(|warning| {
            warning
                .message
                .contains("does not match file name 'from-file'")
        }));
    }

    #[test]
    fn discovery_warns_and_keeps_first_duplicate_theme_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let theme_dir = dir.path().join(THEME_DIR);
        fs::create_dir_all(&theme_dir).expect("theme dir");
        fs::write(
            theme_dir.join("duplicate.yml"),
            full_theme_with_text_and_style_overrides("\"#111111ff\"", "", &[]),
        )
        .expect("theme file");
        fs::write(
            theme_dir.join("duplicate.yaml"),
            full_theme_with_text_and_style_overrides("\"#eeeeeeff\"", "", &[]),
        )
        .expect("theme file");

        let discovery = discover_themes_in_root(dir.path(), None, TerminalColorMode::TrueColor);

        assert_eq!(
            discovery.themes["duplicate"].style("table.cell").fg,
            Some(Color::Rgb(17, 17, 17))
        );
        assert!(discovery.warnings.iter().any(|warning| {
            warning
                .message
                .contains("duplicate theme 'duplicate': .yml takes precedence over .yaml")
        }));
    }

    #[test]
    fn conditional_rules_match_expected_values() {
        let rule = ConditionalColorRule::Range {
            entries: vec![
                RangeEntry {
                    lt: Some(10.0),
                    lte: None,
                    gt: None,
                    gte: None,
                    color: "red".to_owned(),
                },
                RangeEntry {
                    lt: None,
                    lte: None,
                    gt: None,
                    gte: Some(90.0),
                    color: "red".to_owned(),
                },
            ],
        };
        assert_eq!(
            rule.color_for("9", "9", Some(9.0), None),
            Some("red".to_owned())
        );
        assert_eq!(rule.color_for("10", "10", Some(10.0), None), None);
        assert_eq!(
            rule.color_for("90", "90", Some(90.0), None),
            Some("red".to_owned())
        );

        let rule = ConditionalColorRule::Match {
            entries: vec![
                MatchEntry {
                    value: ConditionalValue::Bool(true),
                    color: "green".to_owned(),
                },
                MatchEntry {
                    value: ConditionalValue::String("p".to_owned()),
                    color: "cyan".to_owned(),
                },
            ],
        };
        assert_eq!(
            rule.color_for("yes", "yes", None, None),
            Some("green".to_owned())
        );
        assert_eq!(
            rule.color_for("p", "p", None, None),
            Some("cyan".to_owned())
        );
    }

    #[test]
    fn auto_gradient_interpolates_across_requested_steps() {
        let colors = vec!["white".to_owned(), "red".to_owned()];
        let rule = ConditionalColorRule::AutoGradient {
            colors: colors.clone(),
            steps: 8,
        };
        let theme = default_theme();

        let start = rule
            .color_for("0", "0", Some(0.0), Some((0.0, 70.0)))
            .expect("start");
        let middle = rule
            .color_for("30", "30", Some(30.0), Some((0.0, 70.0)))
            .expect("middle");
        let end = rule
            .color_for("70", "70", Some(70.0), Some((0.0, 70.0)))
            .expect("end");

        assert_eq!(
            theme.conditional_style(&start).expect("start").fg,
            Some(Color::Rgb(255, 255, 255))
        );
        assert_eq!(
            theme.conditional_style(&middle).expect("middle").fg,
            Some(Color::Rgb(255, 146, 146))
        );
        assert_eq!(
            theme.conditional_style(&end).expect("end").fg,
            Some(Color::Rgb(255, 0, 0))
        );
    }

    #[test]
    fn generated_gradient_refs_allow_commas_in_color_aliases() {
        let theme = parse_theme_yaml(
            &full_theme(
                r##"
  "white,alias": "#ffffffff"
  "red,alias": "#ff0000ff"
"##,
            ),
            TerminalColorMode::TrueColor,
        )
        .expect("theme");
        let rule = ConditionalColorRule::AutoGradient {
            colors: vec!["white,alias".to_owned(), "red,alias".to_owned()],
            steps: 2,
        };
        let start = rule
            .color_for("0", "0", Some(0.0), Some((0.0, 1.0)))
            .expect("start");
        let end = rule
            .color_for("1", "1", Some(1.0), Some((0.0, 1.0)))
            .expect("end");

        assert_eq!(
            theme.conditional_style(&start).expect("start").fg,
            Some(Color::Rgb(255, 255, 255))
        );
        assert_eq!(
            theme.conditional_style(&end).expect("end").fg,
            Some(Color::Rgb(255, 0, 0))
        );
    }

    #[test]
    fn sample_theme_fixture_parses() {
        let theme = parse_theme_yaml(
            include_str!("../examples/data/config/themes/cmdzro.yml"),
            TerminalColorMode::TrueColor,
        )
        .expect("sample theme");

        assert_eq!(theme.name(), "cmdzro");
        assert_eq!(
            theme.style("search.highlight").fg,
            Some(Color::Rgb(255, 255, 0))
        );
        assert_eq!(theme.style("search.highlight").bg, None);
        assert!(theme
            .style("search.highlight")
            .add_modifier
            .contains(Modifier::UNDERLINED));
    }
}
