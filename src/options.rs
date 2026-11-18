//! Plugin settings → generation options.

use openidea_plugin::host;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum NamingStrategy {
    #[default]
    Field,
    SnakeCase,
    KebabCase,
    CamelCase,
    PascalCase,
    UpperSnake,
}

impl NamingStrategy {
    fn parse(value: &str) -> Self {
        match value {
            "snake_case" => Self::SnakeCase,
            "kebab-case" => Self::KebabCase,
            "camelCase" => Self::CamelCase,
            "PascalCase" => Self::PascalCase,
            "UPPER_SNAKE" => Self::UpperSnake,
            _ => Self::Field,
        }
    }

    pub(crate) fn apply(self, name: &str) -> String {
        match self {
            Self::Field => name.to_owned(),
            Self::SnakeCase => words(name)
                .into_iter()
                .map(|word| word.to_lowercase())
                .collect::<Vec<_>>()
                .join("_"),
            Self::KebabCase => words(name)
                .into_iter()
                .map(|word| word.to_lowercase())
                .collect::<Vec<_>>()
                .join("-"),
            Self::CamelCase => {
                let mut words = words(name).into_iter();
                let Some(first) = words.next() else {
                    return name.to_owned();
                };
                let mut out = first.to_lowercase();
                for word in words {
                    out.push_str(&capitalize(&word));
                }
                out
            }
            Self::PascalCase => words(name)
                .into_iter()
                .map(|word| capitalize(&word))
                .collect(),
            Self::UpperSnake => words(name)
                .into_iter()
                .map(|word| word.to_uppercase())
                .collect::<Vec<_>>()
                .join("_"),
        }
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out = first.to_uppercase().collect::<String>();
    out.push_str(&chars.as_str().to_lowercase());
    out
}

/// Split `filePath`, `file_path`, `URLPath` into words.
fn words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();

    for (index, ch) in chars.iter().copied().enumerate() {
        if ch == '_' || ch == '-' || ch == ' ' {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        if ch.is_uppercase() && !current.is_empty() {
            let previous = chars[index - 1];
            let next_is_lower = chars.get(index + 1).is_some_and(|next| next.is_lowercase());
            if previous.is_lowercase() || previous.is_ascii_digit() || next_is_lower {
                out.push(std::mem::take(&mut current));
            }
        }
        current.push(ch);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UnknownType {
    #[default]
    Null,
    Object,
}

#[derive(Clone, Debug)]
pub(crate) struct Options {
    pub(crate) naming: NamingStrategy,
    pub(crate) include_static: bool,
    pub(crate) include_transient: bool,
    pub(crate) include_inherited: bool,
    pub(crate) scalar_collection_sample: bool,
    pub(crate) unknown_type: UnknownType,
    pub(crate) max_depth: usize,
    pub(crate) indent: usize,
    pub(crate) preview_before_copy: bool,
    pub(crate) timezone_offset_minutes: i64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            naming: NamingStrategy::Field,
            include_static: false,
            include_transient: true,
            include_inherited: true,
            scalar_collection_sample: false,
            unknown_type: UnknownType::Null,
            max_depth: 5,
            indent: 2,
            preview_before_copy: false,
            timezone_offset_minutes: 480,
        }
    }
}

impl Options {
    pub(crate) fn from_settings() -> Self {
        let naming = host::setting_string("namingStrategy")
            .map(|value| NamingStrategy::parse(&value))
            .unwrap_or_default();
        let unknown_type = host::setting_string("unknownType")
            .map(|value| {
                if value == "object" {
                    UnknownType::Object
                } else {
                    UnknownType::Null
                }
            })
            .unwrap_or_default();
        let max_depth = host::setting_number("maxDepth")
            .unwrap_or(5.0)
            .round()
            .clamp(1.0, 10.0) as usize;
        let indent = host::setting_string("indent")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value == 4)
            .unwrap_or(2);
        let timezone_offset_minutes = host::setting_number("timezoneOffsetMinutes")
            .unwrap_or(480.0)
            .clamp(-720.0, 840.0) as i64;

        Self {
            naming,
            include_static: host::setting_bool("includeStatic").unwrap_or(false),
            include_transient: host::setting_bool("includeTransient").unwrap_or(true),
            include_inherited: host::setting_bool("includeInherited").unwrap_or(true),
            scalar_collection_sample: host::setting_bool("scalarCollectionSample").unwrap_or(false),
            unknown_type,
            max_depth,
            indent,
            preview_before_copy: host::setting_bool("previewBeforeCopy").unwrap_or(false),
            timezone_offset_minutes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_strategies() {
        assert_eq!(NamingStrategy::Field.apply("filePath"), "filePath");
        assert_eq!(NamingStrategy::SnakeCase.apply("filePath"), "file_path");
        assert_eq!(NamingStrategy::SnakeCase.apply("URLPath"), "url_path");
        assert_eq!(NamingStrategy::KebabCase.apply("filePath"), "file-path");
        assert_eq!(NamingStrategy::CamelCase.apply("file_path"), "filePath");
        assert_eq!(NamingStrategy::PascalCase.apply("file_path"), "FilePath");
        assert_eq!(NamingStrategy::UpperSnake.apply("filePath"), "FILE_PATH");
    }
}
