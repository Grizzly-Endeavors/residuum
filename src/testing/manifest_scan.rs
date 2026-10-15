//! Rules the workspace manifests keep, checked by reading `Cargo.toml`.
//!
//! The root package is also the workspace root. Members live in `crates/<dir>/`
//! and are read from every `crates/*/Cargo.toml`. Each rule is one test over
//! the root and every member, and a failure names the manifest and the key at
//! fault. Member-only rules have nothing to check until the first member exists.

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};

    use globset::GlobBuilder;
    use toml::{Table, Value};

    /// A manifest the rules read: the root, or one member.
    struct Manifest {
        /// Relative to the repository root, for messages.
        display: String,
        /// The member's directory name under `crates/`; `None` for the root.
        dir: Option<String>,
        table: Table,
    }

    struct Workspace {
        root: Manifest,
        /// Every directory under `crates/`, whether or not it holds a manifest.
        crate_dirs: Vec<String>,
        /// Every `crates/<dir>/Cargo.toml`, in directory order.
        members: Vec<Manifest>,
    }

    impl Workspace {
        fn load() -> Self {
            let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let root = read_manifest(&root_dir.join("Cargo.toml"), "Cargo.toml".to_owned(), None);
            let crate_dirs = list_crate_dirs(&root_dir.join("crates"));
            let members = crate_dirs
                .iter()
                .filter_map(|dir| {
                    let path = root_dir.join("crates").join(dir).join("Cargo.toml");
                    path.is_file().then(|| {
                        let display = format!("crates/{dir}/Cargo.toml");
                        read_manifest(&path, display, Some(dir.clone()))
                    })
                })
                .collect();
            Self {
                root,
                crate_dirs,
                members,
            }
        }

        /// The root's `[workspace.dependencies]` entry for `key`, if any.
        fn workspace_dependency(&self, key: &str) -> Option<&Table> {
            sub_table(&self.root.table, "workspace")
                .and_then(|workspace| sub_table(workspace, "dependencies"))
                .and_then(|deps| sub_table(deps, key))
        }
    }

    fn read_manifest(path: &Path, display: String, dir: Option<String>) -> Manifest {
        let text = fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
        let table: Table = toml::from_str(&text)
            .unwrap_or_else(|err| panic!("failed to parse {}: {err}", path.display()));
        Manifest {
            display,
            dir,
            table,
        }
    }

    fn list_crate_dirs(crates_dir: &Path) -> Vec<String> {
        if !crates_dir.is_dir() {
            return Vec::new();
        }
        let entries = fs::read_dir(crates_dir)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", crates_dir.display()));
        let mut dirs: Vec<String> = entries
            .map(|entry| {
                entry
                    .unwrap_or_else(|err| panic!("failed to read a crates/ entry: {err}"))
                    .path()
            })
            .filter(|path| path.is_dir())
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect();
        dirs.sort();
        dirs
    }

    fn sub_table<'a>(table: &'a Table, key: &str) -> Option<&'a Table> {
        table.get(key).and_then(Value::as_table)
    }

    /// Whether `value` is `{ workspace = true }`, the form that inherits from the workspace.
    fn inherits(value: Option<&Value>) -> bool {
        value
            .and_then(Value::as_table)
            .and_then(|table| table.get("workspace"))
            .and_then(Value::as_bool)
            == Some(true)
    }

    fn package_name(manifest: &Manifest) -> String {
        sub_table(&manifest.table, "package")
            .and_then(|package| package.get("name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    fn all(ws: &Workspace) -> impl Iterator<Item = &Manifest> {
        std::iter::once(&ws.root).chain(ws.members.iter())
    }

    const DEPENDENCY_KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

    /// One entry of a manifest's dependency tables, with what it resolves to.
    struct Dependency {
        key: String,
        /// The package the key names: the key itself, or `package = "…"` when set.
        package: String,
        /// The features the entry enables, including those its workspace entry enables.
        features: BTreeSet<String>,
        /// `dependencies`, `dev-dependencies` or `build-dependencies`.
        kind: &'static str,
        /// The table the entry sits in, for messages: `dev-dependencies`, `target.cfg(unix).dependencies`.
        table: String,
    }

    fn dependencies_of(ws: &Workspace, manifest: &Manifest) -> Vec<Dependency> {
        let mut sources: Vec<(&'static str, String, &Table)> = Vec::new();
        for kind in DEPENDENCY_KINDS {
            if let Some(deps) = sub_table(&manifest.table, kind) {
                sources.push((kind, kind.to_owned(), deps));
            }
        }
        if let Some(targets) = sub_table(&manifest.table, "target") {
            for (cfg, body) in targets {
                let Some(body) = body.as_table() else {
                    continue;
                };
                for kind in DEPENDENCY_KINDS {
                    if let Some(deps) = sub_table(body, kind) {
                        sources.push((kind, format!("target.{cfg}.{kind}"), deps));
                    }
                }
            }
        }

        let mut found = Vec::new();
        for (kind, table, deps) in sources {
            for (key, entry) in deps {
                found.push(dependency(ws, kind, &table, key, entry));
            }
        }
        found
    }

    fn dependency(
        ws: &Workspace,
        kind: &'static str,
        table: &str,
        key: &str,
        entry: &Value,
    ) -> Dependency {
        let own = entry.as_table();
        let inherited = if inherits(Some(entry)) {
            ws.workspace_dependency(key)
        } else {
            None
        };
        let package = own
            .and_then(|own_table| own_table.get("package"))
            .and_then(Value::as_str)
            .or_else(|| {
                inherited
                    .and_then(|inherited_table| inherited_table.get("package"))
                    .and_then(Value::as_str)
            })
            .unwrap_or(key)
            .to_owned();
        let mut features = BTreeSet::new();
        for source in [own, inherited].into_iter().flatten() {
            let listed = source
                .get("features")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str);
            for feature in listed {
                features.insert(feature.to_owned());
            }
        }
        Dependency {
            key: key.to_owned(),
            package,
            features,
            kind,
            table: table.to_owned(),
        }
    }

    /// Each workspace package's workspace dependencies, in the tables `kinds` names, read from the manifests.
    fn workspace_graph(ws: &Workspace, kinds: &[&str]) -> BTreeMap<String, BTreeSet<String>> {
        let names: BTreeSet<String> = all(ws).map(package_name).collect();
        all(ws)
            .map(|manifest| {
                let edges = dependencies_of(ws, manifest)
                    .into_iter()
                    .filter(|dep| kinds.contains(&dep.kind))
                    .map(|dep| dep.package)
                    .filter(|package| names.contains(package))
                    .collect();
                (package_name(manifest), edges)
            })
            .collect()
    }

    /// A cycle through `graph` as the packages it visits, first and last equal; a self-loop is a cycle.
    fn find_cycle(graph: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
        fn visit(
            node: &str,
            graph: &BTreeMap<String, BTreeSet<String>>,
            stack: &mut Vec<String>,
            done: &mut BTreeSet<String>,
        ) -> Option<Vec<String>> {
            if let Some(start) = stack.iter().position(|name| name == node) {
                let mut cycle = stack.get(start..)?.to_vec();
                cycle.push(node.to_owned());
                return Some(cycle);
            }
            if done.contains(node) {
                return None;
            }
            stack.push(node.to_owned());
            for next in graph.get(node).into_iter().flatten() {
                if let Some(cycle) = visit(next, graph, stack, done) {
                    return Some(cycle);
                }
            }
            stack.pop();
            done.insert(node.to_owned());
            None
        }

        let mut done = BTreeSet::new();
        for node in graph.keys() {
            let mut stack = Vec::new();
            if let Some(cycle) = visit(node, graph, &mut stack, &mut done) {
                return Some(cycle);
            }
        }
        None
    }

    fn workspace_patterns(ws: &Workspace, key: &str) -> Vec<String> {
        sub_table(&ws.root.table, "workspace")
            .and_then(|workspace| workspace.get(key))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    fn matches_any(patterns: &[String], path: &str) -> bool {
        patterns.iter().any(|pattern| {
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .build()
                .is_ok_and(|glob| glob.compile_matcher().is_match(path))
        })
    }

    fn assert_no_problems(rule: &str, problems: &[String]) {
        assert!(
            problems.is_empty(),
            "manifest_scan {rule}:\n{}",
            problems.join("\n")
        );
    }

    fn lints_problems(ws: &Workspace) -> Vec<String> {
        all(ws)
            .filter(|manifest| !inherits(manifest.table.get("lints")))
            .map(|manifest| {
                format!(
                    "{}: `[lints]` must set `workspace = true`, or the manifest gets rustc's and clippy's defaults",
                    manifest.display
                )
            })
            .collect()
    }

    fn member_name_problems(ws: &Workspace) -> Vec<String> {
        let mut problems = Vec::new();
        for member in &ws.members {
            let name = package_name(member);
            if !name.starts_with("residuum-") {
                problems.push(format!(
                    "{}: `package.name` is `{name}`; a member's name starts with `residuum-`",
                    member.display
                ));
            }
            if sub_table(&member.table, "lib").is_some_and(|lib| lib.contains_key("name")) {
                problems.push(format!(
                    "{}: `[lib] name` is set; a member's library takes its name from the package",
                    member.display
                ));
            }
        }
        problems
    }

    fn inherited_package_problems(ws: &Workspace) -> Vec<String> {
        let mut problems = Vec::new();
        for manifest in all(ws) {
            let package = sub_table(&manifest.table, "package");
            for key in ["version", "edition", "license", "publish"] {
                if !inherits(package.and_then(|table| table.get(key))) {
                    problems.push(format!(
                        "{}: `package.{key}` must be `{key}.workspace = true`",
                        manifest.display
                    ));
                }
            }
        }
        problems
    }

    fn test_support_in_normal_dependency_problems(ws: &Workspace) -> Vec<String> {
        let mut problems = Vec::new();
        for manifest in all(ws) {
            for dep in dependencies_of(ws, manifest) {
                if dep.kind == "dependencies" && dep.features.contains("test-support") {
                    problems.push(format!(
                        "{}: `[{}]` enables `test-support` on `{}`; enable it in `[dev-dependencies]` only",
                        manifest.display, dep.table, dep.key
                    ));
                }
            }
        }
        problems
    }

    fn member_directory_problems(ws: &Workspace) -> Vec<String> {
        let mut problems = Vec::new();
        for member in &ws.members {
            let name = package_name(member);
            if name.strip_prefix("residuum-") != member.dir.as_deref() {
                problems.push(format!(
                    "{}: `package.name` `{name}` must match the directory under `crates/` with its `residuum-` prefix removed",
                    member.display
                ));
            }
        }
        problems
    }

    fn member_pattern_problems(ws: &Workspace) -> Vec<String> {
        let members = workspace_patterns(ws, "members");
        let default_members = workspace_patterns(ws, "default-members");
        let mut problems = Vec::new();
        for dir in &ws.crate_dirs {
            let path = format!("crates/{dir}");
            if !matches_any(&members, &path) {
                problems.push(format!(
                    "Cargo.toml: `workspace.members` does not match `{path}`"
                ));
            }
            if !matches_any(&default_members, &path) {
                problems.push(format!(
                    "Cargo.toml: `workspace.default-members` does not match `{path}`"
                ));
            }
        }
        problems
    }

    fn workspace_cycle_problems(ws: &Workspace) -> Vec<String> {
        find_cycle(&workspace_graph(ws, &["dependencies", "dev-dependencies"]))
            .map(|cycle| {
                format!(
                    "workspace manifests: dependency cycle {} through normal and dev dependencies; Cargo compiles the crates on it twice, so their types stop unifying",
                    cycle.join(" -> ")
                )
            })
            .into_iter()
            .collect()
    }

    fn test_support_in_dev_dependency_problems(ws: &Workspace) -> Vec<String> {
        let test_support_packages: BTreeSet<String> = all(ws)
            .filter(|manifest| {
                sub_table(&manifest.table, "features")
                    .is_some_and(|features| features.contains_key("test-support"))
            })
            .map(package_name)
            .collect();
        let mut problems = Vec::new();
        for manifest in all(ws) {
            let deps = dependencies_of(ws, manifest);
            for dep in &deps {
                if !test_support_packages.contains(&dep.package) {
                    continue;
                }
                let enabled = deps.iter().any(|other| {
                    other.kind == "dev-dependencies"
                        && other.package == dep.package
                        && other.features.contains("test-support")
                });
                if !enabled {
                    problems.push(format!(
                        "{}: `[{}]` `{}` depends on `{}`, which has a `test-support` feature; enable it in `[dev-dependencies]`",
                        manifest.display, dep.table, dep.key, dep.package
                    ));
                }
            }
        }
        problems
    }

    #[test]
    fn manifest_rule_1_lints_inherit_from_workspace() {
        let ws = Workspace::load();
        assert_no_problems("rule 1", &lints_problems(&ws));
    }

    #[test]
    fn manifest_rule_2_member_names_and_libraries() {
        let ws = Workspace::load();
        assert_no_problems("rule 2", &member_name_problems(&ws));
    }

    #[test]
    fn manifest_rule_3_package_fields_inherit_from_workspace() {
        let ws = Workspace::load();
        assert_no_problems("rule 3", &inherited_package_problems(&ws));
    }

    #[test]
    fn manifest_rule_4_test_support_stays_out_of_normal_dependencies() {
        let ws = Workspace::load();
        assert_no_problems("rule 4", &test_support_in_normal_dependency_problems(&ws));
    }

    #[test]
    fn manifest_rule_5_workspace_graph_is_acyclic() {
        let ws = Workspace::load();
        assert_no_problems("rule 5", &workspace_cycle_problems(&ws));
    }

    #[test]
    fn manifest_rule_6_members_and_default_members_cover_each_crate() {
        let ws = Workspace::load();
        assert_no_problems("rule 6", &member_pattern_problems(&ws));
    }

    #[test]
    fn manifest_rule_7_member_directory_is_its_package_name() {
        let ws = Workspace::load();
        assert_no_problems("rule 7", &member_directory_problems(&ws));
    }

    #[test]
    fn manifest_rule_8_test_support_enabled_in_dev_dependencies() {
        let ws = Workspace::load();
        assert_no_problems("rule 8", &test_support_in_dev_dependency_problems(&ws));
    }

    /// The graph `scripts/affected-packages.sh --graph` prints for `graph`: one `package: deps` line per package.
    #[cfg(unix)]
    fn graph_text(graph: &BTreeMap<String, BTreeSet<String>>) -> String {
        let mut text = String::new();
        for (package, deps) in graph {
            text.push_str(package);
            text.push(':');
            for dep in deps {
                text.push(' ');
                text.push_str(dep);
            }
            text.push('\n');
        }
        text
    }

    /// The script reads the graph with awk; the manifests give it through `toml`. A
    /// mismatch means the package selection reads an edge the manifests do not have, or
    /// misses one they do. The script is a POSIX shell script, so the Windows run skips it.
    #[cfg(unix)]
    #[test]
    fn affected_packages_graph_matches_manifests() {
        use std::process::Command;

        let root_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let output = Command::new(root_dir.join("scripts/affected-packages.sh"))
            .arg("--graph")
            .current_dir(&root_dir)
            .output()
            .unwrap_or_else(|err| panic!("failed to run scripts/affected-packages.sh: {err}"));
        assert!(
            output.status.success(),
            "scripts/affected-packages.sh --graph failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let printed = String::from_utf8_lossy(&output.stdout).into_owned();

        let ws = Workspace::load();
        let expected = graph_text(&workspace_graph(&ws, &DEPENDENCY_KINDS));
        assert_eq!(
            printed, expected,
            "scripts/affected-packages.sh --graph disagrees with the workspace dependencies in the manifests"
        );
    }
}
