//! Types shared by the remote-access pieces.

/// Which of an instance's three names a host is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostKind {
    /// `{user}.{base}`: the web UI.
    Ui,
    /// `{user}.workbench.{base}`: workbench artifacts.
    Workbench,
    /// `{instance}.{user}.{base}`: public A2A and Teams for this instance.
    Instance,
}

/// The three host names an instance answers for, always derived locally from
/// the stored user and slug and the configured base domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hostnames {
    pub(crate) ui: String,
    pub(crate) workbench: String,
    pub(crate) instance: String,
}

impl Hostnames {
    /// Derive the names for `user`'s instance `slug` under `base`.
    pub(crate) fn derive(user: &str, slug: &str, base: &str) -> Self {
        Self {
            ui: format!("{user}.{base}"),
            workbench: format!("{user}.workbench.{base}"),
            instance: format!("{slug}.{user}.{base}"),
        }
    }

    /// All three names, UI first.
    pub(crate) fn all(&self) -> [&str; 3] {
        [&self.ui, &self.workbench, &self.instance]
    }

    /// Which name `host` is, comparing case-insensitively and ignoring a
    /// trailing dot and any port.
    pub(crate) fn kind_of(&self, host: &str) -> Option<HostKind> {
        let host = normalize_host(host);
        if host == self.ui {
            Some(HostKind::Ui)
        } else if host == self.workbench {
            Some(HostKind::Workbench)
        } else if host == self.instance {
            Some(HostKind::Instance)
        } else {
            None
        }
    }
}

/// Lowercase `host`, drop a port and one trailing dot.
pub(crate) fn normalize_host(host: &str) -> String {
    let without_port = match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) && !name.contains(':') => {
            name
        }
        _ => host,
    };
    without_port
        .strip_suffix('.')
        .unwrap_or(without_port)
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_names_and_kinds() {
        let h = Hostnames::derive("bear", "laptop", "agent-residuum.com");
        assert_eq!(h.ui, "bear.agent-residuum.com");
        assert_eq!(h.workbench, "bear.workbench.agent-residuum.com");
        assert_eq!(h.instance, "laptop.bear.agent-residuum.com");
        assert_eq!(h.kind_of("BEAR.agent-residuum.com."), Some(HostKind::Ui));
        assert_eq!(
            h.kind_of("laptop.bear.agent-residuum.com:443"),
            Some(HostKind::Instance)
        );
        assert_eq!(h.kind_of("evil.agent-residuum.com"), None);
    }
}
