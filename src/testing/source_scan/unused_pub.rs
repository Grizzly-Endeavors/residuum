//! `pub` items of member crates that nothing in the workspace uses outside
//! their own crate's tests. rustc's `dead_code` lint never fires on a `pub`
//! item of a library, since some other crate might use it, so a member's item
//! whose last caller went away gets no warning.
//!
//! The candidates are the items declared exactly `pub` (not `pub(crate)`,
//! `pub(super)` or `pub(in …)`) in a member's `src/`, in production code or in
//! test-support code (`#[cfg(any(test, feature = "test-support"))]`, which
//! exists for other crates' tests): `fn`, `struct`, `enum`, `trait`, `type`,
//! `union`, `const` and `static` items, and the `pub fn` and `pub const` items
//! of inherent `impl` blocks. Trait impl items, fields, enum variants,
//! `pub mod`, `pub use`, `macro_rules!` bodies and lines inside string
//! literals are not candidates.
//!
//! An item counts as used when its name appears as a whole identifier in the
//! code of any scanned file (the root's `src/` and `tests/`, `build.rs`, every
//! member's sources), with `//` comments removed and string literals kept,
//! since serde attributes name functions in strings. Three kinds of line do
//! not count: the item's own declaration line, its own crate's `#[cfg(test)]`
//! code, and `use` and `pub use` statements in its own crate, since an import
//! or a re-export inside the crate calls nothing. An import in another crate
//! counts. The source scans' own files are not read for uses, since their
//! sample lines and anchors name items in strings.
//!
//! The match goes by name alone, so it errs toward "used":
//! - A name some other item or binding shares (`new`, `load`, `name`) counts as
//!   used wherever it appears, so an unused item with a common name is not
//!   reported.
//! - A mention in the crate's own production code counts, including the
//!   item's own body and its `impl` blocks, so a type that only its own
//!   `impl` names is not reported.
//! - `/* */` comments count as code.
//!
//! Two cases are reported although the item is used: an item other crates
//! reach only under a re-export's new name (`pub use inner::Item as Other;`),
//! and one named only by an identifier a macro builds from pieces. For the
//! first, re-export or import the item under its own name, or rename the item
//! to the name its users call.

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;

use super::{SourceFile, item_end, starts_inside_literal, without_comment};

/// A member crate's `pub` item that nothing outside its crate's tests uses.
#[derive(Debug)]
struct UnusedPub {
    path: String,
    /// 1-based.
    line: usize,
    name: String,
    /// Whether its own crate's tests use it, rather than nothing at all.
    own_tests_use_it: bool,
}

impl UnusedPub {
    fn describe(&self) -> String {
        let users = if self.own_tests_use_it {
            "only its own crate's tests use it"
        } else {
            "nothing uses it"
        };
        format!("{}:{}: `{}`: {users}", self.path, self.line, self.name)
    }
}

/// A `pub` item declared in a member crate.
struct Declared<'a> {
    crate_dir: &'a str,
    path: &'a str,
    /// 1-based.
    line: usize,
    name: &'a str,
}

/// A line that names a candidate.
struct Mention<'a> {
    crate_dir: Option<&'a str>,
    path: &'a str,
    /// 1-based.
    line: usize,
    /// The line is own-test code of its crate.
    own_test: bool,
    /// The line is part of a `use` statement.
    import: bool,
}

/// The directory under `crates/` of the member crate `path` belongs to; `None`
/// for the root package.
fn member_crate(path: &str) -> Option<&str> {
    path.strip_prefix("crates/")?.split('/').next()
}

/// A `pub` item declaration; the name is in group 1 or group 2.
fn pub_item() -> Regex {
    Regex::new(concat!(
        r#"^\s*pub\s+(?:(?:const|async|unsafe)\s+)*(?:extern\s+"[^"]*"\s+)?(?:fn|struct|enum|trait|type|union)\s+([A-Za-z_][A-Za-z0-9_]*)"#,
        r"|^\s*pub\s+(?:const|static)\s+(?:mut\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*:",
    ))
    .unwrap()
}

/// The `pub` items `sources` declares in member crates' `src/`, outside their
/// own-test code.
fn declared_pub_items(sources: &[SourceFile]) -> Vec<Declared<'_>> {
    let declaration = pub_item();
    let mut items = Vec::new();
    for file in sources {
        let Some(crate_dir) = member_crate(&file.path) else {
            continue;
        };
        if !file.path.starts_with(&format!("crates/{crate_dir}/src/")) {
            continue;
        }
        let in_literal = starts_inside_literal(&file.lines);
        let mut macro_end = None;
        for (idx, (line, own_test)) in file.lines.iter().zip(&file.own_test).enumerate() {
            if macro_end.is_some_and(|end| idx <= end) {
                continue;
            }
            if in_literal.get(idx).copied().unwrap_or(false) {
                continue;
            }
            if line.trim_start().starts_with("macro_rules!") {
                macro_end = Some(item_end(&file.lines, &in_literal, idx));
                continue;
            }
            if *own_test {
                continue;
            }
            let Some(code) = without_comment(line) else {
                continue;
            };
            if let Some(caps) = declaration.captures(code)
                && let Some(name) = caps.get(1).or_else(|| caps.get(2))
            {
                items.push(Declared {
                    crate_dir,
                    path: &file.path,
                    line: idx + 1,
                    name: name.as_str(),
                });
            }
        }
    }
    items
}

/// Whether `path` is one of the source scans' own files, whose string
/// literals are sample lines and anchors that name items without using them.
fn is_scan_sample(path: &str) -> bool {
    path == "src/testing/source_scan.rs" || path.starts_with("src/testing/source_scan/")
}

/// Every line of `sources` that names one of `names`, by name, outside the
/// source scans' own files.
fn mentions<'a>(
    sources: &'a [SourceFile],
    names: &BTreeSet<&'a str>,
) -> BTreeMap<&'a str, Vec<Mention<'a>>> {
    let identifier = Regex::new("[A-Za-z_][A-Za-z0-9_]*").unwrap();
    let use_start = Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?use\s").unwrap();
    let mut found: BTreeMap<&str, Vec<Mention<'_>>> = BTreeMap::new();
    for file in sources {
        if is_scan_sample(&file.path) {
            continue;
        }
        let crate_dir = member_crate(&file.path);
        let in_literal = starts_inside_literal(&file.lines);
        let mut in_use = false;
        for (idx, (line, own_test)) in file.lines.iter().zip(&file.own_test).enumerate() {
            let Some(code) = without_comment(line) else {
                continue;
            };
            // A string literal's continuation line can start with `use `.
            let starts_in_literal = in_literal.get(idx).copied().unwrap_or(false);
            in_use = in_use || (!starts_in_literal && use_start.is_match(code));
            let import = in_use;
            if code.contains(';') {
                in_use = false;
            }
            for word in identifier.find_iter(code) {
                if let Some(name) = names.get(word.as_str()) {
                    found.entry(*name).or_default().push(Mention {
                        crate_dir,
                        path: &file.path,
                        line: idx + 1,
                        own_test: *own_test,
                        import,
                    });
                }
            }
        }
    }
    found
}

/// The `pub` items of member crates in `sources` that nothing outside their
/// own crate's tests uses, in file and line order.
fn unused_pub_items(sources: &[SourceFile]) -> Vec<UnusedPub> {
    let items = declared_pub_items(sources);
    let names: BTreeSet<&str> = items.iter().map(|item| item.name).collect();
    let by_name = mentions(sources, &names);
    let mut unused = Vec::new();
    for item in &items {
        let elsewhere: Vec<&Mention<'_>> = by_name
            .get(item.name)
            .into_iter()
            .flatten()
            .filter(|mention| mention.path != item.path || mention.line != item.line)
            .collect();
        let used = elsewhere.iter().any(|mention| {
            mention.crate_dir != Some(item.crate_dir) || (!mention.own_test && !mention.import)
        });
        if !used {
            unused.push(UnusedPub {
                path: item.path.to_string(),
                line: item.line,
                name: item.name.to_string(),
                own_tests_use_it: elsewhere.iter().any(|mention| mention.own_test),
            });
        }
    }
    unused
}

#[cfg(test)]
mod tests {
    use super::super::crate_root;
    use super::*;

    /// `build.rs`, which no scanned root holds: production code throughout.
    fn build_script() -> SourceFile {
        let text = std::fs::read_to_string(crate_root().join("build.rs")).unwrap();
        let lines = text.lines().map(str::to_string).collect();
        SourceFile::new("build.rs".to_string(), lines, false, false)
    }

    fn fixture(path: &str, text: &str) -> SourceFile {
        let lines = text.lines().map(str::to_string).collect();
        SourceFile::new(path.to_string(), lines, false, false)
    }

    /// A member crate's `pub` item that nothing outside its own tests uses
    /// is dead, and rustc does not say so.
    #[test]
    fn every_pub_item_of_a_member_crate_has_a_user() {
        let mut sources = super::super::crate_sources();
        sources.push(build_script());
        let unused: Vec<String> = unused_pub_items(&sources)
            .iter()
            .map(UnusedPub::describe)
            .collect();
        assert!(
            unused.is_empty(),
            "these `pub` items of member crates have no user outside their own crate's tests, and rustc does not warn about an unused `pub` item of a library. Delete an item nothing uses, with the tests that exist only for it. A test helper only its own crate's tests use moves into `#[cfg(test)]` code as `pub(crate)` or private; production code only its own tests call goes, with those tests. If the item is used, but only under a name a `use … as …` gives it, re-export or import it under its own name, or rename the item to the name its users call. The rule and its blind spots are described in src/testing/source_scan/unused_pub.rs.\n{}",
            unused.join("\n")
        );
    }

    /// The first crate of the counting fixture: one `pub` item for each way a mention does or does not count.
    const ALPHA_LIB_ZQ: &str = r#"
pub use inner::{
    reexported_only_zq,
};

pub fn unused_zq() {}
pub fn own_tests_only_zq() {}
pub fn commented_only_zq() {}
pub fn used_by_beta_zq() {
    used_in_production_zq();
}
pub fn used_in_production_zq() {}
pub fn named_in_string_zq() {}
pub fn imported_by_beta_zq() {}
pub fn used_by_root_zq() {}
pub async fn unused_async_zq() {}
pub const fn unused_const_fn_zq() {}
pub struct UnusedTypeZq;
pub const UNUSED_CONST_ZQ: u8 = 1;
pub static UNUSED_STATIC_ZQ: u8 = 1;
pub(crate) fn restricted_zq() {}

#[derive(serde::Serialize)]
pub struct PayloadZq {
    #[serde(serialize_with = "named_in_string_zq")]
    pub field: u8,
}

impl PayloadZq {
    pub fn unused_method_zq(&self) {}
}

#[cfg(any(test, feature = "test-support"))]
pub fn support_used_by_beta_tests_zq() {
    support_dependency_zq();
}

#[cfg(any(test, feature = "test-support"))]
pub fn support_dependency_zq() {}

#[cfg(any(test, feature = "test-support"))]
pub fn unused_support_zq() {}

pub const SAMPLE_ZQ: &str = "
pub fn inside_a_literal_zq() {}
";

pub fn used_after_a_use_string_zq() -> u8 { 0 }

fn message_zq() -> String {
    format!(
        "a wrapped message \
         use this wording {}",
        used_after_a_use_string_zq(),
    )
}

macro_rules! make_zq {
    () => {
        pub fn made_by_a_macro_zq() {}
    };
}

#[cfg(test)]
mod tests {
    pub fn own_test_item_zq() {}

    fn check() {
        super::own_tests_only_zq();
    }
}

mod inner {
    pub fn reexported_only_zq() {}
}
"#;

    #[test]
    fn the_unused_pub_rule_counts_other_crates_and_production_code_only() {
        let sources = [
            fixture("crates/alpha/src/lib.rs", ALPHA_LIB_ZQ),
            fixture(
                "crates/alpha/src/other.rs",
                "// commented_only_zq is named here\nlet note = 1; // and here: commented_only_zq\n",
            ),
            fixture(
                "crates/beta/src/lib.rs",
                "use alpha::imported_by_beta_zq;\n\nfn beta() {\n    alpha::used_by_beta_zq();\n    let _ = alpha::SAMPLE_ZQ;\n}\n\n#[cfg(test)]\nmod tests {\n    fn check() {\n        alpha::support_used_by_beta_tests_zq();\n    }\n}\n",
            ),
            fixture(
                "src/lib.rs",
                "fn root() {\n    alpha::used_by_root_zq();\n}\n",
            ),
        ];
        let unused = unused_pub_items(&sources);
        let flagged: Vec<(&str, bool)> = unused
            .iter()
            .map(|item| (item.name.as_str(), item.own_tests_use_it))
            .collect();
        assert_eq!(
            flagged,
            [
                ("unused_zq", false),
                ("own_tests_only_zq", true),
                ("commented_only_zq", false),
                ("unused_async_zq", false),
                ("unused_const_fn_zq", false),
                ("UnusedTypeZq", false),
                ("UNUSED_CONST_ZQ", false),
                ("UNUSED_STATIC_ZQ", false),
                ("unused_method_zq", false),
                ("unused_support_zq", false),
                ("reexported_only_zq", false),
            ],
            "flagged: {unused:#?}"
        );
        let first = unused.first().unwrap();
        assert_eq!(
            (first.path.as_str(), first.line),
            ("crates/alpha/src/lib.rs", 6),
            "a finding names its declaration's file and 1-based line"
        );
    }

    #[test]
    fn a_pub_item_declaration_names_the_item() {
        let declaration = pub_item();
        for (line, name) in [
            ("pub fn load(path: &Path) -> Self {", "load"),
            ("    pub async fn run(&self) {", "run"),
            ("pub const fn limit() -> usize {", "limit"),
            ("pub unsafe extern \"C\" fn init() {", "init"),
            ("pub struct Config {", "Config"),
            ("pub enum Mode {", "Mode"),
            ("pub trait Sink: Send {", "Sink"),
            (
                "pub type Result<T> = std::result::Result<T, Error>;",
                "Result",
            ),
            ("pub union Bits {", "Bits"),
            ("    pub const LIMIT: usize = 3;", "LIMIT"),
            ("pub static mut COUNT: u8 = 0;", "COUNT"),
        ] {
            let caps = declaration
                .captures(line)
                .unwrap_or_else(|| panic!("{line:?} declares a pub item"));
            let found = caps.get(1).or_else(|| caps.get(2)).unwrap().as_str();
            assert_eq!(found, name, "{line:?} declares {name}");
        }
        for line in [
            "pub(crate) fn helper() {",
            "pub(super) struct Inner;",
            "pub(in crate::a) fn scoped() {",
            "pub mod paths;",
            "pub use inner::Item;",
            "    pub name: String,",
            "    pub r#type: String,",
            "fn private() {}",
            "    fn trait_method(&self) {}",
        ] {
            assert!(
                !declaration.is_match(line),
                "{line:?} is not a pub item declaration"
            );
        }
    }
}
