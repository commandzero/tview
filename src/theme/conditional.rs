use std::collections::BTreeMap;

use ratatui::style::Color;

use crate::ops::sort::{parse_bool_key, parse_numeric_scalar, NumericColumnProfile};

use super::{
    color_for_terminal, dark_identifier_rgb, interpolate_rgb, resolved_color_rgb,
    ConditionalColorRule, ConditionalValue, GradientStop, IdentifierColors, MatchEntry, RangeEntry,
    ResolvedColor, ResolvedTheme, DEFAULT_IDENTIFIER_COLORS, IDENTIFIER_SHADES,
};

/// The row domain supplied by presentation preparation, never selected by the evaluator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColorProfileScope {
    CompleteResult,
    EmittedPreview,
}

/// Facts derived from the applicable result or frozen emitted-row domain.
#[derive(Debug, Clone)]
pub(crate) struct ColumnColorProfile {
    pub(crate) scope: ColorProfileScope,
    pub(crate) numeric_profile: NumericColumnProfile,
    pub(crate) numeric_min_max: Option<(f64, f64)>,
    /// Sorted unique nonempty keys in the provider's interpretation domain.
    pub(crate) identifier_indexes: BTreeMap<String, usize>,
}

/// The only store/emitted facts requested by a column's configured rules.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ColorProfileDemand {
    pub(crate) extrema: bool,
    pub(crate) identifiers: bool,
}

impl ColorProfileDemand {
    pub(crate) fn for_rules(rules: &[ConditionalColorRule]) -> Self {
        let mut demand = Self::default();
        for rule in rules {
            match rule {
                ConditionalColorRule::AutoGradient { .. } => demand.extrema = true,
                ConditionalColorRule::Identifiers { .. } => demand.identifiers = true,
                _ => {}
            }
        }
        demand
    }

    pub(crate) fn needs_profile(self) -> bool {
        self.extrema || self.identifiers
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledColumnColors {
    rules: Vec<CompiledRule>,
    profile: ColumnColorProfile,
}

#[derive(Debug, Clone)]
enum CompiledRule {
    Match(Vec<(ConditionalValue, Option<Color>)>),
    Range(Vec<(RangePredicate, Option<Color>)>),
    Fixed(Vec<(f64, Option<Color>)>),
    Auto {
        constant: Option<Color>,
        buckets: Vec<Option<Color>>,
    },
    Identifiers(Vec<Option<Color>>),
}

#[derive(Debug, Clone, Copy)]
struct RangePredicate {
    lt: Option<f64>,
    lte: Option<f64>,
    gt: Option<f64>,
    gte: Option<f64>,
}

impl From<&RangeEntry> for RangePredicate {
    fn from(entry: &RangeEntry) -> Self {
        Self {
            lt: entry.lt,
            lte: entry.lte,
            gt: entry.gt,
            gte: entry.gte,
        }
    }
}

fn resolve_once(
    theme: &ResolvedTheme,
    colors: &mut BTreeMap<String, Option<ResolvedColor>>,
    reference: &str,
) -> Option<ResolvedColor> {
    if let Some(color) = colors.get(reference) {
        return *color;
    }
    let resolved = theme.resolved_color_ref(reference).ok();
    colors.insert(reference.to_owned(), resolved);
    resolved
}

fn direct_color(
    theme: &ResolvedTheme,
    colors: &mut BTreeMap<String, Option<ResolvedColor>>,
    reference: &str,
) -> Option<Color> {
    resolve_once(theme, colors, reference)
        .map(|resolved| color_for_terminal(resolved, theme.resolved_mode))
}

fn generated_color(
    theme: &ResolvedTheme,
    colors: &mut BTreeMap<String, Option<ResolvedColor>>,
    reference: &str,
) -> Option<(u8, u8, u8)> {
    resolve_once(theme, colors, reference).map(resolved_color_rgb)
}

impl CompiledColumnColors {
    pub(crate) fn prepare(
        theme: &ResolvedTheme,
        configured: &[ConditionalColorRule],
        profile: ColumnColorProfile,
    ) -> Self {
        let mut resolved = BTreeMap::new();
        let rules = configured
            .iter()
            .map(|rule| match rule {
                ConditionalColorRule::Match { entries } => CompiledRule::Match(
                    entries
                        .iter()
                        .map(|MatchEntry { value, color }| {
                            (value.clone(), direct_color(theme, &mut resolved, color))
                        })
                        .collect(),
                ),
                ConditionalColorRule::Range { entries } => CompiledRule::Range(
                    entries
                        .iter()
                        .map(|entry| {
                            (
                                RangePredicate::from(entry),
                                direct_color(theme, &mut resolved, &entry.color),
                            )
                        })
                        .collect(),
                ),
                ConditionalColorRule::FixedGradient { stops } => CompiledRule::Fixed(
                    stops
                        .iter()
                        .map(|GradientStop { value, color }| {
                            (*value, direct_color(theme, &mut resolved, color))
                        })
                        .collect(),
                ),
                ConditionalColorRule::AutoGradient { colors, steps } => {
                    let steps = (*steps).max(1);
                    let stops = colors
                        .iter()
                        .map(|color| generated_color(theme, &mut resolved, color))
                        .collect::<Vec<_>>();
                    let buckets = if stops.is_empty() {
                        Vec::new()
                    } else {
                        (0..steps)
                            .map(|bucket| {
                                gradient_bucket(&stops, bucket, steps).map(|rgb| {
                                    color_for_terminal(
                                        ResolvedColor::Rgb {
                                            r: rgb.0,
                                            g: rgb.1,
                                            b: rgb.2,
                                            a: 255,
                                        },
                                        theme.resolved_mode,
                                    )
                                })
                            })
                            .collect()
                    };
                    CompiledRule::Auto {
                        constant: colors
                            .first()
                            .and_then(|color| direct_color(theme, &mut resolved, color)),
                        buckets,
                    }
                }
                ConditionalColorRule::Identifiers { colors } => {
                    let configured = match colors {
                        IdentifierColors::Auto => theme.identifier_colors.as_slice(),
                        IdentifierColors::Colors(colors) => colors.as_slice(),
                    };
                    let families = if configured.is_empty() {
                        DEFAULT_IDENTIFIER_COLORS.to_vec()
                    } else {
                        configured.iter().map(String::as_str).collect()
                    };
                    let targets = families
                        .into_iter()
                        .map(|color| generated_color(theme, &mut resolved, color))
                        .collect::<Vec<_>>();
                    let mut cycle = Vec::with_capacity(targets.len() * IDENTIFIER_SHADES);
                    for shade in 0..IDENTIFIER_SHADES {
                        let ratio = shade as f64 / (IDENTIFIER_SHADES - 1) as f64;
                        for target in &targets {
                            cycle.push(target.map(|target| {
                                let rgb =
                                    interpolate_rgb(dark_identifier_rgb(target), target, ratio);
                                color_for_terminal(
                                    ResolvedColor::Rgb {
                                        r: rgb.0,
                                        g: rgb.1,
                                        b: rgb.2,
                                        a: 255,
                                    },
                                    theme.resolved_mode,
                                )
                            }));
                        }
                    }
                    CompiledRule::Identifiers(cycle)
                }
            })
            .collect();
        Self { rules, profile }
    }

    /// Evaluate once from raw source and already-rendered presentation text.
    pub(crate) fn evaluate(&self, raw: &str, rendered: &str) -> Option<Color> {
        let mut numeric = None;
        let mut scalar = || {
            *numeric.get_or_insert_with(|| parse_numeric_scalar(raw, self.profile.numeric_profile))
        };
        for rule in &self.rules {
            let selected = match rule {
                CompiledRule::Match(entries) => entries
                    .iter()
                    .find(|(value, _)| value_matches(value, raw, rendered, &mut scalar))
                    .map(|(_, color)| *color),
                CompiledRule::Range(entries) => scalar().and_then(|number| {
                    entries
                        .iter()
                        .find(|(entry, _)| range_matches(entry, number))
                        .map(|(_, color)| *color)
                }),
                CompiledRule::Fixed(stops) => scalar().and_then(|number| {
                    stops
                        .iter()
                        .enumerate()
                        .find(|(index, (stop, _))| {
                            number >= *stop
                                && stops.get(index + 1).is_none_or(|(next, _)| number < *next)
                        })
                        .map(|(_, (_, color))| *color)
                }),
                CompiledRule::Auto { constant, buckets } => scalar().and_then(|number| {
                    let (min, max) = self.profile.numeric_min_max?;
                    if buckets.is_empty() {
                        return None;
                    }
                    if max <= min {
                        return Some(*constant);
                    }
                    let steps = buckets.len();
                    let ratio = ((number - min) / (max - min)).clamp(0.0, 1.0);
                    let bucket = (ratio * steps as f64).floor().min((steps - 1) as f64) as usize;
                    buckets.get(bucket).copied()
                }),
                CompiledRule::Identifiers(cycle) => {
                    if rendered.is_empty() || cycle.is_empty() {
                        None
                    } else {
                        self.profile
                            .identifier_indexes
                            .get(rendered)
                            .map(|index| cycle[*index % cycle.len()])
                    }
                }
            };
            if let Some(color) = selected {
                // A matching predicate with an unavailable foreground still wins precedence.
                return color;
            }
        }
        None
    }

    pub(crate) fn scope(&self) -> ColorProfileScope {
        self.profile.scope
    }
}

fn gradient_bucket(
    stops: &[Option<(u8, u8, u8)>],
    bucket: usize,
    steps: usize,
) -> Option<(u8, u8, u8)> {
    if stops.is_empty() {
        return None;
    }
    if stops.len() == 1 || steps <= 1 {
        return stops[0];
    }
    let position = bucket.min(steps - 1) as f64 / (steps - 1) as f64;
    let scaled = position * (stops.len() - 1) as f64;
    let left = stops[scaled.floor() as usize]?;
    let right = stops[scaled.ceil() as usize]?;
    Some(interpolate_rgb(left, right, scaled - scaled.floor()))
}

fn value_matches(
    value: &ConditionalValue,
    raw: &str,
    rendered: &str,
    scalar: &mut impl FnMut() -> Option<f64>,
) -> bool {
    match value {
        ConditionalValue::Bool(expected) => {
            parse_bool_key(raw) == Some(*expected) || parse_bool_key(rendered) == Some(*expected)
        }
        ConditionalValue::Number(expected) => scalar() == Some(*expected),
        ConditionalValue::String(expected) => {
            raw.eq_ignore_ascii_case(expected) || rendered == expected
        }
    }
}

fn range_matches(entry: &RangePredicate, number: f64) -> bool {
    entry.lt.is_none_or(|bound| number < bound)
        && entry.lte.is_none_or(|bound| number <= bound)
        && entry.gt.is_none_or(|bound| number > bound)
        && entry.gte.is_none_or(|bound| number >= bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{default_theme_for_terminal, parse_theme_yaml, TerminalColorMode};

    fn profile(min_max: Option<(f64, f64)>, keys: &[&str]) -> ColumnColorProfile {
        ColumnColorProfile {
            scope: ColorProfileScope::EmittedPreview,
            numeric_profile: NumericColumnProfile::default(),
            numeric_min_max: min_max,
            identifier_indexes: keys
                .iter()
                .enumerate()
                .map(|(index, key)| ((*key).to_owned(), index))
                .collect(),
        }
    }

    fn evaluate(
        theme: &ResolvedTheme,
        rules: &[ConditionalColorRule],
        profile: ColumnColorProfile,
        raw: &str,
        rendered: &str,
    ) -> Option<Color> {
        CompiledColumnColors::prepare(theme, rules, profile).evaluate(raw, rendered)
    }

    fn match_rule(entries: &[(&str, ConditionalValue)]) -> ConditionalColorRule {
        ConditionalColorRule::Match {
            entries: entries
                .iter()
                .map(|(color, value)| MatchEntry {
                    value: value.clone(),
                    color: (*color).to_owned(),
                })
                .collect(),
        }
    }

    fn color_mode(mode: TerminalColorMode) -> ResolvedTheme {
        let mut document: yaml_serde::Value =
            yaml_serde::from_str(include_str!("../../examples/data/config/themes/cmdzro.yml"))
                .expect("theme fixture");
        document["name"] = "color-test".into();
        document["mode"] = "auto".into();
        document["palette"]["a,b;c:(é)"] = "teal_chain".into();
        document["palette"]["teal_chain"] = "#25A39AFF".into();
        let yaml = yaml_serde::to_string(&document).expect("theme fixture YAML");
        parse_theme_yaml(&yaml, mode).expect("color test theme")
    }

    #[test]
    fn direct_colors_preserve_terminal_semantics_and_exact_alias_precedence() {
        let expected = [
            (
                TerminalColorMode::Ansi16,
                [Color::Green, Color::Indexed(1), Color::Indexed(6)],
            ),
            (
                TerminalColorMode::Ansi256,
                [Color::Indexed(46), Color::Indexed(124), Color::Indexed(36)],
            ),
            (
                TerminalColorMode::TrueColor,
                [
                    Color::Rgb(0, 255, 0),
                    Color::Indexed(124),
                    Color::Rgb(37, 163, 154),
                ],
            ),
        ];
        for (mode, colors) in expected {
            let theme = color_mode(mode);
            for (index, color) in ["bright-green", "palette(124)", "#25A39AFF"]
                .into_iter()
                .enumerate()
            {
                let rule = match_rule(&[(color, ConditionalValue::String("hit".to_owned()))]);
                assert_eq!(
                    evaluate(&theme, &[rule], profile(None, &[]), "hit", "hit"),
                    Some(colors[index])
                );
            }
            let alias = match_rule(&[("a,b;c:(é)", ConditionalValue::String("hit".to_owned()))]);
            assert_eq!(
                evaluate(&theme, &[alias], profile(None, &[]), "hit", "hit"),
                Some(colors[2])
            );
            let shadow = match_rule(&[("green", ConditionalValue::String("hit".to_owned()))]);
            assert_eq!(
                evaluate(&theme, &[shadow], profile(None, &[]), "hit", "hit"),
                theme.resolve_color_ref("dark-green").ok()
            );
        }
    }

    #[test]
    fn complete_unicode_palette_alias_survives_each_color_rule_kind() {
        let theme = color_mode(TerminalColorMode::TrueColor);
        let alias = "a,b;c:(é)";
        let expected = Some(Color::Rgb(37, 163, 154));
        let rules = [
            ConditionalColorRule::Range {
                entries: vec![RangeEntry {
                    lt: None,
                    lte: None,
                    gt: None,
                    gte: Some(1.0),
                    color: alias.to_owned(),
                }],
            },
            ConditionalColorRule::FixedGradient {
                stops: vec![GradientStop {
                    value: 1.0,
                    color: alias.to_owned(),
                }],
            },
            ConditionalColorRule::AutoGradient {
                colors: vec![alias.to_owned()],
                steps: 3,
            },
            ConditionalColorRule::Identifiers {
                colors: IdentifierColors::Colors(vec![alias.to_owned()]),
            },
        ];
        for (rule, raw, rendered, extrema, keys) in [
            (&rules[0], "1", "1", None, &[][..]),
            (&rules[1], "1", "1", None, &[][..]),
            (&rules[2], "1", "1", Some((1.0, 2.0)), &[][..]),
        ] {
            assert_eq!(
                evaluate(
                    &theme,
                    std::slice::from_ref(rule),
                    profile(extrema, keys),
                    raw,
                    rendered
                ),
                expected
            );
        }
        let identifier = evaluate(
            &theme,
            &[rules[3].clone()],
            profile(None, &["id"]),
            "id",
            "id",
        );
        assert_eq!(identifier, Some(Color::Rgb(19, 100, 100)));
    }

    #[test]
    fn entry_order_and_unavailable_selected_colors_do_not_fall_through() {
        let theme = default_theme_for_terminal(TerminalColorMode::TrueColor);
        let first = match_rule(&[
            ("missing-alias", ConditionalValue::Bool(true)),
            ("red", ConditionalValue::String("yes".to_owned())),
        ]);
        let fallback = match_rule(&[("yellow", ConditionalValue::String("yes".to_owned()))]);
        assert_eq!(
            evaluate(
                &theme,
                &[first.clone(), fallback.clone()],
                profile(None, &[]),
                "YES",
                "yes"
            ),
            None
        );
        assert_eq!(
            evaluate(
                &theme,
                &[fallback.clone(), first],
                profile(None, &[]),
                "YES",
                "yes"
            ),
            theme.resolve_color_ref("yellow").ok()
        );
        assert_eq!(
            evaluate(&theme, &[fallback], profile(None, &[]), "no", "no"),
            None
        );
        let numeric_quoted = match_rule(&[
            ("red", ConditionalValue::String("123".to_owned())),
            ("green", ConditionalValue::Number(123.0)),
        ]);
        assert_eq!(
            evaluate(&theme, &[numeric_quoted], profile(None, &[]), "123", "123"),
            theme.resolve_color_ref("red").ok()
        );
    }

    #[test]
    fn unavailable_range_fixed_gradient_or_family_retains_selected_rule() {
        let theme = default_theme_for_terminal(TerminalColorMode::TrueColor);
        let range = ConditionalColorRule::Range {
            entries: vec![RangeEntry {
                lt: None,
                lte: None,
                gt: None,
                gte: Some(1.0),
                color: "missing-alias".to_owned(),
            }],
        };
        let fixed = ConditionalColorRule::FixedGradient {
            stops: vec![GradientStop {
                value: 1.0,
                color: "missing-alias".to_owned(),
            }],
        };
        let auto = ConditionalColorRule::AutoGradient {
            colors: vec!["missing-alias".to_owned(), "green".to_owned()],
            steps: 2,
        };
        let family = ConditionalColorRule::Identifiers {
            colors: IdentifierColors::Colors(vec!["missing-alias".to_owned()]),
        };
        let fallback = match_rule(&[("green", ConditionalValue::String("1".to_owned()))]);
        for (rule, scoped) in [
            (range, profile(None, &[])),
            (fixed, profile(None, &[])),
            (auto, profile(Some((1.0, 2.0)), &[])),
            (family, profile(None, &["1"])),
        ] {
            assert_eq!(
                evaluate(&theme, &[rule, fallback.clone()], scoped, "1", "1"),
                None
            );
        }
    }

    #[test]
    fn range_and_fixed_stops_preserve_first_match_and_stop_boundaries() {
        let theme = default_theme_for_terminal(TerminalColorMode::TrueColor);
        let range = ConditionalColorRule::Range {
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
                    lte: Some(20.0),
                    gt: None,
                    gte: Some(5.0),
                    color: "green".to_owned(),
                },
            ],
        };
        let fixed = ConditionalColorRule::FixedGradient {
            stops: [(0.0, "green"), (10.0, "yellow"), (20.0, "red")]
                .into_iter()
                .map(|(value, color)| GradientStop {
                    value,
                    color: color.to_owned(),
                })
                .collect(),
        };
        for (raw, expected) in [
            ("-1", None),
            ("0", theme.resolve_color_ref("green").ok()),
            ("9", theme.resolve_color_ref("green").ok()),
            ("10", theme.resolve_color_ref("yellow").ok()),
            ("19", theme.resolve_color_ref("yellow").ok()),
            ("20", theme.resolve_color_ref("red").ok()),
        ] {
            assert_eq!(
                evaluate(
                    &theme,
                    std::slice::from_ref(&fixed),
                    profile(None, &[]),
                    raw,
                    raw
                ),
                expected
            );
        }
        assert_eq!(
            evaluate(
                &theme,
                &[range.clone(), fixed],
                profile(None, &[]),
                "6",
                "6"
            ),
            theme.resolve_color_ref("red").ok()
        );
        assert_eq!(
            evaluate(&theme, &[range], profile(None, &[]), "10", "10"),
            theme.resolve_color_ref("green").ok()
        );
    }

    #[test]
    fn automatic_gradient_respects_bucket_boundaries_constant_and_partial_resolution() {
        let theme = default_theme_for_terminal(TerminalColorMode::TrueColor);
        let gradient = ConditionalColorRule::AutoGradient {
            colors: vec!["#000000FF".to_owned(), "#FFFFFF00".to_owned()],
            steps: 4,
        };
        let state =
            CompiledColumnColors::prepare(&theme, &[gradient], profile(Some((0.0, 100.0)), &[]));
        for (raw, expected) in [
            ("0", Color::Rgb(0, 0, 0)),
            ("24.99", Color::Rgb(0, 0, 0)),
            ("25", Color::Rgb(85, 85, 85)),
            ("50", Color::Rgb(170, 170, 170)),
            ("75", Color::Rgb(255, 255, 255)),
            ("100", Color::Rgb(255, 255, 255)),
        ] {
            assert_eq!(state.evaluate(raw, raw), Some(expected));
        }
        let one = ConditionalColorRule::AutoGradient {
            colors: vec!["#25A39AFF".to_owned(), "red".to_owned()],
            steps: 1,
        };
        assert_eq!(
            evaluate(&theme, &[one], profile(Some((0.0, 1.0)), &[]), "1", "1"),
            Some(Color::Rgb(37, 163, 154))
        );
        let direct = ConditionalColorRule::AutoGradient {
            colors: vec!["palette(124)".to_owned(), "red".to_owned()],
            steps: 4,
        };
        let limited = default_theme_for_terminal(TerminalColorMode::Ansi256);
        assert_eq!(
            evaluate(
                &limited,
                &[direct],
                profile(Some((5.0, 5.0)), &[]),
                "5",
                "5"
            ),
            Some(Color::Indexed(124))
        );
        let one_step_indexed = ConditionalColorRule::AutoGradient {
            colors: vec!["palette(1)".to_owned(), "red".to_owned()],
            steps: 1,
        };
        assert_eq!(
            evaluate(
                &limited,
                std::slice::from_ref(&one_step_indexed),
                profile(Some((5.0, 5.0)), &[]),
                "5",
                "5",
            ),
            Some(Color::Indexed(1))
        );
        assert_eq!(
            evaluate(
                &limited,
                &[one_step_indexed],
                profile(Some((0.0, 10.0)), &[]),
                "5",
                "5",
            ),
            Some(Color::Indexed(124))
        );
        let partial = ConditionalColorRule::AutoGradient {
            colors: vec![
                "red".to_owned(),
                "missing-alias".to_owned(),
                "blue".to_owned(),
            ],
            steps: 5,
        };
        let state =
            CompiledColumnColors::prepare(&theme, &[partial], profile(Some((0.0, 100.0)), &[]));
        assert!(state.evaluate("0", "0").is_some());
        assert_eq!(state.evaluate("25", "25"), None);
        assert!(state.evaluate("100", "100").is_some());
        assert_eq!(
            evaluate(
                &theme,
                &[ConditionalColorRule::AutoGradient {
                    colors: vec!["red".to_owned(), "blue".to_owned()],
                    steps: 4,
                }],
                profile(None, &[]),
                "bad",
                "bad"
            ),
            None
        );
    }

    #[test]
    fn numeric_rules_use_raw_percent_byte_time_and_scientific_scalars() {
        let theme = default_theme_for_terminal(TerminalColorMode::TrueColor);
        let cases = [
            ("50%", 50.0, NumericColumnProfile::default()),
            ("2KB", 2_000.0, NumericColumnProfile::default()),
            ("2m", 120.0, NumericColumnProfile::time()),
            ("2M", 2_000_000.0, NumericColumnProfile::default()),
            ("2e3", 2_000.0, NumericColumnProfile::default()),
            ("2e3KiB", 2_048_000.0, NumericColumnProfile::default()),
        ];
        for (raw, scalar, numeric_profile) in cases {
            let rule = ConditionalColorRule::Range {
                entries: vec![RangeEntry {
                    lt: None,
                    lte: Some(scalar),
                    gt: None,
                    gte: Some(scalar),
                    color: "yellow".to_owned(),
                }],
            };
            let mut scoped = profile(None, &[]);
            scoped.numeric_profile = numeric_profile;
            assert_eq!(
                evaluate(&theme, &[rule], scoped, raw, "rounded presentation"),
                theme.resolve_color_ref("yellow").ok(),
                "{raw}"
            );
        }
    }

    #[test]
    fn profile_scope_facts_change_gradients_and_sorted_identifier_assignments() {
        let theme = default_theme_for_terminal(TerminalColorMode::TrueColor);
        let gradient = ConditionalColorRule::AutoGradient {
            colors: vec!["#000000FF".to_owned(), "#FFFFFFFF".to_owned()],
            steps: 4,
        };
        let emitted = CompiledColumnColors::prepare(
            &theme,
            std::slice::from_ref(&gradient),
            profile(Some((1.0, 2.0)), &[]),
        );
        let mut complete_profile = profile(Some((1.0, 99_999_999.0)), &[]);
        complete_profile.scope = ColorProfileScope::CompleteResult;
        let complete = CompiledColumnColors::prepare(&theme, &[gradient], complete_profile);
        assert_eq!(emitted.evaluate("2", "2"), Some(Color::Rgb(255, 255, 255)));
        assert_eq!(complete.evaluate("2", "2"), Some(Color::Rgb(0, 0, 0)));

        let identifiers = ConditionalColorRule::Identifiers {
            colors: IdentifierColors::Colors(vec!["red".to_owned(), "blue".to_owned()]),
        };
        let emitted = CompiledColumnColors::prepare(
            &theme,
            std::slice::from_ref(&identifiers),
            profile(None, &["beta", "gamma"]),
        );
        let mut complete_profile = profile(None, &["alpha", "beta", "gamma"]);
        complete_profile.scope = ColorProfileScope::CompleteResult;
        let complete = CompiledColumnColors::prepare(&theme, &[identifiers], complete_profile);
        assert_ne!(
            emitted.evaluate("beta", "beta"),
            complete.evaluate("beta", "beta")
        );
        assert_eq!(
            emitted.evaluate("different source", "beta"),
            emitted.evaluate("beta", "beta")
        );
    }

    #[test]
    fn identifier_family_first_shades_repeat_and_missing_family_only_affects_its_turn() {
        let theme = default_theme_for_terminal(TerminalColorMode::TrueColor);
        let keys = (0..34)
            .map(|index| format!("key{index:02}"))
            .collect::<Vec<_>>();
        let indexes = keys.iter().map(String::as_str).collect::<Vec<_>>();
        let explicit = ConditionalColorRule::Identifiers {
            colors: IdentifierColors::Colors(vec![
                "#FF0000FF".to_owned(),
                "missing-alias".to_owned(),
            ]),
        };
        let prepared = CompiledColumnColors::prepare(&theme, &[explicit], profile(None, &indexes));
        assert_eq!(
            prepared.evaluate("raw", "key00"),
            Some(Color::Rgb(128, 0, 0))
        );
        assert_eq!(prepared.evaluate("raw", "key01"), None);
        assert_eq!(
            prepared.evaluate("raw", "key02"),
            Some(Color::Rgb(136, 0, 0))
        );
        assert_eq!(
            prepared.evaluate("raw", "key32"),
            Some(Color::Rgb(128, 0, 0))
        );
        assert_eq!(prepared.evaluate("raw", ""), None);
        assert_eq!(prepared.evaluate("raw", "unprofiled"), None);
        let automatic = ConditionalColorRule::Identifiers {
            colors: IdentifierColors::Auto,
        };
        assert!(evaluate(
            &theme,
            &[automatic],
            profile(None, &indexes),
            "raw",
            "key00"
        )
        .is_some());
    }
}
