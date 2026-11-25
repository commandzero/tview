use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn write_saved_view_atomic(path: &Path, yaml: &str) -> anyhow::Result<()> {
    use std::fs;
    use std::io::Write;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = if path.exists() {
        let existing = fs::read_to_string(path).unwrap_or_default();
        SavedViewYaml(yaml).with_comments_from(SavedViewYaml(&existing))
    } else {
        yaml.to_owned()
    };
    let tmp_path = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("yml")
    ));
    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)?;
    Ok(())
}

pub(super) struct SavedViewYaml<'a>(pub(super) &'a str);

impl SavedViewYaml<'_> {
    pub(super) fn with_comments_from(self, existing: Self) -> String {
        let header = existing.header_comments();
        let inline_comments = existing.inline_comments();
        let mut merged = self.apply_inline_comments(&inline_comments);
        if !header.is_empty() {
            merged = format!("{header}\n{merged}");
        }
        merged
    }

    fn header_comments(&self) -> String {
        self.0
            .lines()
            .take_while(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn inline_comments(&self) -> BTreeMap<YamlPath, String> {
        let mut tracker = SavedViewYamlPathTracker::default();
        let mut comments = BTreeMap::new();
        for line in self.0.lines().map(YamlLine::parse) {
            let path = tracker.path_for_line(line);
            let Some((path, comment)) = path.zip(line.comment) else {
                continue;
            };
            let comment = comment.trim().to_owned();
            if let Some(wildcard) = path.sequence_wildcard() {
                comments.insert(wildcard, comment.clone());
            }
            comments.insert(path, comment);
        }
        comments
    }

    fn apply_inline_comments(self, comments: &BTreeMap<YamlPath, String>) -> String {
        let mut tracker = SavedViewYamlPathTracker::default();
        let mut output = String::new();
        for line in self.0.lines() {
            output.push_str(line);
            let comment = tracker
                .path_for_line(YamlLine::parse(line))
                .and_then(|path| {
                    comments
                        .get(&path)
                        .or_else(|| comments.get(&path.sequence_wildcard()?))
                });
            if let Some(comment) = comment {
                output.push(' ');
                output.push_str(comment);
            }
            output.push('\n');
        }
        output
    }
}

#[derive(Clone, Copy)]
struct YamlLine<'a> {
    content: &'a str,
    comment: Option<&'a str>,
}

impl<'a> YamlLine<'a> {
    fn parse(line: &'a str) -> Self {
        if let Some((content, comment)) = split_yaml_inline_comment(line) {
            Self {
                content,
                comment: Some(comment),
            }
        } else {
            Self {
                content: line,
                comment: None,
            }
        }
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct YamlPath(String);

impl YamlPath {
    fn sequence_wildcard(&self) -> Option<Self> {
        let (prefix, suffix) = self.0.rsplit_once('.')?;
        suffix
            .chars()
            .all(|ch| ch.is_ascii_digit())
            .then(|| Self(format!("{prefix}.*")))
    }

    fn push_key(&mut self, key: &str) {
        if !self.0.is_empty() {
            self.0.push('.');
        }
        self.0.push_str(key);
    }
}

fn split_yaml_inline_comment(line: &str) -> Option<(&str, &str)> {
    #[derive(PartialEq, Eq)]
    enum Quoting {
        Unquoted,
        Single,
        Double,
        Escaped,
    }
    use Quoting::{Double, Escaped, Single, Unquoted};

    let mut quoting = Unquoted;
    for (index, ch) in line.char_indices() {
        quoting = match (&quoting, ch) {
            (Escaped, _) => Double,
            (Double, '\\') => Escaped,
            (Unquoted, '\'') => Single,
            (Unquoted, '"') => Double,
            (Single, '\'') | (Double, '"') => Unquoted,
            (Unquoted, '#') => {
                let content = line[..index].trim_end();
                return (!content.is_empty()).then_some((content, &line[index..]));
            }
            _ => quoting,
        };
    }
    None
}

#[derive(Default)]
struct SavedViewYamlPathTracker {
    stack: Vec<(usize, String)>,
    sequence_indices: BTreeMap<YamlPath, usize>,
}

impl SavedViewYamlPathTracker {
    fn path_for_line(&mut self, line: YamlLine<'_>) -> Option<YamlPath> {
        let trimmed = line.content.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let indent = line.content.len().saturating_sub(trimmed.len());
        if let Some(rest) = trimmed.strip_prefix("- ") {
            self.stack
                .retain(|(stack_indent, _)| *stack_indent < indent);
            let parent = self.current_path();
            let index = self.sequence_indices.entry(parent).or_insert(0);
            let index_component = index.to_string();
            *index += 1;
            self.stack.push((indent, index_component));
            return self
                .path_for_mapping(rest)
                .or_else(|| Some(self.current_path()));
        }

        self.stack
            .retain(|(stack_indent, _)| *stack_indent < indent);
        self.path_for_mapping(trimmed)
    }

    fn path_for_mapping(&mut self, content: &str) -> Option<YamlPath> {
        let (key, value) = content.split_once(':')?;
        let key = saved_view_yaml_path_key(key);
        if key.is_empty() {
            return None;
        }
        let mut path = self.current_path();
        path.push_key(&key);
        if value.trim().is_empty() {
            let indent = self
                .stack
                .last()
                .map(|(indent, _)| indent.saturating_add(2))
                .unwrap_or(0);
            self.stack.push((indent, key));
        }
        Some(path)
    }

    fn current_path(&self) -> YamlPath {
        YamlPath(
            self.stack
                .iter()
                .map(|(_, component)| component.as_str())
                .collect::<Vec<_>>()
                .join("."),
        )
    }
}

fn saved_view_yaml_path_key(key: &str) -> String {
    let key = key.trim();
    key.strip_prefix('"')
        .and_then(|key| key.strip_suffix('"'))
        .or_else(|| {
            key.strip_prefix('\'')
                .and_then(|key| key.strip_suffix('\''))
        })
        .unwrap_or(key)
        .to_owned()
}
