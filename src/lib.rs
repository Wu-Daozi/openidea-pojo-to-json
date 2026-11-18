//! Copy a Java POJO's fields as a JSON skeleton with type-based defaults.

mod defaults;
mod options;
mod pojo;

use std::collections::HashMap;

use openidea_plugin::{
    Error, Invocation, NotificationLevel, PickItem, Plugin, Project, host, register_plugin,
};

const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_EXTERNAL_SOURCE_BYTES: usize = 2 * 1024 * 1024;
const MAX_RESOLVE_ATTEMPTS: usize = 128;
const MAX_CANDIDATE_READS: usize = 40;

#[derive(Default)]
struct PojoToJson;

impl Plugin for PojoToJson {
    fn run_command(
        &mut self,
        id: &str,
        ctx: &Invocation,
        _arg: Option<&str>,
    ) -> Result<Option<String>, Error> {
        match id {
            "copy-json" => copy_json(ctx),
            _ => Err(Error::NotFound(format!("command {id}"))),
        }
    }
}

fn copy_json(ctx: &Invocation) -> Result<Option<String>, Error> {
    let editor = ctx
        .editor()
        .ok_or_else(|| Error::Failed("Open a Java file first".into()))?;

    let language = editor.language();
    if language != "java" {
        return Err(Error::Failed(format!(
            "Copy JSON only works in Java files (current language: {language})"
        )));
    }

    let document = editor.document()?;
    if document.text.len() > MAX_SOURCE_BYTES {
        return Err(Error::Failed(
            "This Java file is too large for Copy JSON (limit: 2 MiB)".into(),
        ));
    }

    let offset = editor
        .selections()?
        .first()
        .and_then(|range| openidea_plugin::text::offset(&document.text, range.start));

    let options = options::Options::from_settings();
    let now = defaults::Now::from_offset_minutes(options.timezone_offset_minutes);

    let project = ctx.project();
    let mut sources: HashMap<String, Option<String>> = HashMap::new();
    let mut resolve = |name: &str, hints: &pojo::ResolutionHints| -> Option<String> {
        let key = format!("{name}|{}", hints.key(name));
        if let Some(source) = sources.get(&key) {
            return source.clone();
        }
        if sources.len() >= MAX_RESOLVE_ATTEMPTS {
            return None;
        }
        let source = project
            .as_ref()
            .and_then(|project| resolve_project_type(project, name, hints));
        sources.insert(key, source.clone());
        source
    };

    let generated = match pojo::generate_with(&document.text, offset, &mut resolve, &options, &now)
    {
        Ok(generated) => generated,
        Err(pojo::GenerateError::NeedChoice(names)) => {
            let items: Vec<PickItem> = names.iter().map(|name| PickItem::new(name, name)).collect();
            let Some(choice) = host::pick("Copy JSON: choose a class", &items)? else {
                return Err(Error::Cancelled);
            };
            pojo::generate_named_with(&document.text, &choice, &mut resolve, &options, &now)
                .map_err(generate_error)?
        }
        Err(error) => return Err(generate_error(error)),
    };

    let field_count = if options.preview_before_copy {
        let Some(edited) = host::input_multiline("Copy JSON: review", Some(&generated.json))?
        else {
            return Err(Error::Cancelled);
        };
        host::copy_to_clipboard(&edited)?;
        top_level_field_count(&edited)
    } else {
        host::copy_to_clipboard(&generated.json)?;
        Some(generated.field_count)
    };

    let body = match field_count {
        Some(0) => format!(
            "Copied {{}} for {} (no serializable fields)",
            generated.class_name
        ),
        Some(count) => format!(
            "Copied JSON for {} ({} fields)",
            generated.class_name, count
        ),
        None => format!("Copied JSON for {}", generated.class_name),
    };
    host::notify(NotificationLevel::Info, "Copy JSON", &body);

    Ok(None)
}

/// Finds the source of a Java type declared in another file of the open
/// project. Public classes live in `<Name>.java`, which is the cheap path; the
/// project symbol index covers nested and package-private declarations.
fn resolve_project_type(
    project: &Project,
    name: &str,
    hints: &pojo::ResolutionHints,
) -> Option<String> {
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    if let Some(qualified) = hints.imports.get(name) {
        let package = qualified
            .rsplit_once('.')
            .map(|(package, _)| package.to_owned());
        candidates.push((
            format!("**/{}.java", qualified.replace('.', "/")),
            package.clone(),
        ));
        candidates.push((format!("{}.java", qualified.replace('.', "/")), package));
    }
    if let Some(package) = &hints.package {
        candidates.push((
            format!("**/{}/{name}.java", package.replace('.', "/")),
            Some(package.clone()),
        ));
        candidates.push((
            format!("{}/{name}.java", package.replace('.', "/")),
            Some(package.clone()),
        ));
    }
    for package in &hints.wildcard_packages {
        candidates.push((
            format!("**/{}/{name}.java", package.replace('.', "/")),
            Some(package.clone()),
        ));
        candidates.push((
            format!("{}/{name}.java", package.replace('.', "/")),
            Some(package.clone()),
        ));
    }
    let fallback_package = hints
        .imports
        .get(name)
        .and_then(|qualified| {
            qualified
                .rsplit_once('.')
                .map(|(package, _)| package.to_owned())
        })
        .or_else(|| hints.package.clone());
    candidates.push((format!("**/{name}.java"), fallback_package.clone()));
    candidates.push((format!("{name}.java"), fallback_package));

    let mut reads = 0;
    for (pattern, expected_package) in candidates {
        if reads >= MAX_CANDIDATE_READS {
            break;
        }
        let Ok(mut paths) = project.find_files(&pattern, 20) else {
            continue;
        };
        paths.sort_by_key(|path| (source_rank(path), path.clone()));
        for path in paths {
            if reads >= MAX_CANDIDATE_READS {
                break;
            }
            reads += 1;
            if let Ok(source) = project.read_file(&path)
                && source.len() <= MAX_EXTERNAL_SOURCE_BYTES
                && pojo::declares_type(&source, name, expected_package.as_deref())
            {
                return Some(source);
            }
        }
    }

    let preferred_package = hints
        .imports
        .get(name)
        .and_then(|qualified| qualified.rsplit_once('.').map(|(package, _)| package))
        .or(hints.package.as_deref());

    let mut fallback = None;
    if let Ok(symbols) = project.symbols(name, 50) {
        for symbol in symbols {
            let kind = symbol.kind.as_str();
            let kind_matches = kind.eq_ignore_ascii_case("class")
                || kind.eq_ignore_ascii_case("record")
                || kind.eq_ignore_ascii_case("enum");
            if symbol.name != name || !kind_matches || !symbol.location.path.ends_with(".java") {
                continue;
            }
            let Ok(source) = project.read_file(&symbol.location.path) else {
                continue;
            };
            if source.len() > MAX_EXTERNAL_SOURCE_BYTES {
                continue;
            }
            let package_matches = preferred_package
                .is_none_or(|package| symbol.container.as_deref() == Some(package));
            if package_matches && pojo::declares_type(&source, name, preferred_package) {
                return Some(source);
            }
            if symbol.container.is_none() && fallback.is_none() {
                fallback = Some(source);
            }
        }
    }
    fallback
}

fn source_rank(path: &str) -> u8 {
    if path.contains("/src/main/") {
        0
    } else if path.contains("/src/test/") {
        1
    } else {
        2
    }
}

fn top_level_field_count(json: &str) -> Option<usize> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|value| value.as_object().map(serde_json::Map::len))
}

fn generate_error(error: pojo::GenerateError) -> Error {
    match error {
        pojo::GenerateError::NeedChoice(names) => Error::Failed(format!(
            "Several classes match; place the cursor inside one: {}",
            names.join(", ")
        )),
        pojo::GenerateError::Failed(message) => Error::Failed(message),
    }
}

register_plugin!(PojoToJson);

#[cfg(test)]
mod tests {
    use super::*;
    use openidea_plugin::mock::MockHost;

    fn run(host: &MockHost, source: &str, language: &str, caret: (u32, u32)) -> Result<(), Error> {
        host.open_editor(
            "/project/src/User.java",
            language,
            source,
            &[(caret.0, caret.1, caret.0, caret.1)],
        );
        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .map(|_| ())
    }

    #[test]
    fn copies_json_to_the_clipboard() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        let source = "public class User {\n    private int id;\n    private String name;\n}\n";

        run(&host, source, "java", (0, 16)).unwrap();

        assert_eq!(
            host.clipboard().as_deref(),
            Some("{\n  \"id\": 0,\n  \"name\": \"\"\n}")
        );
        let notifications = host.notifications();
        assert_eq!(notifications.len(), 1);
        assert!(notifications[0].body.contains("User"));
        assert!(notifications[0].body.contains("2 fields"));
    }

    #[test]
    fn expands_lists_of_types_from_other_project_files() {
        let host =
            MockHost::install().with_capabilities(&["editor.read", "clipboard", "project.read"]);
        host.set_project_root(Some("/project"));
        host.add_file(
            "/project/src/main/java/UploadFileVo.java",
            "public class UploadFileVo {\n    private String filePath;\n    private String fileName;\n    private String newFileName;\n}\n",
        );
        let source =
            "public class A {\n    private Long id;\n    private List<UploadFileVo> fileList;\n}\n";
        host.open_editor(
            "/project/src/main/java/A.java",
            "java",
            source,
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();

        assert_eq!(
            host.clipboard().as_deref(),
            Some(
                "{\n  \"id\": 0,\n  \"fileList\": [\n    {\n      \"filePath\": \"\",\n      \"fileName\": \"\",\n      \"newFileName\": \"\"\n    }\n  ]\n}"
            )
        );
    }

    #[test]
    fn resolves_same_named_types_by_import() {
        let host =
            MockHost::install().with_capabilities(&["editor.read", "clipboard", "project.read"]);
        host.set_project_root(Some("/project"));
        host.add_file(
            "/project/src/main/java/com/a/UploadFileVo.java",
            "package com.a;\npublic class UploadFileVo {\n    private String aOnly;\n}\n",
        );
        host.add_file(
            "/project/src/main/java/com/b/UploadFileVo.java",
            "package com.b;\npublic class UploadFileVo {\n    private String bOnly;\n}\n",
        );
        let source = "package com.use;\nimport com.b.UploadFileVo;\nimport java.util.List;\npublic class A {\n    private List<UploadFileVo> files;\n}\n";
        host.open_editor(
            "/project/src/main/java/com/use/A.java",
            "java",
            source,
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();

        assert_eq!(
            host.clipboard().as_deref(),
            Some("{\n  \"files\": [\n    {\n      \"bOnly\": \"\"\n    }\n  ]\n}")
        );
    }

    #[test]
    fn resolves_types_in_the_same_package_without_an_import() {
        let host =
            MockHost::install().with_capabilities(&["editor.read", "clipboard", "project.read"]);
        host.set_project_root(Some("/project"));
        host.add_file(
            "/project/src/main/java/com/a/UploadFileVo.java",
            "package com.a;\npublic class UploadFileVo {\n    private String samePackage;\n}\n",
        );
        let source = "package com.a;\npublic class A {\n    private UploadFileVo file;\n}\n";
        host.open_editor(
            "/project/src/main/java/com/a/A.java",
            "java",
            source,
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();

        assert_eq!(
            host.clipboard().as_deref(),
            Some("{\n  \"file\": {\n    \"samePackage\": \"\"\n  }\n}")
        );
    }

    #[test]
    fn resolves_wildcard_imports() {
        let host =
            MockHost::install().with_capabilities(&["editor.read", "clipboard", "project.read"]);
        host.set_project_root(Some("/project"));
        host.add_file(
            "/project/src/main/java/com/b/UploadFileVo.java",
            "package com.b;\npublic class UploadFileVo {\n    private String wildcardOnly;\n}\n",
        );
        let source = "package com.use;\nimport com.b.*;\npublic class A {\n    private UploadFileVo file;\n}\n";
        host.open_editor(
            "/project/src/main/java/com/use/A.java",
            "java",
            source,
            &[(0, 0, 0, 0)],
        );

        let project = host.invocation().project().unwrap();
        let hints = pojo::resolution_hints(source);
        let resolved = resolve_project_type(&project, "UploadFileVo", &hints);
        assert!(resolved.is_some(), "hints: {hints:?}");

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();
        assert_eq!(
            host.clipboard().as_deref(),
            Some("{\n  \"file\": {\n    \"wildcardOnly\": \"\"\n  }\n}")
        );
    }

    #[test]
    fn prefers_main_sources_over_test_sources() {
        let host =
            MockHost::install().with_capabilities(&["editor.read", "clipboard", "project.read"]);
        host.set_project_root(Some("/project"));
        host.add_file(
            "/project/src/main/java/com/a/UploadFileVo.java",
            "package com.a;\npublic class UploadFileVo {\n    private String mainOnly;\n}\n",
        );
        host.add_file(
            "/project/src/test/java/com/a/UploadFileVo.java",
            "package com.a;\npublic class UploadFileVo {\n    private String testOnly;\n}\n",
        );
        let source = "package com.a;\nimport com.a.UploadFileVo;\npublic class A {\n    private UploadFileVo file;\n}\n";
        host.open_editor(
            "/project/src/main/java/com/a/A.java",
            "java",
            source,
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();
        assert_eq!(
            host.clipboard().as_deref(),
            Some("{\n  \"file\": {\n    \"mainOnly\": \"\"\n  }\n}")
        );
    }

    #[test]
    fn skips_candidates_with_the_wrong_package() {
        let host =
            MockHost::install().with_capabilities(&["editor.read", "clipboard", "project.read"]);
        host.set_project_root(Some("/project"));
        host.add_file(
            "/project/src/main/java/com/a/UploadFileVo.java",
            "package com.b;\npublic class UploadFileVo {\n    private String wrongPackage;\n}\n",
        );
        let source = "package com.a;\nimport com.a.UploadFileVo;\npublic class A {\n    private UploadFileVo file;\n}\n";
        host.open_editor(
            "/project/src/main/java/com/a/A.java",
            "java",
            source,
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();
        assert_eq!(host.clipboard().as_deref(), Some("{\n  \"file\": null\n}"));
    }

    #[test]
    fn previews_and_copies_the_edited_json() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        host.set_setting("previewBeforeCopy", "true");
        host.answer_next_prompt(Some(&["{\n  \"id\": 99\n}"]));
        host.open_editor(
            "/project/src/User.java",
            "java",
            "public class User { private int id; }",
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();

        assert_eq!(host.clipboard().as_deref(), Some("{\n  \"id\": 99\n}"));
        assert!(host.notifications()[0].body.contains("1 fields"));
    }

    #[test]
    fn cancelling_the_preview_does_not_copy() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        host.set_setting("previewBeforeCopy", "true");
        host.answer_next_prompt(None);
        host.open_editor(
            "/project/src/User.java",
            "java",
            "public class User { private int id; }",
            &[(0, 0, 0, 0)],
        );

        let error = PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap_err();

        assert!(matches!(error, Error::Cancelled));
        assert_eq!(host.clipboard(), None);
    }

    #[test]
    fn reads_naming_and_indent_settings() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        host.set_setting("namingStrategy", "\"snake_case\"");
        host.set_setting("indent", "\"4\"");
        host.open_editor(
            "/project/src/User.java",
            "java",
            "public class User { private String filePath; }",
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();

        assert_eq!(
            host.clipboard().as_deref(),
            Some("{\n    \"file_path\": \"\"\n}")
        );
    }

    #[test]
    fn skips_files_that_declare_a_different_type() {
        let host =
            MockHost::install().with_capabilities(&["editor.read", "clipboard", "project.read"]);
        host.set_project_root(Some("/project"));
        host.add_file(
            "/project/src/main/java/com/a/Foo.java",
            "package com.a;\npublic class FooBar {\n    private String bar;\n}\n",
        );
        let source =
            "package com.a;\nimport com.a.Foo;\npublic class A {\n    private Foo file;\n}\n";
        host.open_editor(
            "/project/src/main/java/com/a/A.java",
            "java",
            source,
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();
        assert_eq!(host.clipboard().as_deref(), Some("{\n  \"file\": null\n}"));
    }

    #[test]
    fn skips_external_sources_over_the_size_limit() {
        let host =
            MockHost::install().with_capabilities(&["editor.read", "clipboard", "project.read"]);
        host.set_project_root(Some("/project"));
        let padding = " ".repeat(MAX_EXTERNAL_SOURCE_BYTES + 16);
        host.add_file(
            "/project/src/main/java/com/a/UploadFileVo.java",
            &format!(
                "package com.a;\npublic class UploadFileVo {{ private String value; }}\n{padding}"
            ),
        );
        let source = "package com.a;\nimport com.a.UploadFileVo;\npublic class A {\n    private UploadFileVo file;\n}\n";
        host.open_editor(
            "/project/src/main/java/com/a/A.java",
            "java",
            source,
            &[(0, 0, 0, 0)],
        );

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();
        assert_eq!(host.clipboard().as_deref(), Some("{\n  \"file\": null\n}"));
    }

    #[test]
    fn command_honours_single_string_ignore_properties() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        let source = "import com.fasterxml.jackson.annotation.JsonIgnoreProperties;\n@JsonIgnoreProperties(\"internalNote\")\npublic class Request {\n    private String internalNote;\n    private String name;\n}\n";
        host.open_editor("/project/src/Request.java", "java", source, &[(0, 0, 0, 0)]);

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();

        assert_eq!(host.clipboard().as_deref(), Some("{\n  \"name\": \"\"\n}"));
    }

    #[test]
    fn command_honours_explicit_value_single_string_ignore_properties() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        let source = "import com.fasterxml.jackson.annotation.JsonIgnoreProperties;\n@JsonIgnoreProperties(value = \"internalNote\")\npublic class Request {\n    private String internalNote;\n    private String name;\n}\n";
        host.open_editor("/project/src/Request.java", "java", source, &[(0, 0, 0, 0)]);

        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();

        assert_eq!(host.clipboard().as_deref(), Some("{\n  \"name\": \"\"\n}"));
    }

    #[test]
    fn reports_a_denied_clipboard_permission() {
        let host = MockHost::install().with_capabilities(&["editor.read"]);
        let error = run(
            &host,
            "public class User { private int id; }",
            "java",
            (0, 16),
        )
        .unwrap_err();

        assert!(matches!(error, Error::Denied(capability) if capability == "clipboard"));
        assert_eq!(host.clipboard(), None);
    }

    #[test]
    fn rejects_non_java_editors() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        let error = run(&host, "data class User(val id: Int)", "kotlin", (0, 0)).unwrap_err();

        assert!(matches!(error, Error::Failed(message) if message.contains("Java")));
        assert_eq!(host.clipboard(), None);
    }

    #[test]
    fn rejects_a_missing_editor() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        let error = PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap_err();

        assert!(matches!(error, Error::Failed(message) if message.contains("Open a Java file")));
    }

    #[test]
    fn picks_a_class_and_honours_cancellation() {
        let host = MockHost::install().with_capabilities(&["editor.read", "clipboard"]);
        let source = "import java.util.List;\nclass A { int a; }\nclass B { int b; }\n";
        host.open_editor("/project/src/Two.java", "java", source, &[(0, 0, 0, 0)]);

        host.answer_next_prompt(None);
        let error = PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap_err();
        assert!(matches!(error, Error::Cancelled));
        assert_eq!(host.clipboard(), None);

        host.answer_next_prompt(Some(&["B"]));
        PojoToJson
            .run_command("copy-json", &host.invocation(), None)
            .unwrap();
        assert_eq!(host.clipboard().as_deref(), Some("{\n  \"b\": 0\n}"));
    }
}
