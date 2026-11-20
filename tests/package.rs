//! The release archive's layout, built here as `release.yml` builds it:
//! `tracon-plugin.toml`, `bin/workboard` and `ui/` (from `spa/dist`). The
//! manifest is what the node reads, the binary starts in plugin mode from
//! `exec`, and the UI uses only relative URLs, since the node serves it under
//! `/t/<token>/`.

mod support;

use std::path::Path;

use support::Plugin;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn manifest() -> toml::Table {
    let text = std::fs::read_to_string(Path::new(ROOT).join("tracon-plugin.toml")).unwrap();
    toml::from_str(&text).unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn the_manifest_is_contract_1_with_a_ui_and_versioned_with_the_crate() {
    let m = manifest();
    let mut keys: Vec<&str> = m.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["contract", "exec", "name", "ui", "version"]);
    assert_eq!(m["name"].as_str(), Some("workboard"));
    assert_eq!(m["version"].as_str(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(m["contract"].as_integer(), Some(1));
    assert_eq!(
        m["exec"].as_array().unwrap(),
        &[
            toml::Value::from("bin/workboard"),
            toml::Value::from("serve")
        ]
    );
    let ui = m["ui"].as_table().unwrap();
    let mut ui_keys: Vec<&str> = ui.keys().map(String::as_str).collect();
    ui_keys.sort();
    assert_eq!(ui_keys, ["entry", "title"]);
    assert_eq!(ui["entry"].as_str(), Some("ui/index.html"));
    assert_eq!(ui["title"].as_str(), Some("Workboard"));
}

/// Every `src="..."` and `href="..."` in `html`.
fn references(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for attr in ["src=\"", "href=\""] {
        let mut rest = html;
        while let Some(at) = rest.find(attr) {
            rest = &rest[at + attr.len()..];
            let end = rest.find('"').unwrap();
            out.push(rest[..end].to_string());
            rest = &rest[end..];
        }
    }
    out
}

#[tokio::test]
async fn the_packaged_layout_starts_from_exec_and_its_ui_is_relative() {
    let stage = tempfile::tempdir().unwrap();
    let root = stage.path();
    std::fs::copy(
        Path::new(ROOT).join("tracon-plugin.toml"),
        root.join("tracon-plugin.toml"),
    )
    .unwrap();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::copy(support::BIN, root.join("bin/workboard")).unwrap();
    copy_dir(&Path::new(ROOT).join("spa/dist"), &root.join("ui"));

    let m = manifest();
    let entry = root.join(m["ui"]["entry"].as_str().unwrap());
    let html = std::fs::read_to_string(&entry).unwrap();
    assert!(
        !html.contains("<script>"),
        "no inline scripts under the CSP"
    );
    let refs = references(&html);
    assert!(refs.iter().any(|r| r.contains("assets/")), "{refs:?}");
    for r in &refs {
        assert!(r.starts_with("./"), "{r} is not relative");
        assert!(
            entry.parent().unwrap().join(r).is_file(),
            "{r} is not in ui/"
        );
    }
    for js in std::fs::read_dir(root.join("ui/assets")).unwrap() {
        let path = js.unwrap().path();
        if path.extension().is_some_and(|e| e == "js") {
            let code = std::fs::read_to_string(&path).unwrap();
            for absolute in ["\"/api", "'/api", "`/api"] {
                assert!(!code.contains(absolute), "{absolute} in {}", path.display());
            }
        }
    }

    let exec = root.join(m["exec"][0].as_str().unwrap());
    let run = tempfile::tempdir().unwrap();
    let plugin = Plugin::start_exec(&exec, run.path(), None).await;
    assert_eq!(plugin.task_count().await, 0);
}
