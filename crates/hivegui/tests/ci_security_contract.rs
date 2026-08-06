use std::{collections::BTreeSet, fs, path::PathBuf};

const GITLEAKS_VERSION: &str = "8.30.1";
const CARGO_DENY_VERSION: &str = "0.20.2";
const SQLX_CLI_VERSION: &str = "0.9.0";
const MYSQL_ASYNC_VERSION: &str = "=0.36.2";
const SQLX_VERSION: &str = "=0.9.0";
const AWS_SDK_S3_VERSION: &str = "=1.133.0";
const VENDORED_WAYLAND_SCANNER_VERSION: &str = "0.31.10";

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("resolve repository root")
}

fn workflow_source() -> String {
    fs::read_to_string(repository_root().join(".github/workflows/ci.yml"))
        .expect("read the blocking CI workflow")
}

fn repository_source(relative_path: &str) -> String {
    fs::read_to_string(repository_root().join(relative_path))
        .unwrap_or_else(|error| panic!("read {relative_path}: {error}"))
}

fn without_toml_comment(line: &str) -> String {
    let mut result = String::new();
    let mut quoted = false;
    let mut escaped = false;

    for character in line.chars() {
        if escaped {
            result.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => {
                result.push(character);
                escaped = true;
            }
            '"' => {
                quoted = !quoted;
                result.push(character);
            }
            '#' if !quoted => break,
            _ => result.push(character),
        }
    }
    result
}

fn toml_nesting_delta(source: &str) -> isize {
    let mut depth = 0;
    let mut quoted = false;
    let mut escaped = false;

    for character in source.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '{' | '[' if !quoted => depth += 1,
            '}' | ']' if !quoted => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn toml_value(source: &str, section: &str, key: &str) -> Option<String> {
    let section_header = format!("[{section}]");
    let mut in_section = false;
    let mut lines = source.lines();

    while let Some(raw_line) = lines.next() {
        let line = without_toml_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == section_header;
            continue;
        }
        if !in_section || trimmed.is_empty() {
            continue;
        }

        let Some((candidate, value)) = trimmed.split_once('=') else {
            continue;
        };
        if candidate.trim().trim_matches(['\'', '"']) != key {
            continue;
        }

        let mut value = value.trim().to_owned();
        let mut depth = toml_nesting_delta(&value);
        while depth > 0 {
            let continuation = lines
                .next()
                .unwrap_or_else(|| panic!("unterminated TOML value for [{section}] {key}"));
            let continuation = without_toml_comment(continuation);
            depth += toml_nesting_delta(&continuation);
            value.push(' ');
            value.push_str(continuation.trim());
        }
        return Some(value);
    }
    None
}

fn split_top_level(source: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0;
    let mut quoted = false;
    let mut escaped = false;

    for (index, character) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '{' | '[' if !quoted => depth += 1,
            '}' | ']' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                parts.push(source[start..index].trim());
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(source[start..].trim());
    parts
}

fn inline_field<'a>(value: &'a str, field: &str) -> Option<&'a str> {
    let body = value.trim().strip_prefix('{')?.strip_suffix('}')?;
    split_top_level(body).into_iter().find_map(|part| {
        let (candidate, value) = part.split_once('=')?;
        (candidate.trim() == field).then_some(value.trim())
    })
}

fn unquoted(value: &str) -> Option<&str> {
    value.trim().strip_prefix('"')?.strip_suffix('"')
}

fn dependency_version(value: &str) -> Option<&str> {
    unquoted(value).or_else(|| inline_field(value, "version").and_then(unquoted))
}

fn dependency_bool(value: &str, field: &str) -> Option<bool> {
    match inline_field(value, field)? {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn quoted_values(value: &str) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut current = String::new();

    for character in value.chars() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' if quoted => {
                quoted = false;
                values.insert(std::mem::take(&mut current));
            }
            '"' => quoted = true,
            _ if quoted => current.push(character),
            _ => {}
        }
    }
    values
}

fn dependency_features(value: &str) -> BTreeSet<String> {
    inline_field(value, "features")
        .map(quoted_values)
        .unwrap_or_default()
}

fn expected_features(features: &[&str]) -> BTreeSet<String> {
    features
        .iter()
        .map(|feature| (*feature).to_owned())
        .collect()
}

fn assert_dependency_version(
    manifest_path: &str,
    section: &str,
    dependency: &str,
    expected: &str,
) -> String {
    let manifest = repository_source(manifest_path);
    let value = toml_value(&manifest, section, dependency).unwrap_or_else(|| {
        panic!("{manifest_path} [{section}] must declare `{dependency}` explicitly")
    });
    assert_eq!(
        dependency_version(&value),
        Some(expected),
        "{manifest_path} [{section}] `{dependency}` must pin version `{expected}`; found `{value}`"
    );
    value
}

fn lock_package_version(package: &str) -> Option<(String, String)> {
    let package = format!("[package]\n{package}");
    let name = toml_value(&package, "package", "name")?;
    let version = toml_value(&package, "package", "version")?;
    Some((unquoted(&name)?.to_owned(), unquoted(&version)?.to_owned()))
}

#[derive(Debug)]
struct WorkflowStep {
    yaml: String,
    run: Option<String>,
    item_indent: usize,
    job_yaml: String,
    job_direct_indent: usize,
}

fn indentation(line: &str) -> usize {
    line.bytes().take_while(|byte| *byte == b' ').count()
}

fn without_yaml_comment(line: &str) -> String {
    let mut result = String::new();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;

    for character in line.chars() {
        if escaped {
            result.push(character);
            escaped = false;
            continue;
        }

        match character {
            '\\' if double_quoted => {
                result.push(character);
                escaped = true;
            }
            '\'' if !double_quoted => {
                single_quoted = !single_quoted;
                result.push(character);
            }
            '"' if !single_quoted => {
                double_quoted = !double_quoted;
                result.push(character);
            }
            '#' if !single_quoted && !double_quoted => break,
            _ => result.push(character),
        }
    }

    result
}

fn extract_run(step_lines: &[&str], item_indent: usize) -> Option<String> {
    for (index, line) in step_lines.iter().enumerate() {
        if indentation(line) != item_indent + 2 {
            continue;
        }

        let yaml = without_yaml_comment(line);
        let trimmed = yaml.trim_start();
        let Some(value) = trimmed.strip_prefix("run:") else {
            continue;
        };
        if !value.is_empty() && !value.starts_with(char::is_whitespace) {
            continue;
        }

        let value = value.trim();
        if value.starts_with('|') || value.starts_with('>') {
            let run_indent = indentation(line);
            let mut script = String::new();
            for block_line in &step_lines[index + 1..] {
                if !block_line.trim().is_empty() && indentation(block_line) <= run_indent {
                    break;
                }
                script.push_str(block_line.trim_start());
                script.push('\n');
            }
            return Some(script);
        }

        return Some(value.trim_matches(['\'', '"']).to_owned());
    }

    None
}

fn workflow_steps(source: &str) -> Vec<WorkflowStep> {
    let lines = source.lines().collect::<Vec<_>>();
    let mut steps = Vec::new();

    for (steps_index, line) in lines.iter().enumerate() {
        let yaml = without_yaml_comment(line);
        if yaml.trim() != "steps:" {
            continue;
        }

        let steps_indent = indentation(line);
        let job_indent = (0..steps_index)
            .rev()
            .find_map(|index| {
                let code = without_yaml_comment(lines[index]);
                let trimmed = code.trim();
                (indentation(lines[index]) < steps_indent
                    && trimmed.ends_with(':')
                    && !trimmed.starts_with("- "))
                .then_some(indentation(lines[index]))
            })
            .expect("steps must be nested in a CI job");
        let job_start = (0..steps_index)
            .rev()
            .find(|index| {
                let code = without_yaml_comment(lines[*index]);
                indentation(lines[*index]) == job_indent
                    && code.trim().ends_with(':')
                    && !code.trim().starts_with("- ")
            })
            .expect("locate CI job start");
        let job_end = (steps_index + 1..lines.len())
            .find(|index| {
                let code = without_yaml_comment(lines[*index]);
                !code.trim().is_empty() && indentation(lines[*index]) <= job_indent
            })
            .unwrap_or(lines.len());
        let job_yaml = lines[job_start..job_end].join("\n");
        let mut index = steps_index + 1;
        while index < lines.len() {
            let code = without_yaml_comment(lines[index]);
            if code.trim().is_empty() {
                index += 1;
                continue;
            }
            if indentation(lines[index]) <= steps_indent {
                break;
            }
            if !code.trim_start().starts_with("- ") {
                index += 1;
                continue;
            }

            let item_indent = indentation(lines[index]);
            let start = index;
            index += 1;
            while index < lines.len() {
                let next_code = without_yaml_comment(lines[index]);
                if !next_code.trim().is_empty()
                    && (indentation(lines[index]) <= steps_indent
                        || (indentation(lines[index]) == item_indent
                            && next_code.trim_start().starts_with("- ")))
                {
                    break;
                }
                index += 1;
            }

            let step_lines = &lines[start..index];
            steps.push(WorkflowStep {
                yaml: step_lines.join("\n"),
                run: extract_run(step_lines, item_indent),
                item_indent,
                job_yaml: job_yaml.clone(),
                job_direct_indent: steps_indent,
            });
        }
    }

    steps
}

fn direct_yaml_value(step: &WorkflowStep, key: &str) -> Option<String> {
    step.yaml.lines().find_map(|line| {
        let code = without_yaml_comment(line);
        let indent = indentation(line);
        let trimmed = if indent == step.item_indent {
            code.trim_start().strip_prefix("- ")?
        } else if indent == step.item_indent + 2 {
            code.trim()
        } else {
            return None;
        };
        trimmed
            .strip_prefix(key)
            .and_then(|value| value.strip_prefix(':'))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn direct_job_value(step: &WorkflowStep, key: &str) -> Option<String> {
    step.job_yaml.lines().find_map(|line| {
        if indentation(line) != step.job_direct_indent {
            return None;
        }
        let code = without_yaml_comment(line);
        code.trim()
            .strip_prefix(key)
            .and_then(|value| value.strip_prefix(':'))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn nested_yaml_value(
    source: &str,
    parent_indent: usize,
    parent: &str,
    child: &str,
) -> Option<String> {
    let lines = source.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let code = without_yaml_comment(line);
        if indentation(line) != parent_indent || code.trim() != format!("{parent}:") {
            continue;
        }
        for child_line in &lines[index + 1..] {
            let child_code = without_yaml_comment(child_line);
            if !child_code.trim().is_empty() && indentation(child_line) <= parent_indent {
                break;
            }
            if let Some(value) = child_code
                .trim()
                .strip_prefix(&format!("{child}:"))
                .map(str::trim)
                .map(|value| value.trim_matches(['\'', '"']))
            {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn scoped_exact_env(step: &WorkflowStep, name: &str, expected: &str) -> bool {
    scoped_env_value(step, name).is_some_and(|value| value == expected)
}

fn scoped_env_value(step: &WorkflowStep, name: &str) -> Option<String> {
    nested_yaml_value(&step.yaml, step.item_indent + 2, "env", name)
        .or_else(|| nested_yaml_value(&step.job_yaml, step.job_direct_indent, "env", name))
}

fn condition_is_guaranteed_true(value: &str) -> bool {
    let value = value.trim().trim_matches(['\'', '"']);
    let value = value
        .strip_prefix("${{")
        .and_then(|value| value.strip_suffix("}}"))
        .unwrap_or(value)
        .split_whitespace()
        .collect::<String>()
        .to_ascii_lowercase();
    matches!(value.as_str(), "true" | "always()")
}

fn metadata_is_reachable_and_blocking(step: &WorkflowStep) -> bool {
    [direct_job_value(step, "if"), direct_yaml_value(step, "if")]
        .into_iter()
        .flatten()
        .all(|condition| condition_is_guaranteed_true(&condition))
        && [
            direct_job_value(step, "continue-on-error"),
            direct_yaml_value(step, "continue-on-error"),
        ]
        .into_iter()
        .flatten()
        .all(|value| value.trim_matches(['\'', '"']) == "false")
}

fn assert_actions_are_commit_pinned(steps: &[WorkflowStep]) {
    for action in steps
        .iter()
        .filter_map(|step| direct_yaml_value(step, "uses"))
    {
        let revision = action
            .trim_matches(['\'', '"'])
            .rsplit_once('@')
            .map(|(_, revision)| revision)
            .unwrap_or_default();
        assert!(
            revision.len() == 40
                && revision
                    .chars()
                    .all(|character| character.is_ascii_hexdigit()),
            "CI actions must be pinned to a full commit SHA, found `{action}`"
        );
    }
}

fn has_sha256_literal(source: &str) -> bool {
    source
        .split(|character: char| !character.is_ascii_hexdigit())
        .any(|word| word.len() == 64)
}

fn yaml_scalar_matches(source: &str, name: &str, predicate: impl Fn(&str) -> bool) -> bool {
    let mut block_indent = None;

    for line in source.lines() {
        let code = without_yaml_comment(line);
        if let Some(indent) = block_indent {
            if code.trim().is_empty() || indentation(line) > indent {
                continue;
            }
            block_indent = None;
        }

        let trimmed = code.trim();
        let value = trimmed
            .strip_prefix(&format!("{name}:"))
            .map(str::trim)
            .map(|value| value.trim_matches(['\'', '"']));
        if value.is_some_and(&predicate) {
            return true;
        }

        if trimmed
            .split_once(':')
            .is_some_and(|(_, value)| matches!(value.trim(), "|" | "|-" | "|+" | ">" | ">-" | ">+"))
        {
            block_indent = Some(indentation(line));
        }
    }

    false
}

fn has_exact_yaml_scalar(source: &str, name: &str, expected: &str) -> bool {
    yaml_scalar_matches(source, name, |value| value == expected)
}

fn without_shell_comment(line: &str) -> String {
    let mut result = String::new();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;

    for character in line.chars() {
        if escaped {
            result.push(character);
            escaped = false;
            continue;
        }

        match character {
            '\\' if !single_quoted => {
                result.push(character);
                escaped = true;
            }
            '\'' if !double_quoted => {
                single_quoted = !single_quoted;
                result.push(character);
            }
            '"' if !single_quoted => {
                double_quoted = !double_quoted;
                result.push(character);
            }
            '#' if !single_quoted && !double_quoted => break,
            _ => result.push(character),
        }
    }

    result
}

fn shell_lines(script: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut pending = String::new();
    let mut heredoc_end: Option<String> = None;

    for raw_line in script.lines() {
        if let Some(delimiter) = &heredoc_end {
            if raw_line.trim() == delimiter {
                heredoc_end = None;
            }
            continue;
        }

        let code = without_shell_comment(raw_line);
        let trimmed = code.trim();
        if trimmed.is_empty() {
            continue;
        }

        let continued = trimmed.ends_with('\\');
        pending.push_str(trimmed.trim_end_matches('\\').trim_end());
        if continued {
            pending.push(' ');
            continue;
        }

        if let Some(delimiter) = heredoc_delimiter(&pending) {
            heredoc_end = Some(delimiter);
        }
        lines.push(std::mem::take(&mut pending));
    }

    if !pending.trim().is_empty() {
        lines.push(pending);
    }
    lines
}

fn heredoc_delimiter(line: &str) -> Option<String> {
    let tokens = shell_words(line);
    for (index, token) in tokens.iter().enumerate() {
        if token == "<<" || token == "<<-" {
            return tokens.get(index + 1).cloned();
        }
        if let Some(delimiter) = token.strip_prefix("<<-").filter(|value| !value.is_empty()) {
            return Some(delimiter.to_owned());
        }
        if let Some(delimiter) = token.strip_prefix("<<").filter(|value| !value.is_empty()) {
            return Some(delimiter.to_owned());
        }
    }
    None
}

fn shell_segments(line: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut segment = String::new();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;
    let mut characters = line.chars().peekable();

    while let Some(character) = characters.next() {
        if escaped {
            segment.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if !single_quoted => {
                segment.push(character);
                escaped = true;
            }
            '\'' if !double_quoted => {
                single_quoted = !single_quoted;
                segment.push(character);
            }
            '"' if !single_quoted => {
                double_quoted = !double_quoted;
                segment.push(character);
            }
            ';' | '|' if !single_quoted && !double_quoted => {
                if characters.peek() == Some(&character) {
                    characters.next();
                }
                if !segment.trim().is_empty() {
                    segments.push(std::mem::take(&mut segment));
                }
            }
            '&' if !single_quoted && !double_quoted && characters.peek() == Some(&'&') => {
                characters.next();
                if !segment.trim().is_empty() {
                    segments.push(std::mem::take(&mut segment));
                }
            }
            _ => segment.push(character),
        }
    }

    if !segment.trim().is_empty() {
        segments.push(segment);
    }
    segments
}

fn shell_words(segment: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut single_quoted = false;
    let mut double_quoted = false;
    let mut escaped = false;

    for character in segment.chars() {
        if escaped {
            word.push(character);
            escaped = false;
            continue;
        }
        match character {
            '\\' if !single_quoted => escaped = true,
            '\'' if !double_quoted => single_quoted = !single_quoted,
            '"' if !single_quoted => double_quoted = !double_quoted,
            character if character.is_whitespace() && !single_quoted && !double_quoted => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(character),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        let mut characters = name.chars();
        characters
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
            && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
    })
}

fn executable_command(words: &[String]) -> &[String] {
    let mut index = 0;
    while words
        .get(index)
        .is_some_and(|word| matches!(word.as_str(), "if" | "then" | "do" | "!"))
    {
        index += 1;
    }
    while words.get(index).is_some_and(|word| is_assignment(word)) {
        index += 1;
    }

    if words.get(index).is_some_and(|word| word == "sudo") {
        index += 1;
        while words.get(index).is_some_and(|word| word.starts_with('-')) {
            index += 1;
        }
    }
    if words.get(index).is_some_and(|word| word == "env") {
        index += 1;
        while words
            .get(index)
            .is_some_and(|word| word.starts_with('-') || is_assignment(word))
        {
            index += 1;
        }
    }
    if words.get(index).is_some_and(|word| word == "command") {
        index += 1;
        while words.get(index).is_some_and(|word| word.starts_with('-')) {
            index += 1;
        }
    }

    &words[index..]
}

fn commands(step: &WorkflowStep) -> Vec<Vec<String>> {
    step.run
        .as_deref()
        .into_iter()
        .flat_map(shell_lines)
        .flat_map(|line| shell_segments(&line))
        .map(|segment| shell_words(&segment))
        .filter(|words| !words.is_empty())
        .collect()
}

fn command_in_step(step: &WorkflowStep, predicate: impl Fn(&[String]) -> bool) -> bool {
    commands(step)
        .iter()
        .map(|words| executable_command(words))
        .any(predicate)
}

fn raw_command_in_step(step: &WorkflowStep, predicate: impl Fn(&[String]) -> bool) -> bool {
    commands(step).iter().any(|words| predicate(words))
}

fn run_step_matching<'a>(
    steps: &'a [WorkflowStep],
    purpose: &str,
    predicate: impl Fn(&[String]) -> bool,
) -> &'a WorkflowStep {
    steps
        .iter()
        .find(|step| metadata_is_reachable_and_blocking(step) && command_in_step(step, &predicate))
        .unwrap_or_else(|| panic!("CI workflow is missing an executable {purpose} command"))
}

fn run_step_matching_raw<'a>(
    steps: &'a [WorkflowStep],
    purpose: &str,
    predicate: impl Fn(&[String]) -> bool,
) -> &'a WorkflowStep {
    steps
        .iter()
        .find(|step| {
            metadata_is_reachable_and_blocking(step) && raw_command_in_step(step, &predicate)
        })
        .unwrap_or_else(|| panic!("CI workflow is missing an executable {purpose} command"))
}

fn contains_words(command: &[String], expected: &[&str]) -> bool {
    command.windows(expected.len()).any(|window| {
        window
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
    })
}

fn starts_with_words(command: &[String], expected: &[&str]) -> bool {
    command
        .iter()
        .take(expected.len())
        .map(String::as_str)
        .eq(expected.iter().copied())
}

fn executable_name(command: &[String]) -> &str {
    command
        .first()
        .map(|word| word.rsplit('/').next().unwrap_or(word))
        .unwrap_or_default()
}

fn command_has_assignment(command: &[String], name: &str, expected: &str) -> bool {
    command.iter().any(|word| {
        word.split_once('=')
            .is_some_and(|(candidate, value)| candidate == name && value == expected)
    })
}

fn references_shell_variable(source: &str, name: &str) -> bool {
    let compact = source.split_whitespace().collect::<String>();
    source.contains(&format!("${name}"))
        || source.contains(&format!("${{{name}}}"))
        || compact.contains(&format!("${{{{env.{name}}}}}"))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn approved_digest_is_available(step: &WorkflowStep) -> bool {
    scoped_env_value(step, "GITLEAKS_SHA256").is_some_and(|value| is_sha256(&value))
        || commands(step).iter().any(|words| {
            let executable = executable_command(words);
            (executable.is_empty()
                && words
                    .iter()
                    .take_while(|word| is_assignment(word))
                    .any(|word| word.strip_prefix("GITLEAKS_SHA256=").is_some_and(is_sha256)))
                || (executable_name(executable) == "export"
                    && executable
                        .iter()
                        .skip(1)
                        .any(|word| word.strip_prefix("GITLEAKS_SHA256=").is_some_and(is_sha256)))
        })
}

fn sha256sum_checks_manifest(command: &[String]) -> bool {
    executable_name(command) == "sha256sum"
        && command.iter().skip(1).any(|argument| {
            argument == "--check"
                || (argument.starts_with('-')
                    && !argument.starts_with("--")
                    && argument[1..].contains('c'))
        })
}

fn downloaded_asset(command: &[String]) -> Option<String> {
    for pair in command.windows(2) {
        if matches!(pair[0].as_str(), "-o" | "-O" | "--output") && pair[1] != "-" {
            return Some(pair[1].clone());
        }
    }
    if let Some(output) = command
        .iter()
        .find_map(|argument| argument.strip_prefix("--output="))
        .filter(|output| *output != "-")
    {
        return Some(output.to_owned());
    }
    command
        .iter()
        .find(|argument| argument.contains("gitleaks/releases/download"))
        .and_then(|url| url.rsplit('/').next())
        .map(|asset| asset.split(['?', '#']).next().unwrap_or(asset).to_owned())
        .filter(|asset| !asset.is_empty())
}

fn references_downloaded_asset(source: &str, asset: &str) -> bool {
    source.contains(asset)
        || asset
            .rsplit('/')
            .next()
            .is_some_and(|file_name| !file_name.is_empty() && source.contains(file_name))
}

fn checked_manifest_binds_digest_and_asset(step: &WorkflowStep, asset: &str) -> bool {
    let Some(script) = step.run.as_deref() else {
        return false;
    };
    shell_lines(script).iter().any(|line| {
        let has_check = shell_segments(line).iter().any(|segment| {
            let words = shell_words(segment);
            sha256sum_checks_manifest(executable_command(&words))
        });
        has_check
            && line.contains('|')
            && !line.contains(';')
            && references_downloaded_asset(line, asset)
            && (references_shell_variable(line, "GITLEAKS_SHA256")
                && approved_digest_is_available(step)
                || has_sha256_literal(line))
    })
}

fn hash_assignment(step: &WorkflowStep, asset: &str) -> Option<String> {
    let script = step.run.as_deref()?;
    for line in shell_lines(script) {
        let Some((prefix, substitution)) = line.split_once("=$(") else {
            continue;
        };
        let Some(variable) = prefix
            .split_whitespace()
            .last()
            .filter(|value| {
                let mut characters = value.chars();
                characters
                    .next()
                    .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
                    && characters
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            })
            .map(ToOwned::to_owned)
        else {
            continue;
        };
        let substitution = substitution
            .rsplit_once(')')
            .map_or(substitution, |(body, _)| body);
        let hashes_asset = shell_segments(substitution).iter().any(|segment| {
            let words = shell_words(segment);
            let command = executable_command(&words);
            (executable_name(command) == "sha256sum"
                || executable_name(command) == "shasum" && contains_words(command, &["-a", "256"]))
                && command
                    .iter()
                    .any(|argument| references_downloaded_asset(argument, asset))
        });
        if hashes_asset {
            return Some(variable);
        }
    }
    None
}

fn explicitly_compares_digest(step: &WorkflowStep, asset: &str) -> bool {
    let Some(actual_variable) = hash_assignment(step, asset) else {
        return false;
    };
    approved_digest_is_available(step)
        && commands(step).iter().any(|words| {
            let command = executable_command(words);
            matches!(executable_name(command), "test" | "[" | "[[")
                && command
                    .iter()
                    .any(|word| matches!(word.as_str(), "=" | "=="))
                && command
                    .iter()
                    .any(|word| references_shell_variable(word, &actual_variable))
                && command
                    .iter()
                    .any(|word| references_shell_variable(word, "GITLEAKS_SHA256"))
        })
}

fn assert_blocking(step: &WorkflowStep, purpose: &str) {
    assert!(
        metadata_is_reachable_and_blocking(step),
        "{purpose} must be unconditionally reachable and blocking at both job and step scope:\n{}",
        step.yaml
    );

    let script = step
        .run
        .as_deref()
        .expect("matched step must have run script");
    let executable = shell_lines(script).join("\n");
    assert!(
        !executable.contains("||"),
        "{purpose} must not use `||` to bypass failure:\n{}",
        step.yaml
    );
    for words in commands(step) {
        let command = executable_command(&words);
        let bypass = starts_with_words(command, &["set", "+e"])
            || starts_with_words(command, &["true"])
            || starts_with_words(command, &["exit", "0"]);
        assert!(
            !bypass,
            "{purpose} must be blocking; found failure bypass in:\n{}",
            step.yaml
        );
    }
}

#[test]
fn secret_scan_is_fixed_version_checksum_verified_full_history_and_blocking() {
    let workflow = workflow_source();
    let steps = workflow_steps(&workflow);

    assert!(
        !steps.is_empty(),
        "CI workflow must contain parsed YAML steps"
    );
    assert_actions_are_commit_pinned(&steps);

    let checkout_step = steps
        .iter()
        .find(|step| {
            metadata_is_reachable_and_blocking(step)
                && direct_yaml_value(step, "uses").is_some_and(|action| {
                    action
                        .trim_matches(['\'', '"'])
                        .starts_with("actions/checkout@")
                })
        })
        .expect("CI workflow must have an actual actions/checkout step");
    assert!(
        has_exact_yaml_scalar(&checkout_step.yaml, "fetch-depth", "0"),
        "secret scanning git history requires a full checkout"
    );

    let is_gitleaks_download = |command: &[String]| {
        matches!(executable_name(command), "curl" | "wget")
            && command.iter().any(|argument| {
                argument.contains("gitleaks/gitleaks/releases/download")
                    || argument.contains("gitleaks/releases/download")
            })
    };
    let download_step =
        run_step_matching(&steps, "Gitleaks release download", is_gitleaks_download);
    assert!(
        command_in_step(download_step, |command| {
            if !is_gitleaks_download(command) {
                return false;
            }
            executable_name(command) == "wget"
                || command.iter().any(|argument| {
                    argument == "--fail"
                        || argument == "--fail-with-body"
                        || (argument.starts_with('-')
                            && !argument.starts_with("--")
                            && argument[1..].contains('f'))
                })
        }),
        "the Gitleaks release download must fail on HTTP errors"
    );
    assert!(
        command_in_step(download_step, |command| {
            is_gitleaks_download(command)
                && command.iter().any(|argument| {
                    let is_download_url = argument.contains("gitleaks/releases/download");
                    let uses_literal = argument.contains(&format!("/v{GITLEAKS_VERSION}/"))
                        || argument.contains(&format!("gitleaks_{GITLEAKS_VERSION}_"));
                    let uses_scoped_variable =
                        references_shell_variable(argument, "GITLEAKS_VERSION")
                            && scoped_exact_env(
                                download_step,
                                "GITLEAKS_VERSION",
                                GITLEAKS_VERSION,
                            );
                    is_download_url && (uses_literal || uses_scoped_variable)
                })
        }),
        "Gitleaks must be pinned to exactly {GITLEAKS_VERSION}"
    );
    assert_blocking(download_step, "Gitleaks release download");
    let download_commands = commands(download_step);
    let downloaded_asset = download_commands
        .iter()
        .map(|words| executable_command(words))
        .find(|command| is_gitleaks_download(command))
        .and_then(downloaded_asset)
        .expect("Gitleaks download must identify the asset later bound to its approved digest");

    let checksum_step = steps
        .iter()
        .find(|step| {
            metadata_is_reachable_and_blocking(step)
                && (checked_manifest_binds_digest_and_asset(step, &downloaded_asset)
                    || explicitly_compares_digest(step, &downloaded_asset))
        })
        .expect(
            "Gitleaks checksum must bind the approved digest to the downloaded asset via `sha256sum --check` or an explicit strict comparison",
        );
    assert_blocking(checksum_step, "Gitleaks checksum verification");

    let install_step = run_step_matching(&steps, "Gitleaks installation", |command| {
        matches!(executable_name(command), "install" | "mv")
            && command.iter().any(|argument| argument.contains("gitleaks"))
            && command.iter().any(|argument| {
                argument == "/usr/local/bin/gitleaks"
                    || argument == "/usr/local/bin/"
                    || argument.starts_with("/usr/local/bin/") && argument.ends_with("gitleaks")
            })
    });
    assert_blocking(install_step, "Gitleaks installation");

    let scan_step = run_step_matching(&steps, "Gitleaks secret scan", |command| {
        executable_name(command) == "gitleaks"
            && command
                .get(1)
                .is_some_and(|subcommand| matches!(subcommand.as_str(), "git" | "detect"))
            && !command
                .iter()
                .any(|argument| argument == "--no-git" || argument.starts_with("--no-git="))
    });
    assert_blocking(scan_step, "Gitleaks secret scan");

    assert!(
        repository_root().join(".gitleaks.toml").is_file(),
        "the reviewed Gitleaks configuration must be committed"
    );
}

#[test]
fn dependency_advisory_scan_is_fixed_version_and_blocking() {
    let workflow = workflow_source();
    let steps = workflow_steps(&workflow);

    let install_step = run_step_matching(&steps, "cargo-deny installation", |command| {
        starts_with_words(
            command,
            &[
                "cargo",
                "install",
                "--locked",
                "cargo-deny",
                "--version",
                CARGO_DENY_VERSION,
            ],
        )
    });
    assert_blocking(install_step, "cargo-deny installation");

    let advisory_step = run_step_matching(&steps, "dependency advisory scan", |command| {
        starts_with_words(command, &["cargo", "deny", "check", "advisories"])
    });
    assert_blocking(advisory_step, "dependency advisory scan");
}

#[test]
fn approved_dependency_remediation_mysql_async_is_minimal() {
    let mysql_async = assert_dependency_version(
        "Cargo.toml",
        "workspace.dependencies",
        "mysql_async",
        MYSQL_ASYNC_VERSION,
    );
    assert_eq!(
        dependency_bool(&mysql_async, "default-features"),
        Some(false),
        "Cargo.toml mysql_async must set `default-features = false` so the remediated graph stays minimal"
    );
    assert_eq!(
        dependency_features(&mysql_async),
        expected_features(&["minimal", "native-tls-tls"]),
        "Cargo.toml mysql_async must enable exactly `minimal` and `native-tls-tls`"
    );
}

#[test]
fn approved_dependency_remediation_workspace_sqlx_is_featureless() {
    let sqlx =
        assert_dependency_version("Cargo.toml", "workspace.dependencies", "sqlx", SQLX_VERSION);
    assert_eq!(
        dependency_bool(&sqlx, "default-features"),
        Some(false),
        "Cargo.toml workspace SQLx must set `default-features = false`"
    );
    assert!(
        inline_field(&sqlx, "features").is_none(),
        "Cargo.toml workspace SQLx must not enable shared features; select database and TLS features in each consuming crate, found `{sqlx}`"
    );
}

#[test]
fn approved_dependency_remediation_agent_does_not_depend_on_sqlx() {
    let agent = repository_source("crates/agent/Cargo.toml");
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        assert!(
            toml_value(&agent, section, "sqlx").is_none(),
            "crates/agent/Cargo.toml [{section}] must not depend on SQLx; persistence belongs to the HiveGUI/HiveWeb boundary crates"
        );
    }
}

#[test]
fn approved_dependency_remediation_hivegui_sqlx_is_sqlite_only() {
    let hivegui = repository_source("crates/hivegui/Cargo.toml");
    let hivegui_sqlx = toml_value(&hivegui, "dependencies", "sqlx")
        .expect("crates/hivegui/Cargo.toml [dependencies] must declare SQLx");
    assert_eq!(
        dependency_bool(&hivegui_sqlx, "workspace"),
        Some(true),
        "HiveGUI SQLx must inherit the exact workspace version"
    );
    assert_eq!(
        dependency_features(&hivegui_sqlx),
        expected_features(&["chrono", "macros", "runtime-tokio", "sqlite"]),
        "HiveGUI SQLx must enable exactly SQLite, Tokio, macros, and chrono; `macros` includes the derive support required by checked queries, while external MySQL TLS belongs to mysql_async, so SQLx must compile neither a MySQL driver nor a TLS backend"
    );
}

#[test]
fn approved_dependency_remediation_hiveweb_sqlx_uses_verified_tls_without_rsa() {
    let hiveweb = repository_source("crates/hiveweb/Cargo.toml");
    let hiveweb_sqlx = toml_value(&hiveweb, "dependencies", "sqlx")
        .expect("crates/hiveweb/Cargo.toml [dependencies] must declare SQLx");
    assert_eq!(
        dependency_bool(&hiveweb_sqlx, "workspace"),
        Some(true),
        "HiveWeb SQLx must inherit the exact workspace version"
    );
    assert_eq!(
        dependency_features(&hiveweb_sqlx),
        expected_features(&[
            "chrono",
            "json",
            "macros",
            "mysql",
            "runtime-tokio",
            "rust_decimal",
            "tls-rustls-ring-webpki",
        ]),
        "HiveWeb SQLx must enable exactly its MySQL data types, Tokio, and the verified WebPKI TLS path; `derive` is provided by `macros` (per T017G/T017D) and must not be re-declared; `mysql-rsa` and broader TLS/database features are forbidden"
    );
}

#[test]
fn approved_dependency_remediation_aws_s3_uses_only_the_modern_https_client() {
    let aws_s3 = assert_dependency_version(
        "Cargo.toml",
        "workspace.dependencies",
        "aws-sdk-s3",
        AWS_SDK_S3_VERSION,
    );
    assert_eq!(
        dependency_bool(&aws_s3, "default-features"),
        Some(false),
        "Cargo.toml aws-sdk-s3 must disable defaults because the default set also enables the legacy Rustls client"
    );
    assert_eq!(
        dependency_features(&aws_s3),
        expected_features(&["default-https-client", "http-1x", "rt-tokio", "sigv4a",]),
        "Cargo.toml aws-sdk-s3 must enable exactly the four modern client features and must not enable legacy `rustls`"
    );
}

#[test]
fn approved_dependency_remediation_wayland_scanner_is_a_reviewable_local_patch() {
    let root = repository_source("Cargo.toml");
    let patch = toml_value(&root, "patch.crates-io", "wayland-scanner").expect(
        "Cargo.toml [patch.crates-io] must route wayland-scanner to the reviewed local copy",
    );
    assert_eq!(
        inline_field(&patch, "path").and_then(unquoted),
        Some("third_party/wayland-scanner"),
        "wayland-scanner must use `third_party/wayland-scanner`; found `{patch}`"
    );

    let vendor_manifest_path = repository_root().join("third_party/wayland-scanner/Cargo.toml");
    assert!(
        vendor_manifest_path.is_file(),
        "missing third_party/wayland-scanner/Cargo.toml for the reviewed quick-xml compatibility patch"
    );
    let vendor = repository_source("third_party/wayland-scanner/Cargo.toml");
    assert_eq!(
        toml_value(&vendor, "package", "name")
            .as_deref()
            .and_then(unquoted),
        Some("wayland-scanner"),
        "vendored manifest must preserve the upstream crate identity"
    );
    assert_eq!(
        toml_value(&vendor, "package", "version")
            .as_deref()
            .and_then(unquoted),
        Some(VENDORED_WAYLAND_SCANNER_VERSION),
        "vendored wayland-scanner must preserve upstream version {VENDORED_WAYLAND_SCANNER_VERSION}"
    );
    let repository = toml_value(&vendor, "package", "repository")
        .and_then(|value| unquoted(&value).map(ToOwned::to_owned))
        .unwrap_or_default();
    assert!(
        repository == "https://github.com/smithay/wayland-rs",
        "vendored wayland-scanner must preserve its upstream repository provenance; found `{repository}`"
    );

    let quick_xml = toml_value(&vendor, "dependencies", "quick-xml")
        .and_then(|value| dependency_version(&value).map(ToOwned::to_owned))
        .or_else(|| {
            toml_value(&vendor, "dependencies.quick-xml", "version")
                .and_then(|value| unquoted(&value).map(ToOwned::to_owned))
        });
    assert_eq!(
        quick_xml.as_deref(),
        Some("0.41"),
        "vendored wayland-scanner must change only its quick-xml compatibility dependency to 0.41"
    );

    let provenance_path = repository_root().join("third_party/wayland-scanner/PROVENANCE.md");
    assert!(
        provenance_path.is_file(),
        "third_party/wayland-scanner/PROVENANCE.md must record the reviewed upstream source and minimal compatibility patch"
    );
    let provenance = repository_source("third_party/wayland-scanner/PROVENANCE.md");
    assert!(
        provenance.contains("https://github.com/smithay/wayland-rs")
            && provenance.contains(VENDORED_WAYLAND_SCANNER_VERSION),
        "wayland-scanner provenance must identify smithay/wayland-rs and upstream version {VENDORED_WAYLAND_SCANNER_VERSION}"
    );
    assert!(
        provenance.contains("xml_content") && provenance.contains("xml10_content"),
        "wayland-scanner provenance must disclose the minimal `xml_content` to `xml10_content` quick-xml 0.41 compatibility patch"
    );
}

#[test]
fn approved_dependency_remediation_lockfile_excludes_advisory_packages() {
    let lockfile = repository_source("Cargo.lock");
    let packages = lockfile
        .split("[[package]]")
        .skip(1)
        .filter_map(lock_package_version)
        .collect::<Vec<_>>();
    let forbidden = [
        ("rsa", None, "RUSTSEC-2023-0071"),
        ("lru", Some("0.12.5"), "RUSTSEC-2026-0002"),
        ("quick-xml", Some("0.39.4"), "RUSTSEC-2026-0194/0195"),
        (
            "rustls-webpki",
            Some("0.101.7"),
            "RUSTSEC-2026-0098/0099/0104",
        ),
    ];
    let remaining = forbidden
        .into_iter()
        .filter_map(|(name, version, advisory)| {
            packages
                .iter()
                .find(|(candidate_name, candidate_version)| {
                    candidate_name == name
                        && version.is_none_or(|expected| candidate_version == expected)
                })
                .map(|(_, found_version)| format!("{name} {found_version} ({advisory})"))
        })
        .collect::<Vec<_>>();
    assert!(
        remaining.is_empty(),
        "Cargo.lock still contains forbidden advisory packages: {}. Apply the approved dependency remediation and regenerate the lockfile without advisory exceptions",
        remaining.join(", ")
    );
}

#[test]
fn approved_dependency_remediation_zbus_xml_is_exact() {
    let lockfile = repository_source("Cargo.lock");
    let zbus_xml_versions = lockfile
        .split("[[package]]")
        .skip(1)
        .filter_map(lock_package_version)
        .filter_map(|(name, version)| (name == "zbus_xml").then_some(version))
        .collect::<Vec<_>>();
    assert_eq!(
        zbus_xml_versions,
        vec!["5.2.1"],
        "Cargo.lock must resolve exactly zbus_xml 5.2.1 for the approved quick-xml remediation; found {zbus_xml_versions:?}"
    );
}

#[test]
fn approved_dependency_remediation_has_no_advisory_exceptions() {
    let deny = repository_source("deny.toml");
    assert!(
        toml_value(&deny, "advisories", "ignore").is_none(),
        "deny.toml must not define [advisories].ignore; the approved remediation has zero advisory exceptions"
    );
}

#[test]
fn approved_dependency_remediation_sqlx_offline_check_uses_exact_cli_and_is_blocking() {
    let workflow = workflow_source();
    let steps = workflow_steps(&workflow);

    let install_step = run_step_matching(&steps, "cargo-sqlx installation", |command| {
        starts_with_words(
            command,
            &[
                "cargo",
                "install",
                "--locked",
                "sqlx-cli",
                "--version",
                SQLX_CLI_VERSION,
                "--no-default-features",
                "--features",
                "sqlite,mysql,rustls",
            ],
        )
    });
    assert_blocking(install_step, "cargo-sqlx installation");

    let check_step = run_step_matching_raw(
        &steps,
        "SQLx offline metadata freshness check",
        |raw_command| {
            starts_with_words(
                executable_command(raw_command),
                &["cargo", "sqlx", "prepare", "--workspace", "--check"],
            ) && command_has_assignment(raw_command, "SQLX_OFFLINE", "true")
        },
    );
    assert_blocking(check_step, "SQLx offline metadata freshness check");
}

#[test]
fn sqlx_offline_metadata_is_committed_and_nonempty() {
    let metadata_directory = repository_root().join(".sqlx");
    assert!(
        metadata_directory.is_dir(),
        "missing .sqlx directory: offline metadata absence must block CI"
    );

    let metadata_files = fs::read_dir(&metadata_directory)
        .expect("read .sqlx metadata directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.starts_with("query-") && name.ends_with(".json"))
        })
        .count();

    assert!(
        metadata_files > 0,
        ".sqlx must contain committed query-*.json metadata; staleness is enforced by the exact --check command"
    );
}
