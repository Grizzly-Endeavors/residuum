//! Test harness that runs the Pebble ACME test CA and its challenge test
//! server in Docker, for tests that need a real CA.
//!
//! Tests that use it are `#[ignore]`d and run with
//! `cargo test --quiet pebble -- --ignored`; they need Docker and pull the images on first use.
//!
//! To move to a newer Pebble: pick the release at
//! <https://github.com/letsencrypt/pebble/releases>, change [`PEBBLE_VERSION`]
//! (the image tag has no leading `v`), and replace `pebble.minica.pem` with
//! `test/certs/pebble.minica.pem` from that tag. The CA's own TLS certificate
//! (`test/certs/localhost` in the image) must still be signed by that root and
//! name `127.0.0.1`.

use std::net::{SocketAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tracing::debug;

use super::acme::AcmeSettings;
use super::tls::CertResolver;

/// Pinned Pebble release; both images share the tag.
const PEBBLE_VERSION: &str = "2.10.1";
const PEBBLE_IMAGE: &str = "ghcr.io/letsencrypt/pebble:2.10.1";
const CHALLTESTSRV_IMAGE: &str = "ghcr.io/letsencrypt/pebble-challtestsrv:2.10.1";

/// The root that signs Pebble's own HTTPS certificate (`test/certs/pebble.minica.pem` at [`PEBBLE_VERSION`]).
const PEBBLE_MINICA_PEM: &str = include_str!("pebble.minica.pem");

static CONTAINER_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// A running Pebble CA plus a challenge test server acting as its DNS resolver.
///
/// Both run with host networking on freshly chosen free ports, so tests can run in parallel.
/// Validation connections for TLS-ALPN-01 go to `127.0.0.1:tls_port()`; every
/// name resolves to 127.0.0.1 unless the test says otherwise. The containers are removed on drop.
pub(crate) struct PebbleHarness {
    dir: tempfile::TempDir,
    containers: Vec<String>,
    acme_port: u16,
    management_port: u16,
    dns_port: u16,
    tls_port: u16,
    root_ca: PathBuf,
}

impl PebbleHarness {
    /// Start both containers and wait until they accept connections.
    pub(crate) async fn start() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir().context("failed to create the harness directory")?;
        let acme_port = free_port()?;
        let pebble_management_port = free_port()?;
        let http_port = free_port()?;
        let tls_port = free_port()?;
        let management_port = free_port()?;
        let dns_port = free_dns_port()?;

        let root_ca = dir.path().join("pebble.minica.pem");
        std::fs::write(&root_ca, PEBBLE_MINICA_PEM)
            .context("failed to write the Pebble root CA")?;

        // Paths are inside the image: it carries Pebble's own test certificates.
        let config = serde_json::json!({
            "pebble": {
                "listenAddress": format!("127.0.0.1:{acme_port}"),
                "managementListenAddress": format!("127.0.0.1:{pebble_management_port}"),
                "certificate": "test/certs/localhost/cert.pem",
                "privateKey": "test/certs/localhost/key.pem",
                "httpPort": http_port,
                "tlsPort": tls_port,
                "ocspResponderURL": "",
                "externalAccountBindingRequired": false,
                "domainBlocklist": ["blocked-domain.example"],
                "retryAfter": { "authz": 1, "order": 1 },
                "keyAlgorithm": "ecdsa",
                "profiles": {
                    "default": { "description": "default", "validityPeriod": 7_776_000 }
                }
            }
        });
        let config_path = dir.path().join("pebble-config.json");
        std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)
            .context("failed to write the Pebble configuration")?;
        set_world_readable(dir.path(), &config_path)?;

        let sequence = CONTAINER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let unique = format!(
            "residuum-test-{}-{sequence}-{:08x}",
            std::process::id(),
            rand::random::<u32>()
        );
        let mut harness = Self {
            dir,
            containers: Vec::new(),
            acme_port,
            management_port,
            dns_port,
            tls_port,
            root_ca,
        };

        let dns_bind = format!("127.0.0.1:{dns_port}");
        let management_bind = format!("127.0.0.1:{management_port}");
        // The test listens on the TLS-ALPN-01 port itself; the other challenge servers are off.
        let challtestsrv = Self::run_container(
            &format!("{unique}-dns"),
            CHALLTESTSRV_IMAGE,
            &[],
            &[
                "-management",
                &management_bind,
                "-dnsserver",
                &dns_bind,
                "-defaultIPv6",
                "",
                "-http01",
                "",
                "-https01",
                "",
                "-tlsalpn01",
                "",
                "-doh",
                "",
            ],
        )?;
        harness.containers.push(challtestsrv);

        let mount = format!("{}:/pebble-config.json:ro", config_path.display());
        let pebble = Self::run_container(
            &format!("{unique}-ca"),
            PEBBLE_IMAGE,
            &[
                "-e",
                "PEBBLE_VA_NOSLEEP=1",
                "-e",
                "PEBBLE_AUTHZREUSE=0",
                "-v",
                &mount,
            ],
            &["-config", "/pebble-config.json", "-dnsserver", &dns_bind],
        )?;
        harness.containers.push(pebble);

        for (what, port) in [
            ("Pebble", acme_port),
            ("challenge test server", management_port),
        ] {
            harness.wait_for_port(what, port).await?;
        }
        Ok(harness)
    }

    /// The CA's ACME directory URL.
    pub(crate) fn directory_url(&self) -> String {
        format!("https://127.0.0.1:{}/dir", self.acme_port)
    }

    /// Root certificate that signs the CA's HTTPS certificate.
    pub(crate) fn root_ca_pem_path(&self) -> &Path {
        &self.root_ca
    }

    /// Address of the test DNS server (UDP and TCP).
    pub(crate) fn dns_addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.dns_port))
    }

    /// Port Pebble connects to for TLS-ALPN-01 validation.
    pub(crate) fn tls_port(&self) -> u16 {
        self.tls_port
    }

    /// ACME settings pointing at this CA, storing state under `state_dir`.
    pub(crate) fn acme_settings(&self, state_dir: &Path) -> AcmeSettings {
        AcmeSettings {
            directory_url: self.directory_url(),
            root_ca_pem: Some(self.root_ca.clone()),
            state_dir: state_dir.to_path_buf(),
        }
    }

    /// A scratch directory that lives as long as the harness.
    pub(crate) fn scratch_dir(&self) -> &Path {
        self.dir.path()
    }

    /// Make `host` resolve to 127.0.0.1.
    pub(crate) async fn add_a(&self, host: &str) -> anyhow::Result<()> {
        self.manage(
            "add-a",
            &serde_json::json!({ "host": host, "addresses": ["127.0.0.1"] }),
        )
        .await
    }

    /// Publish a CAA `issue` record with each value in `issue_values` for `host`.
    pub(crate) async fn set_caa(&self, host: &str, issue_values: &[&str]) -> anyhow::Result<()> {
        let policies: Vec<_> = issue_values
            .iter()
            .map(|value| serde_json::json!({ "tag": "issue", "value": value }))
            .collect();
        self.manage(
            "add-caa",
            &serde_json::json!({ "host": host, "policies": policies }),
        )
        .await
    }

    /// Answer TLS-ALPN-01 validation connections on `tls_port()` with `resolver`'s server configuration.
    pub(crate) fn serve_tls_alpn(
        &self,
        resolver: &Arc<CertResolver>,
    ) -> anyhow::Result<TlsAlpnListener> {
        let listener = TcpListener::bind(("127.0.0.1", self.tls_port())).with_context(|| {
            format!(
                "failed to listen on the TLS-ALPN-01 port {}",
                self.tls_port()
            )
        })?;
        listener.set_nonblocking(true)?;
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let acceptor = TlsAcceptor::from(resolver.server_config()?);
        let task = crate::util::spawn_in_span(async move {
            loop {
                let Ok((stream, peer)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                crate::util::spawn_in_span(async move {
                    match acceptor.accept(stream).await {
                        Ok(_) => debug!(%peer, "TLS-ALPN-01 handshake served"),
                        Err(error) => debug!(%peer, %error, "TLS-ALPN-01 handshake failed"),
                    }
                });
            }
        });
        Ok(TlsAlpnListener { task })
    }

    fn run_container(
        name: &str,
        image: &str,
        docker_args: &[&str],
        app_args: &[&str],
    ) -> anyhow::Result<String> {
        let output = docker(
            ["run", "-d", "--rm", "--name", name, "--network", "host"]
                .into_iter()
                .chain(docker_args.iter().copied())
                .chain(std::iter::once(image))
                .chain(app_args.iter().copied()),
        )?;
        anyhow::ensure!(
            output.status.success(),
            "docker run {image} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(name.to_owned())
    }

    async fn wait_for_port(&self, what: &str, port: u16) -> anyhow::Result<()> {
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let logs: Vec<String> = self
            .containers
            .iter()
            .map(|name| {
                let output = docker(["logs", "--tail", "20", name]);
                match output {
                    Ok(o) => format!(
                        "{name}: {}{}",
                        String::from_utf8_lossy(&o.stdout),
                        String::from_utf8_lossy(&o.stderr)
                    ),
                    Err(error) => format!("{name}: {error:#}"),
                }
            })
            .collect();
        anyhow::bail!(
            "{what} did not start listening on port {port}\n{}",
            logs.join("\n")
        )
    }

    /// POST `body` to the challenge test server's management API.
    async fn manage(&self, endpoint: &str, body: &serde_json::Value) -> anyhow::Result<()> {
        let body = serde_json::to_vec(body)?;
        let mut stream = TcpStream::connect(("127.0.0.1", self.management_port))
            .await
            .context("failed to reach the challenge test server")?;
        let head = format!(
            "POST /{endpoint} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(&body).await?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        let response = String::from_utf8_lossy(&response);
        let status_line = response.lines().next().unwrap_or_default();
        anyhow::ensure!(
            status_line.contains(" 200"),
            "challenge test server rejected /{endpoint}: {status_line}"
        );
        Ok(())
    }
}

impl Drop for PebbleHarness {
    fn drop(&mut self) {
        if self.containers.is_empty() {
            return;
        }
        let names = self.containers.iter().map(String::as_str);
        if let Err(error) = docker(["rm", "-f"].into_iter().chain(names)) {
            tracing::warn!(error = %format!("{error:#}"), containers = ?self.containers, "failed to remove Pebble containers");
        }
    }
}

/// Stops the TLS-ALPN-01 listener when dropped.
pub(crate) struct TlsAlpnListener {
    task: JoinHandle<()>,
}

impl Drop for TlsAlpnListener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn docker<'a>(args: impl IntoIterator<Item = &'a str>) -> anyhow::Result<Output> {
    Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("failed to run docker (these tests need Docker)")
}

fn free_port() -> anyhow::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

/// A port that is free for both UDP and TCP, since the DNS server uses both.
fn free_dns_port() -> anyhow::Result<u16> {
    for _ in 0..20 {
        let port = free_port()?;
        if UdpSocket::bind(("127.0.0.1", port)).is_ok() {
            return Ok(port);
        }
    }
    anyhow::bail!("no port free for both UDP and TCP")
}

/// The container runs as another user, so it must be able to read the mounted configuration.
#[cfg_attr(
    not(unix),
    expect(
        clippy::unnecessary_wraps,
        reason = "mirrors the unix signature, where setting permissions can fail"
    )
)]
fn set_world_readable(dir: &Path, file: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))?;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644))?;
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, file);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_images_use_the_pinned_version() {
        assert!(PEBBLE_IMAGE.ends_with(&format!(":{PEBBLE_VERSION}")));
        assert!(CHALLTESTSRV_IMAGE.ends_with(&format!(":{PEBBLE_VERSION}")));
    }

    #[test]
    fn embedded_root_is_a_certificate() {
        assert!(PEBBLE_MINICA_PEM.starts_with("-----BEGIN CERTIFICATE-----"));
    }
}
