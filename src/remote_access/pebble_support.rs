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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context as _;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolveHosts, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RecordType;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tracing::debug;

use super::acme::AcmeSettings;
use super::tls::CertResolver;
use crate::testing::ports::{ReservedPort, reserve_port};
use crate::testing::wait;

/// Pinned Pebble release; both images share the tag.
const PEBBLE_VERSION: &str = "2.10.1";
const PEBBLE_IMAGE: &str = "ghcr.io/letsencrypt/pebble:2.10.1";
const CHALLTESTSRV_IMAGE: &str = "ghcr.io/letsencrypt/pebble-challtestsrv:2.10.1";

/// The root that signs Pebble's own HTTPS certificate (`test/certs/pebble.minica.pem` at [`PEBBLE_VERSION`]).
const PEBBLE_MINICA_PEM: &str = include_str!("pebble.minica.pem");

static CONTAINER_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// A running Pebble CA plus a challenge test server acting as its DNS resolver.
///
/// Both run with host networking on ports reserved through
/// [`crate::testing::ports`], below the OS's ephemeral range, so tests can run
/// in parallel: each reservation is held until just before the container that
/// binds the port starts. Validation connections for TLS-ALPN-01 go to `127.0.0.1:tls_port()`; every
/// name resolves to 127.0.0.1 unless the test says otherwise. The containers are removed on drop.
pub(crate) struct PebbleHarness {
    dir: tempfile::TempDir,
    containers: Vec<String>,
    acme_port: u16,
    management_port: u16,
    dns_port: u16,
    tls_port: u16,
    /// Holds `tls_port` until the test binds it, see [`Self::release_tls_port`].
    tls_reservation: Mutex<Option<ReservedPort>>,
    /// Pebble's HTTP-01 port. No test answers HTTP-01, so nothing ever binds
    /// it; the reservation keeps it for the harness's lifetime so the number
    /// Pebble is configured with stays this harness's.
    _http_reservation: ReservedPort,
    root_ca: PathBuf,
}

impl PebbleHarness {
    /// Start both containers and wait until they accept connections.
    pub(crate) async fn start() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir().context("failed to create the harness directory")?;
        let acme = reserve_port();
        let acme_port = acme.port();
        let pebble_management = reserve_port();
        let http = reserve_port();
        let tls = reserve_port();
        let tls_port = tls.port();
        let challenge_management = reserve_port();
        let management_port = challenge_management.port();
        let dns = reserve_dns_port()?;
        let dns_port = dns.port();

        let root_ca = dir.path().join("pebble.minica.pem");
        std::fs::write(&root_ca, PEBBLE_MINICA_PEM)
            .context("failed to write the Pebble root CA")?;

        let config_path = write_pebble_config(
            dir.path(),
            acme_port,
            pebble_management.port(),
            http.port(),
            tls_port,
        )?;

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
            tls_reservation: Mutex::new(Some(tls)),
            _http_reservation: http,
            root_ca,
        };

        let dns_bind = format!("127.0.0.1:{dns_port}");
        let management_bind = format!("127.0.0.1:{management_port}");
        // The test listens on the TLS-ALPN-01 port itself; the other challenge servers are off.
        let dns_container = format!("{unique}-dns");
        harness.create_container(
            &dns_container,
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
            &[],
        )?;
        drop((challenge_management, dns));
        start_container(&dns_container)?;

        // Pebble rejects 5% of nonces by default to exercise a client's retries.
        // instant-acme retries a rejected nonce twice, so three rejections in a
        // row fail an order now and then. The engine retries a failed order
        // only after a backoff of half a minute or more, and the ACME tests
        // order once, so the harness turns the rejection off.
        let ca_container = format!("{unique}-ca");
        harness.create_container(
            &ca_container,
            PEBBLE_IMAGE,
            &[
                "-e",
                "PEBBLE_VA_NOSLEEP=1",
                "-e",
                "PEBBLE_AUTHZREUSE=0",
                "-e",
                "PEBBLE_WFE_NONCEREJECT=0",
            ],
            &["-config", "/pebble-config.json", "-dnsserver", &dns_bind],
            &[(&config_path, "/pebble-config.json")],
        )?;
        drop((acme, pebble_management));
        start_container(&ca_container)?;

        for (what, port) in [
            ("Pebble", acme_port),
            ("challenge test server", management_port),
        ] {
            harness.wait_for_port(what, port).await;
        }
        // Pebble asks over TCP, and the CAA lookups under test over UDP.
        harness.wait_for_dns("TCP", ConnectionConfig::tcp()).await?;
        harness.wait_for_dns("UDP", ConnectionConfig::udp()).await?;
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
        self.release_tls_port();
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

    /// Create a container and copy `files` (local path, path in the container)
    /// into it; [`start_container`] starts it.
    ///
    /// Files are copied rather than bind-mounted because the Docker daemon may not
    /// share this process's filesystem: on the CI runners it is a sidecar that
    /// sees only the job's work directory, and a bind mount of a path it lacks
    /// becomes an empty directory. The network is shared, which host networking
    /// relies on. Containers are not `--rm`, so a container that exits at
    /// startup keeps its logs for `docker logs`; `Drop` removes them.
    fn create_container(
        &mut self,
        name: &str,
        image: &str,
        docker_args: &[&str],
        app_args: &[&str],
        files: &[(&Path, &str)],
    ) -> anyhow::Result<()> {
        let created = docker(
            ["create", "--name", name, "--network", "host"]
                .into_iter()
                .chain(docker_args.iter().copied())
                .chain(std::iter::once(image))
                .chain(app_args.iter().copied()),
        )?;
        anyhow::ensure!(
            created.status.success(),
            "docker create {image} failed: {}",
            String::from_utf8_lossy(&created.stderr)
        );
        self.containers.push(name.to_owned());

        for (source, destination) in files {
            let source = source.to_string_lossy();
            let target = format!("{name}:{destination}");
            let copied = docker(["cp", source.as_ref(), target.as_str()])?;
            anyhow::ensure!(
                copied.status.success(),
                "docker cp {source} into {name} failed: {}",
                String::from_utf8_lossy(&copied.stderr)
            );
        }
        Ok(())
    }

    /// Wait for a container to accept connections. A container that never
    /// listens fails the wait with the last lines of every container's log.
    async fn wait_for_port(&self, what: &str, port: u16) {
        wait::until_reporting(
            format!("the {what} on port {port} to start listening"),
            || async {
                TcpStream::connect(("127.0.0.1", port))
                    .await
                    .ok()
                    .map(|_| ())
            },
            || self.container_logs(),
        )
        .await;
    }

    /// Wait for the challenge test server to answer a DNS query over
    /// `connection`. The server logs a failure to bind its DNS port and keeps
    /// running, and whatever holds the port instead may accept connections, so
    /// only an answer shows the port is the challenge test server's.
    async fn wait_for_dns(
        &self,
        protocol: &str,
        connection: ConnectionConfig,
    ) -> anyhow::Result<()> {
        let resolver = dns_resolver(self.dns_addr(), connection)?;
        wait::until_reporting(
            format!(
                "the challenge test server to answer DNS over {protocol} on port {}",
                self.dns_port
            ),
            || async {
                resolver
                    .lookup("harness-ready.test.", RecordType::A)
                    .await
                    .ok()
                    .map(|_| ())
            },
            || self.container_logs(),
        )
        .await;
        Ok(())
    }

    /// The last lines of every container's log, for a failed wait.
    fn container_logs(&self) -> String {
        self.containers
            .iter()
            .map(|name| match docker(["logs", "--tail", "20", name]) {
                Ok(o) => format!(
                    "{name}: {}{}",
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(error) => format!("{name}: {error:#}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Let go of the TLS-ALPN-01 port so the caller can bind it: the fake relay
    /// that stands in for the front door, or [`Self::serve_tls_alpn`].
    pub(crate) fn release_tls_port(&self) {
        let reservation = self
            .tls_reservation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(reservation);
    }

    /// How many DNS queries for `host` the challenge test server has answered
    /// since its history for `host` was last cleared.
    pub(crate) async fn dns_queries(&self, host: &str) -> anyhow::Result<usize> {
        let body = self
            .manage_reading("dns-request-history", &serde_json::json!({ "host": host }))
            .await?;
        let history: Vec<serde_json::Value> = serde_json::from_str(&body)
            .with_context(|| format!("unreadable DNS request history: {body}"))?;
        Ok(history.len())
    }

    /// Forget the DNS queries for `host` the challenge test server recorded.
    pub(crate) async fn clear_dns_queries(&self, host: &str) -> anyhow::Result<()> {
        self.manage(
            "clear-request-history",
            &serde_json::json!({ "host": host, "type": "dns" }),
        )
        .await
    }

    /// POST `body` to the challenge test server's management API.
    async fn manage(&self, endpoint: &str, body: &serde_json::Value) -> anyhow::Result<()> {
        self.manage_reading(endpoint, body).await.map(|_| ())
    }

    /// POST `body` to the challenge test server's management API and return
    /// the response body.
    async fn manage_reading(
        &self,
        endpoint: &str,
        body: &serde_json::Value,
    ) -> anyhow::Result<String> {
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
        Ok(response
            .split_once("\r\n\r\n")
            .map(|(_, answer)| answer.to_owned())
            .unwrap_or_default())
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

/// Start a container made by [`PebbleHarness::create_container`].
fn start_container(name: &str) -> anyhow::Result<()> {
    let started = docker(["start", name])?;
    anyhow::ensure!(
        started.status.success(),
        "docker start {name} failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );
    Ok(())
}

/// Write Pebble's configuration into `dir`, readable by the container's user,
/// and return its path.
fn write_pebble_config(
    dir: &Path,
    acme_port: u16,
    management_port: u16,
    http_port: u16,
    tls_port: u16,
) -> anyhow::Result<PathBuf> {
    // Paths are inside the image: it carries Pebble's own test certificates.
    let config = serde_json::json!({
        "pebble": {
            "listenAddress": format!("127.0.0.1:{acme_port}"),
            "managementListenAddress": format!("127.0.0.1:{management_port}"),
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
    let config_path = dir.join("pebble-config.json");
    std::fs::write(&config_path, serde_json::to_vec_pretty(&config)?)
        .context("failed to write the Pebble configuration")?;
    set_world_readable(&config_path)?;
    Ok(config_path)
}

/// A reserved port whose UDP side is held too, since the DNS server binds both.
struct DnsReservation {
    tcp: ReservedPort,
    _udp: UdpSocket,
}

impl DnsReservation {
    fn port(&self) -> u16 {
        self.tcp.port()
    }
}

/// Reserve a port that is free for both TCP and UDP. [`reserve_port`] hands
/// out each port once per process, so a port whose UDP side is taken is not
/// offered again.
fn reserve_dns_port() -> anyhow::Result<DnsReservation> {
    for _ in 0..20 {
        let tcp = reserve_port();
        if let Ok(udp) = UdpSocket::bind(("127.0.0.1", tcp.port())) {
            return Ok(DnsReservation { tcp, _udp: udp });
        }
    }
    anyhow::bail!("no reserved port was free for UDP as well as TCP")
}

/// A resolver that asks only `server` over `connection`, without caching.
fn dns_resolver(
    server: SocketAddr,
    mut connection: ConnectionConfig,
) -> anyhow::Result<TokioResolver> {
    connection.port = server.port();
    let config = ResolverConfig::from_name_servers(vec![NameServerConfig::new(
        server.ip(),
        true,
        vec![connection],
    )]);
    let mut builder = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
    let options = builder.options_mut();
    options.cache_size = 0;
    options.use_hosts_file = ResolveHosts::Never;
    builder
        .build()
        .context("failed to build the harness's DNS resolver")
}

/// The container runs as another user, and `docker cp` keeps the file's mode, so the configuration must be readable by others.
#[cfg_attr(
    not(unix),
    expect(
        clippy::unnecessary_wraps,
        reason = "mirrors the unix signature, where setting permissions can fail"
    )
)]
fn set_world_readable(file: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o644))?;
    }
    #[cfg(not(unix))]
    {
        let _ = file;
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
