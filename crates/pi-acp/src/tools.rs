use std::{
    fmt::Write,
    path::{Component, Path, PathBuf},
};

use agent_client_protocol::v2::schema::{
    Diff, DiffChange, ToolCallContent, ToolCallLocation, ToolKind,
};
use serde_json::Value;

/// The kind of a call of Pi's tool `name`.
pub fn kind(name: &str) -> ToolKind {
    match name {
        "read" | "ls" => ToolKind::Read,
        "edit" | "write" => ToolKind::Edit,
        "grep" | "find" => ToolKind::Search,
        "bash" => ToolKind::Execute,
        _ => ToolKind::Other,
    }
}

/// The title of a call of tool `name`: the tool's name capitalized, followed
/// by the first line of its main argument as Markdown inline code.
pub fn title(name: &str, args: &Value) -> String {
    let mut chars = name.chars();
    let display: String = chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default();
    let key = match name {
        "bash" => "command",
        "grep" | "find" => "pattern",
        _ => "path",
    };
    let argument = args[key]
        .as_str()
        .and_then(|text| text.trim().lines().next())
        .map(|first_line| {
            let mut argument: String = first_line.chars().take(160).collect();
            if first_line.chars().count() > 160 {
                argument.push('…');
            }
            argument
        });
    match argument {
        Some(argument) => format!("{display} {}", code_span(&argument)),
        None => display,
    }
}

/// The file a call's `path` argument names, resolved against `cwd`.
pub fn path(args: &Value, cwd: &Path) -> Option<PathBuf> {
    let path = args["path"].as_str()?;
    let mut resolved = PathBuf::new();
    for component in cwd.join(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    Some(resolved)
}

/// The file a call of tool `name` works on, with the line `read` starts at.
pub fn locations(name: &str, args: &Value, cwd: &Path) -> Vec<ToolCallLocation> {
    let Some(path) = path(args, cwd) else {
        return Vec::new();
    };
    let mut location = ToolCallLocation::new(path);
    if name == "read"
        && let Some(line) = args["offset"]
            .as_u64()
            .filter(|line| *line > 0)
            .and_then(|line| u32::try_from(line).ok())
    {
        location = location.line(line);
    }
    vec![location]
}

/// The change a successful `edit` or `write` made, from its arguments and
/// result `details`. `existed` says whether the file existed when the tool
/// call opened; `None` means historical existence is unknown.
pub fn diff(
    name: &str,
    args: &Value,
    details: &Value,
    cwd: &Path,
    existed: Option<bool>,
) -> Option<ToolCallContent> {
    let file = path(args, cwd)?;
    let diff = match name {
        "edit" => match details["patch"].as_str() {
            Some(unified) => {
                Diff::patch(git_patch(unified, &file), vec![DiffChange::modify(&*file)])
            }
            None => Diff::new(vec![DiffChange::modify(file)]),
        },
        "write" if existed == Some(true) => Diff::new(vec![DiffChange::modify(file)]),
        "write" if existed == Some(false) => Diff::patch(
            new_file_patch(&file, args["content"].as_str().unwrap_or("")),
            vec![DiffChange::add(&*file)],
        ),
        _ => return None,
    };
    Some(ToolCallContent::Diff(diff))
}

/// Pi's unified patch of `file` in ACP's `git_patch` format: its `---` and
/// `+++` headers, which name the file as the tool was given it, become a
/// `diff --git` header with the absolute path.
// TODO: quote paths with characters Git quotes, should such a path come up.
fn git_patch(unified: &str, file: &Path) -> String {
    let file = file.display();
    let hunks = unified
        .find("\n@@")
        .map_or("", |start| &unified[start + 1..]);
    format!("diff --git {file} {file}\n--- {file}\n+++ {file}\n{hunks}")
}

/// A `git_patch` that adds `file` with `content`.
fn new_file_patch(file: &Path, content: &str) -> String {
    let file = file.display();
    let mut text =
        format!("diff --git {file} {file}\nnew file mode 100644\n--- /dev/null\n+++ {file}\n");
    if content.is_empty() {
        return text;
    }
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() == 1 {
        text.push_str("@@ -0,0 +1 @@\n");
    } else {
        let _ = writeln!(text, "@@ -0,0 +1,{} @@", lines.len());
    }
    for line in &lines {
        text.push('+');
        text.push_str(line);
        text.push('\n');
    }
    if !content.ends_with('\n') {
        text.push_str("\\ No newline at end of file\n");
    }
    text
}

/// `text` as a Markdown code span, fenced by a run of backticks longer than
/// any run inside it. A space pads each side where text starting or ending
/// with a backtick would extend the fence, or where Markdown would strip one
/// of the spaces `text` starts and ends with.
fn code_span(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest + 1);
    let stripped = text.starts_with(' ') && text.ends_with(' ') && text.contains(|c| c != ' ');
    let pad = if text.starts_with('`') || text.ends_with('`') || stripped {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}
