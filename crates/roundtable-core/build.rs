//! Bundles `agent_templates/*.md` into a single file for `include_str!`.
//!
//! Each template is one markdown file: a `---` frontmatter block (plain
//! `key: value` lines) plus the system prompt as the body. This script
//! validates every file at compile time so a broken template fails the
//! build with the filename — not at runtime in front of the user.

use std::path::Path;

const REQUIRED: &[&str] = &[
    "key",
    "name",
    "avatar_color",
    "avatar_icon",
    "role",
    "model",
    "temperature",
    "max_tokens",
];

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("agent_templates");
    println!("cargo:rerun-if-changed=agent_templates");
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("agent_templates/ directory is missing")
        .map(|entry| entry.expect("unreadable agent_templates entry").path())
        .filter(|path| path.extension().map(|ext| ext == "md").unwrap_or(false))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "agent_templates/ has no .md files — the app would ship with no presets"
    );
    for path in &files {
        let name = path
            .file_name()
            .expect("template without a filename")
            .to_string_lossy()
            .to_string();
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|_| panic!("cannot read {name}"));
        validate(&name, &text);
        entries.push((name, text));
    }
    let mut bundle = String::new();
    for (name, text) in &entries {
        bundle.push_str(&format!("\n@@@@TEMPLATE:{name}@@@@\n{text}"));
        if !text.ends_with('\n') {
            bundle.push('\n');
        }
    }
    let out = Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR is set"))
        .join("agent_templates_bundle.txt");
    std::fs::write(&out, bundle).expect("cannot write template bundle");
}

fn validate(name: &str, text: &str) {
    let mut lines = text.lines();
    assert!(
        lines.next().map(str::trim) == Some("---"),
        "{name}: must start with a `---` frontmatter fence"
    );
    let mut seen = std::collections::HashSet::new();
    let mut body_started = false;
    for line in lines {
        if !body_started && line.trim() == "---" {
            body_started = true;
            continue;
        }
        if !body_started {
            let (key, _) = line.split_once(':').unwrap_or_else(|| {
                panic!("{name}: bad frontmatter line (want `key: value`): {line}")
            });
            seen.insert(key.trim().to_string());
        }
    }
    assert!(
        body_started,
        "{name}: frontmatter never closes with a second `---`"
    );
    for required in REQUIRED {
        assert!(
            seen.contains(*required),
            "{name}: frontmatter is missing `{required}`"
        );
    }
    let body = text.splitn(3, "---").nth(2).unwrap_or("").trim();
    assert!(
        !body.is_empty(),
        "{name}: the system prompt body below the frontmatter is empty"
    );
}
