use std::path::Path;

const INTERNAL_DB_PATHS: &[&str] = &["memory/vectors.db", "team/vectors.db"];

#[must_use]
pub fn is_blocked_path(relative: &str) -> bool {
    if is_internal_data_path(relative) || is_last_known_good_copy(relative) {
        return true;
    }

    Path::new(relative)
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(residuum_core::util::fs::is_atomic_write_temp)
}

fn is_last_known_good_copy(relative: &str) -> bool {
    let path = Path::new(relative.trim_start_matches("./"));
    path.parent() == Some(Path::new("config"))
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".last-known-good.toml"))
}

fn is_internal_data_path(relative: &str) -> bool {
    if Path::new(relative)
        .components()
        .any(|c| c.as_os_str() == ".index")
    {
        return true;
    }

    let normalized = relative.trim_start_matches("./");
    INTERNAL_DB_PATHS.iter().any(|db| {
        normalized == *db
            || normalized == format!("{db}-wal")
            || normalized == format!("{db}-shm")
            || normalized == format!("{db}-journal")
    })
}

pub fn dir_holds_internal_data(dir: &Path, relative: &str) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let entry_relative = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        if is_internal_data_path(&entry_relative) {
            return Ok(true);
        }
        if entry.file_type()?.is_dir() && dir_holds_internal_data(&entry.path(), &entry_relative)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_index_segment_at_any_depth() {
        assert!(is_blocked_path(".index"));
        assert!(is_blocked_path(".index/foo"));
        assert!(is_blocked_path("memory/.index"));
        assert!(is_blocked_path("memory/.index/segments/foo.bin"));
        assert!(is_blocked_path("deep/nested/path/.index/file"));
    }

    #[test]
    fn blocks_residuums_own_database_and_sidecars() {
        assert!(is_blocked_path("memory/vectors.db"));
        assert!(is_blocked_path("memory/vectors.db-wal"));
        assert!(is_blocked_path("memory/vectors.db-shm"));
        assert!(is_blocked_path("memory/vectors.db-journal"));
    }

    #[test]
    fn blocks_the_team_layers_database_and_index() {
        assert!(is_blocked_path("team/vectors.db"));
        assert!(is_blocked_path("team/vectors.db-wal"));
        assert!(is_blocked_path("team/.index/segments/foo.bin"));
        assert!(!is_blocked_path("team/wiki/vectors.db"));
    }

    #[test]
    fn blocks_last_known_good_copies_in_the_config_directory() {
        assert!(is_blocked_path("config/config.last-known-good.toml"));
        assert!(is_blocked_path("config/providers.last-known-good.toml"));
        assert!(is_blocked_path("./config/providers.last-known-good.toml"));
        assert!(!is_blocked_path("notes/config.last-known-good.toml"));
        assert!(!is_blocked_path("config/mcp.json"));
    }

    #[test]
    fn a_users_own_db_or_sqlite_file_is_not_blocked() {
        assert!(!is_blocked_path("data.db"));
        assert!(!is_blocked_path("store.sqlite"));
        assert!(!is_blocked_path("notes/vectors.db"));
        assert!(!is_blocked_path("store.sqlite-wal"));
    }

    #[test]
    fn finds_internal_data_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("memory/.index")).unwrap();
        std::fs::create_dir_all(dir.path().join("notes/deep")).unwrap();
        std::fs::write(dir.path().join("notes/deep/page.md"), "x").unwrap();
        std::fs::create_dir_all(dir.path().join("memory")).unwrap();
        std::fs::write(dir.path().join("memory/vectors.db-wal"), "x").unwrap();

        assert!(dir_holds_internal_data(&dir.path().join("memory"), "memory").unwrap());
        assert!(!dir_holds_internal_data(&dir.path().join("notes"), "notes").unwrap());
    }

    #[test]
    fn a_users_own_database_elsewhere_is_not_internal_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        std::fs::write(dir.path().join("data/store.sqlite"), "x").unwrap();

        assert!(!dir_holds_internal_data(&dir.path().join("data"), "data").unwrap());
    }

    #[test]
    fn in_flight_writes_are_not_internal_data() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".page.md.0badf00d.residuum-tmp"), "x").unwrap();
        assert!(!dir_holds_internal_data(dir.path(), "").unwrap());
    }

    #[test]
    fn blocks_in_flight_atomic_write_temp_files() {
        assert!(is_blocked_path("wiki/.page.md.0badf00d.residuum-tmp"));
        assert!(!is_blocked_path("wiki/.page.md.tmp"));
    }

    #[test]
    fn does_not_block_look_alike_names() {
        assert!(!is_blocked_path("index.md"));
        assert!(!is_blocked_path("my.index.md"));
        assert!(!is_blocked_path("wiki/index.md"));
        assert!(!is_blocked_path("skills/index.md"));
        assert!(!is_blocked_path("SOUL.md"));
        assert!(!is_blocked_path("skills/research.md"));
        assert!(!is_blocked_path(""));
    }

    #[test]
    fn does_not_block_files_that_merely_contain_db_or_sqlite() {
        assert!(!is_blocked_path("database.md"));
        assert!(!is_blocked_path("sqlite-notes.md"));
    }
}
