use super::{ColumnAlignment, ColumnTypeMetadata, DisplayFormatMetadata, TableView};
use crate::ops::filter::{FilterKind, FilterMode};
use crate::ops::sort::{SortDirection, SortMode};
use crate::table::NullPlacement;
use crate::theme::{ConditionalColorRule, ConditionalValue};

impl TableView {
    pub fn to_saved_view_yaml(
        &self,
        name: &str,
        input_filename: &str,
        locale: Option<&str>,
    ) -> String {
        let mut yaml = String::new();
        yaml.push_str(&format!("name: {}\n", yaml_scalar(name)));
        yaml.push_str("filenames:\n");
        yaml.push_str(&format!("  - {}\n", yaml_scalar(input_filename)));

        let mut source_yaml = String::new();
        self.write_source_yaml(&mut source_yaml);
        write_mapping(&mut yaml, "source", &source_yaml);

        let mut view_yaml = String::new();
        self.write_view_options(&mut view_yaml, locale);
        self.write_columns(&mut view_yaml);
        self.write_view_sorts(&mut view_yaml);
        self.write_view_filters(&mut view_yaml);
        write_mapping(&mut yaml, "view", &view_yaml);
        yaml
    }

    fn write_source_yaml(&self, source_yaml: &mut String) {
        if let Some(options) = self.committed_source.as_ref() {
            self.write_source_options(source_yaml, options);
        }
        if let Some(object_mode) = self.object_mode {
            source_yaml.push_str(&format!("object_mode: {}\n", object_mode.resolved));
        }
    }

    fn write_source_options(&self, source_yaml: &mut String, options: &crate::ingest::OpenOptions) {
        self.write_source_format(source_yaml, options.format);
        if let Some(json_path) = &options.json_path {
            source_yaml.push_str(&format!("json_path: {}\n", yaml_scalar(json_path.as_str())));
        }
        if options.schema_scan == crate::ingest::SchemaScan::Full {
            source_yaml.push_str("schema_scan: full\n");
        }
        if let Some(query) = &options.native_query {
            source_yaml.push_str(&format!("query: {}\n", yaml_scalar(query)));
        } else if let Some(table) = &options.table {
            source_yaml.push_str(&format!("table: {}\n", yaml_scalar(table)));
        }
        if let Some(limit) = options.limit {
            source_yaml.push_str(&format!("limit: {}\n", limit));
        }
        write_source_filters(source_yaml, &options.source_filters);
        write_source_sorts(source_yaml, &options.source_sort);
    }

    fn write_source_format(&self, source_yaml: &mut String, format: crate::ingest::InputFormat) {
        if format != crate::ingest::InputFormat::Auto {
            source_yaml.push_str(&format!("format: {}\n", format));
        } else if let Some(provenance) = &self.source_query_provenance {
            source_yaml.push_str(&format!(
                "format: {}\n",
                match provenance.language {
                    crate::table::NativeQueryLanguage::Sql => "sqlite",
                    crate::table::NativeQueryLanguage::Esql => "elasticsearch",
                }
            ));
        }
    }

    fn write_view_options(&self, view_yaml: &mut String, locale: Option<&str>) {
        if let Some(locale) = locale {
            view_yaml.push_str(&format!("locale: {}\n", yaml_scalar(locale)));
        }
        if let Some(nulls) = self.view_nulls {
            view_yaml.push_str(&format!("nulls: {}\n", null_placement_name(nulls)));
        }
    }

    fn write_columns(&self, view_yaml: &mut String) {
        let mut columns = (0..self.source_column_count())
            .filter(|&column| self.should_save_column(column))
            .peekable();
        if columns.peek().is_none() {
            return;
        }
        view_yaml.push_str("columns:\n");
        for source_column in columns {
            view_yaml.push_str(&self.column_yaml(source_column));
        }
    }

    fn should_save_column(&self, source_column: usize) -> bool {
        self.column_width_modified.contains(&source_column)
            || self.hidden_columns.contains(&source_column)
            || self
                .column_alignment_overrides
                .get(source_column)
                .is_some_and(Option::is_some)
            || self.column_metadata_modified.contains(&source_column)
            || self
                .column_nulls
                .get(source_column)
                .is_some_and(Option::is_some)
            || self
                .column_color_rules
                .get(source_column)
                .is_some_and(|rules| !rules.is_empty())
    }

    fn column_yaml(&self, source_column: usize) -> String {
        let key = self.source_column_name(source_column);
        let mut block = format!("  {}:\n", yaml_key(&key));
        if let Some(label) = self
            .column_label_overrides
            .get(source_column)
            .and_then(Option::as_ref)
        {
            block.push_str(&format!("    label: {}\n", yaml_scalar(label)));
        }
        if let Some(nulls) = self.column_nulls.get(source_column).copied().flatten() {
            block.push_str(&format!("    nulls: {}\n", null_placement_name(nulls)));
        }
        self.write_column_metadata(&mut block, source_column);
        if self.column_width_modified.contains(&source_column) {
            let width = self
                .column_widths
                .get(source_column)
                .copied()
                .unwrap_or(1)
                .max(1);
            block.push_str(&format!("    width: {width}\n"));
        }
        if self.hidden_columns.contains(&source_column) {
            block.push_str("    visible: false\n");
        }
        if let Some(alignment) = self
            .column_alignment_overrides
            .get(source_column)
            .copied()
            .flatten()
        {
            block.push_str(&format!("    align: {}\n", alignment_name(alignment)));
        }
        self.write_column_colors(&mut block, source_column);
        block
    }

    fn write_column_metadata(&self, block: &mut String, source_column: usize) {
        if self.column_metadata_modified.contains(&source_column) {
            let metadata = self
                .column_display
                .get(source_column)
                .copied()
                .unwrap_or_default();
            if metadata.column_type != ColumnTypeMetadata::Text {
                block.push_str(&format!(
                    "    type: {}\n",
                    column_type_metadata_name(metadata.column_type)
                ));
            }
            if metadata.format != DisplayFormatMetadata::Plain {
                block.push_str(&format!(
                    "    format: {}\n",
                    display_format_metadata_name(metadata.format)
                ));
            }
            if let Some(mask) = metadata.mask {
                block.push_str(&format!("    mask: {}\n", yaml_scalar(&mask.to_mask())));
            }
        }
    }

    fn write_column_colors(&self, block: &mut String, source_column: usize) {
        let Some(rules) = self
            .column_color_rules
            .get(source_column)
            .filter(|rules| !rules.is_empty())
        else {
            return;
        };
        block.push_str("    colors:\n");
        for rule in rules {
            push_color_rule_yaml(block, rule);
        }
    }

    fn write_view_sorts(&self, view_yaml: &mut String) {
        if !self.sort_keys.is_empty() {
            view_yaml.push_str("sort:\n");
            for key in &self.sort_keys {
                view_yaml.push_str(&format!(
                    "  - column: {}\n",
                    yaml_scalar(&self.source_column_name(key.column))
                ));
                view_yaml.push_str(&format!(
                    "    direction: {}\n",
                    match key.direction {
                        SortDirection::Ascending => "asc",
                        SortDirection::Descending => "desc",
                    }
                ));
                view_yaml.push_str(&format!("    kind: {}\n", sort_mode_name(key.mode)));
            }
        }
    }

    fn write_view_filters(&self, view_yaml: &mut String) {
        if !self.filters.is_empty() {
            view_yaml.push_str("filters:\n");
            for filter in &self.filters {
                view_yaml.push_str(&format!(
                    "  - column: {}\n",
                    yaml_scalar(&self.source_column_name(filter.column))
                ));
                view_yaml.push_str(&format!(
                    "    action: {}\n",
                    match filter.mode {
                        FilterMode::In => "in",
                        FilterMode::Out => "out",
                    }
                ));
                view_yaml.push_str(&format!("    kind: {}\n", filter_kind_name(filter.kind)));
                view_yaml.push_str(&format!("    condition: {}\n", yaml_scalar(&filter.input)));
            }
        }
    }
}

fn null_placement_name(nulls: NullPlacement) -> &'static str {
    match nulls {
        NullPlacement::First => "first",
        NullPlacement::Last => "last",
    }
}

fn alignment_name(alignment: ColumnAlignment) -> &'static str {
    match alignment {
        ColumnAlignment::Left => "left",
        ColumnAlignment::Right => "right",
    }
}

fn write_mapping(yaml: &mut String, key: &str, contents: &str) {
    if contents.is_empty() {
        yaml.push_str(&format!("{key}: {{}}\n"));
    } else {
        yaml.push_str(&format!("{key}:\n"));
        push_indented_yaml(yaml, contents, 2);
    }
}

fn write_source_filters(source_yaml: &mut String, filters: &[crate::ingest::SourceFilterRequest]) {
    if !filters.is_empty() {
        source_yaml.push_str("filters:\n");
        for filter in filters {
            source_yaml.push_str(&format!("  - column: {}\n", yaml_scalar(&filter.column)));
            source_yaml.push_str(&format!(
                "    operator: {}\n",
                source_filter_operator_name(filter.operator)
            ));
            if let Some(value) = &filter.operand {
                source_yaml.push_str(&format!("    value: {}\n", source_operand_yaml(value)));
            }
        }
    }
}

fn write_source_sorts(source_yaml: &mut String, sorts: &[crate::ingest::SourceSortRequest]) {
    if !sorts.is_empty() {
        source_yaml.push_str("sort:\n");
        for sort in sorts {
            source_yaml.push_str(&format!("  - column: {}\n", yaml_scalar(&sort.column)));
            source_yaml.push_str(&format!(
                "    direction: {}\n",
                match sort.direction {
                    crate::table::SortDirection::Ascending => "asc",
                    crate::table::SortDirection::Descending => "desc",
                }
            ));
        }
    }
}

fn column_type_metadata_name(column_type: ColumnTypeMetadata) -> &'static str {
    match column_type {
        ColumnTypeMetadata::Text => "text",
        ColumnTypeMetadata::Date => "date",
        ColumnTypeMetadata::Ip => "ip",
        ColumnTypeMetadata::Float => "float",
        ColumnTypeMetadata::Int => "integer",
        ColumnTypeMetadata::SemVer => "semver",
        ColumnTypeMetadata::BooleanWord => "word",
        ColumnTypeMetadata::BooleanChar => "char",
        ColumnTypeMetadata::BooleanBit => "bit",
    }
}

fn display_format_metadata_name(format: DisplayFormatMetadata) -> &'static str {
    match format {
        DisplayFormatMetadata::Plain => "plain",
        DisplayFormatMetadata::Locale => "locale",
        DisplayFormatMetadata::Mask => "mask",
        DisplayFormatMetadata::Uppercase => "uppercase",
        DisplayFormatMetadata::Lowercase => "lowercase",
        DisplayFormatMetadata::BooleanChar => "char",
        DisplayFormatMetadata::BooleanBit => "bit",
        DisplayFormatMetadata::BooleanWord => "word",
    }
}

fn sort_mode_name(mode: SortMode) -> &'static str {
    match mode {
        SortMode::Lexical => "lexical",
        SortMode::Natural => "natural",
        SortMode::Numeric => "numeric",
        SortMode::Date | SortMode::SemVer | SortMode::Ip | SortMode::Boolean => "type",
    }
}

fn filter_kind_name(kind: FilterKind) -> &'static str {
    match kind {
        FilterKind::Text => "text",
        FilterKind::Regex => "regex",
        FilterKind::Numeric => "numeric",
    }
}

fn push_color_rule_yaml(block: &mut String, rule: &ConditionalColorRule) {
    match rule {
        ConditionalColorRule::Match { entries } => {
            block.push_str("      - match:\n");
            for entry in entries {
                block.push_str(&format!(
                    "          {}: {}\n",
                    conditional_value_yaml(&entry.value),
                    yaml_scalar(&entry.color)
                ));
            }
        }
        ConditionalColorRule::Range { entries } => {
            block.push_str("      - range:\n");
            for entry in entries {
                block.push_str(&format!(
                    "          {}: {}\n",
                    yaml_scalar(&range_entry_expression(entry)),
                    yaml_scalar(&entry.color)
                ));
            }
        }
        ConditionalColorRule::FixedGradient { stops } => {
            block.push_str("      - gradient:\n");
            block.push_str("          mode: fixed\n");
            block.push_str("          stops:\n");
            for stop in stops {
                block.push_str(&format!(
                    "            {}: {}\n",
                    yaml_scalar(&stop.value.to_string()),
                    yaml_scalar(&stop.color)
                ));
            }
        }
        ConditionalColorRule::AutoGradient { colors, steps } => {
            block.push_str("      - gradient:\n");
            block.push_str("          mode: auto\n");
            block.push_str(&format!("          steps: {steps}\n"));
            block.push_str("          colors:\n");
            for color in colors {
                block.push_str(&format!("            - {}\n", yaml_scalar(color)));
            }
        }
        ConditionalColorRule::Identifiers { colors } => {
            block.push_str("      - identifiers:\n");
            match colors {
                crate::theme::IdentifierColors::Auto => {
                    block.push_str("          colors: auto\n");
                }
                crate::theme::IdentifierColors::Colors(colors) => {
                    block.push_str("          colors:\n");
                    for color in colors {
                        block.push_str(&format!("            - {}\n", yaml_scalar(color)));
                    }
                }
            }
        }
    }
}

fn range_entry_expression(entry: &crate::theme::RangeEntry) -> String {
    let mut parts = Vec::new();
    if let Some(value) = entry.gte {
        parts.push(format!(">={value}"));
    }
    if let Some(value) = entry.gt {
        parts.push(format!(">{value}"));
    }
    if let Some(value) = entry.lte {
        parts.push(format!("<={value}"));
    }
    if let Some(value) = entry.lt {
        parts.push(format!("<{value}"));
    }
    parts.join(" ")
}

pub(super) fn conditional_value_yaml(value: &ConditionalValue) -> String {
    match value {
        ConditionalValue::Bool(value) => value.to_string(),
        ConditionalValue::Number(value) => value.to_string(),
        ConditionalValue::String(value) => yaml_quoted_scalar(value),
    }
}

fn yaml_key(value: &str) -> String {
    yaml_scalar(value)
}

fn push_indented_yaml(output: &mut String, yaml: &str, spaces: usize) {
    let indent = " ".repeat(spaces);
    for line in yaml.lines() {
        output.push_str(&indent);
        output.push_str(line);
        output.push('\n');
    }
}

fn yaml_scalar(value: &str) -> String {
    let requires_quoting =
        value.is_empty() || matches!(value, "true" | "false" | "yes" | "no" | "null");
    let plain_characters = value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'));
    if !requires_quoting && plain_characters {
        value.to_owned()
    } else {
        yaml_quoted_scalar(value)
    }
}

fn source_filter_operator_name(operator: crate::table::SourceFilterOperator) -> &'static str {
    use crate::table::SourceFilterOperator as Operator;
    match operator {
        Operator::Equal => "equal",
        Operator::NotEqual => "not_equal",
        Operator::LessThan => "less_than",
        Operator::LessThanOrEqual => "less_or_equal",
        Operator::GreaterThan => "greater_than",
        Operator::GreaterThanOrEqual => "greater_or_equal",
        Operator::Contains => "contains",
        Operator::Prefix => "prefix",
        Operator::IsNull => "is_null",
        Operator::IsNotNull => "is_not_null",
    }
}

fn source_operand_yaml(value: &crate::table::SourceOperand) -> String {
    match value {
        crate::table::SourceOperand::Null => "null".to_owned(),
        crate::table::SourceOperand::Boolean(value) => value.to_string(),
        crate::table::SourceOperand::Integer(value) => value.to_string(),
        crate::table::SourceOperand::Float(value) => value.to_string(),
        crate::table::SourceOperand::Text(value) => yaml_scalar(value),
        crate::table::SourceOperand::Binary(value) => yaml_scalar(&String::from_utf8_lossy(value)),
    }
}

fn yaml_quoted_scalar(value: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            ch if ch.is_control() => quoted.push_str(&format!("\\x{:02X}", ch as u32)),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}
