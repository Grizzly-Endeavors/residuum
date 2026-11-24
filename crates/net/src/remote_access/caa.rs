use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Context as _;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolveHosts, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::{RData, RecordType};
use thiserror::Error;
use tokio::time::Instant;
use tracing::{debug, warn};

use super::types::normalize_host;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CaaSettings {
    pub resolver: SocketAddr,
}

impl Default for CaaSettings {
    fn default() -> Self {
        Self {
            resolver: SocketAddr::from(([1, 1, 1, 1], 53)),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum CaaWaitError {
    #[error("CAA records for {} did not appear in time", .missing.join(", "))]
    TimedOut { missing: Vec<String> },
    #[error("CAA lookup failed: {0:#}")]
    Lookup(anyhow::Error),
}

fn build_resolver(settings: &CaaSettings) -> anyhow::Result<TokioResolver> {
    let mut connections = vec![ConnectionConfig::udp(), ConnectionConfig::tcp()];
    for connection in &mut connections {
        connection.port = settings.resolver.port();
    }
    let config = ResolverConfig::from_name_servers(vec![NameServerConfig::new(
        settings.resolver.ip(),
        true,
        connections,
    )]);
    let mut builder = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default());
    let options = builder.options_mut();
    options.cache_size = 0;
    options.ndots = 0;
    options.use_hosts_file = ResolveHosts::Never;
    options.timeout = Duration::from_secs(4);
    options.attempts = 2;
    options.try_tcp_on_error = true;
    builder.build().context("failed to build the CAA resolver")
}

fn issue_value_pins_account(value: &str, account_uri: &str) -> bool {
    value.split(';').skip(1).any(|parameter| {
        parameter
            .split_once('=')
            .is_some_and(|(tag, parameter_value)| {
                tag.trim().eq_ignore_ascii_case("accounturi")
                    && parameter_value.trim() == account_uri
            })
    })
}

async fn lookup_allows(
    resolver: &TokioResolver,
    name: &str,
    account_uri: &str,
) -> anyhow::Result<bool> {
    let query = format!("{}.", normalize_host(name));
    let lookup = match resolver.lookup(query.as_str(), RecordType::CAA).await {
        Ok(lookup) => lookup,
        Err(error) if error.is_no_records_found() || error.is_nx_domain() => return Ok(false),
        Err(error) => {
            return Err(anyhow::Error::new(error)
                .context(format!("failed to query CAA records for {name}")));
        }
    };
    Ok(lookup.answers().iter().any(|record| {
        let RData::CAA(caa) = &record.data else {
            return false;
        };
        caa.tag.eq_ignore_ascii_case("issue")
            && std::str::from_utf8(&caa.value)
                .is_ok_and(|value| issue_value_pins_account(value, account_uri))
    }))
}

#[cfg(test)]
pub(crate) async fn caa_allows_account(
    settings: &CaaSettings,
    name: &str,
    account_uri: &str,
) -> anyhow::Result<bool> {
    lookup_allows(&build_resolver(settings)?, name, account_uri).await
}

pub(crate) async fn wait_for_caa(
    settings: &CaaSettings,
    names: &[String],
    account_uri: &str,
    timeout: Duration,
    poll: Duration,
) -> Result<(), CaaWaitError> {
    let resolver = build_resolver(settings).map_err(CaaWaitError::Lookup)?;
    let deadline = Instant::now() + timeout;
    let mut missing: Vec<String> = names.to_vec();
    let mut warned = false;

    loop {
        let mut still_missing = Vec::new();
        let mut last_failure: Option<anyhow::Error> = None;
        for name in missing {
            match lookup_allows(&resolver, &name, account_uri).await {
                Ok(true) => debug!(name = %name, "CAA pins the account"),
                Ok(false) => still_missing.push(name),
                Err(error) => {
                    if !warned {
                        warn!(error = %format!("{error:#}"), resolver = %settings.resolver, "CAA lookup failed, retrying until the timeout");
                        warned = true;
                    }
                    last_failure = Some(error);
                    still_missing.push(name);
                }
            }
        }
        missing = still_missing;
        if missing.is_empty() {
            return Ok(());
        }
        if Instant::now() + poll >= deadline {
            return Err(match last_failure {
                Some(error) => CaaWaitError::Lookup(error.context(format!(
                    "could not confirm the CAA records for {}",
                    missing.join(", ")
                ))),
                None => CaaWaitError::TimedOut { missing },
            });
        }
        tokio::time::sleep(poll).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const ACCOUNT: &str = "https://ca.example/acme/acct/42";

    #[test]
    fn issue_value_with_matching_account_pins() {
        assert!(issue_value_pins_account(
            &format!("letsencrypt.org; accounturi={ACCOUNT}"),
            ACCOUNT
        ));
    }

    #[test]
    fn whitespace_and_tag_case_are_tolerated() {
        assert!(issue_value_pins_account(
            &format!("letsencrypt.org ;  AccountURI = {ACCOUNT}  "),
            ACCOUNT
        ));
        assert!(issue_value_pins_account(
            &format!("ca.example;validationmethods=tls-alpn-01;accounturi={ACCOUNT}"),
            ACCOUNT
        ));
    }

    #[test]
    fn other_accounts_and_missing_parameters_do_not_pin() {
        assert!(!issue_value_pins_account("letsencrypt.org", ACCOUNT));
        assert!(!issue_value_pins_account(";", ACCOUNT));
        assert!(!issue_value_pins_account(
            "letsencrypt.org; accounturi=https://ca.example/acme/acct/43",
            ACCOUNT
        ));
        assert!(!issue_value_pins_account(
            &format!("letsencrypt.org; accounturi={ACCOUNT}9"),
            ACCOUNT
        ));
    }

    #[test]
    fn account_uri_in_the_issuer_position_does_not_pin() {
        assert!(!issue_value_pins_account(ACCOUNT, ACCOUNT));
        assert!(!issue_value_pins_account(
            &format!("accounturi={ACCOUNT}"),
            ACCOUNT
        ));
    }

    #[test]
    fn default_resolver_is_cloudflare() {
        assert_eq!(CaaSettings::default().resolver.to_string(), "1.1.1.1:53");
    }

    use crate::remote_access::pebble_support::PebbleHarness;

    fn pin(account: &str) -> String {
        format!("pebble.letsencrypt.org; accounturi={account}")
    }

    #[tokio::test]
    #[ignore = "needs docker: runs Pebble"]
    async fn pebble_caa_lookup_matches_the_account_only_at_the_exact_name() {
        let pebble = PebbleHarness::start().await.unwrap();
        let settings = CaaSettings {
            resolver: pebble.dns_addr(),
        };
        pebble
            .set_caa("pinned.lab.test", &[&pin(ACCOUNT)])
            .await
            .unwrap();
        pebble
            .set_caa("other.lab.test", &[&pin("https://ca.example/acme/acct/43")])
            .await
            .unwrap();
        pebble
            .set_caa("plain.lab.test", &["pebble.letsencrypt.org"])
            .await
            .unwrap();
        pebble
            .set_caa("parent.lab.test", &[&pin(ACCOUNT)])
            .await
            .unwrap();

        assert!(
            caa_allows_account(&settings, "pinned.lab.test", ACCOUNT)
                .await
                .unwrap()
        );
        assert!(
            caa_allows_account(&settings, "PINNED.lab.test.", ACCOUNT)
                .await
                .unwrap()
        );
        assert!(
            !caa_allows_account(&settings, "other.lab.test", ACCOUNT)
                .await
                .unwrap()
        );
        assert!(
            !caa_allows_account(&settings, "plain.lab.test", ACCOUNT)
                .await
                .unwrap()
        );
        assert!(
            !caa_allows_account(&settings, "absent.lab.test", ACCOUNT)
                .await
                .unwrap()
        );
        assert!(
            !caa_allows_account(&settings, "child.parent.lab.test", ACCOUNT)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    #[ignore = "needs docker: runs Pebble"]
    async fn pebble_caa_wait_times_out_on_missing_names_then_succeeds_once_added() {
        let pebble = Arc::new(PebbleHarness::start().await.unwrap());
        let settings = CaaSettings {
            resolver: pebble.dns_addr(),
        };
        let names = vec!["one.wait.test".to_owned(), "two.wait.test".to_owned()];
        pebble
            .set_caa("one.wait.test", &[&pin(ACCOUNT)])
            .await
            .unwrap();

        let error = wait_for_caa(
            &settings,
            &names,
            ACCOUNT,
            Duration::from_millis(1500),
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        match error {
            CaaWaitError::TimedOut { missing } => {
                assert_eq!(missing, vec!["two.wait.test".to_owned()]);
            }
            CaaWaitError::Lookup(e) => panic!("unexpected lookup failure: {e:#}"),
        }

        pebble.clear_dns_queries("two.wait.test").await.unwrap();
        let late = Arc::clone(&pebble);
        let adder = tokio::spawn(async move {
            residuum_core::testing::wait::until("the wait to look up the missing name", || async {
                (late.dns_queries("two.wait.test").await.unwrap() > 0).then_some(())
            })
            .await;
            late.set_caa("two.wait.test", &[&pin(ACCOUNT)])
                .await
                .unwrap();
        });
        wait_for_caa(
            &settings,
            &names,
            ACCOUNT,
            Duration::from_secs(15),
            Duration::from_millis(200),
        )
        .await
        .unwrap();
        adder.await.unwrap();
    }

    #[tokio::test]
    async fn unreachable_resolver_is_a_lookup_error_not_missing_records() {
        let settings = CaaSettings {
            resolver: SocketAddr::from(([127, 0, 0, 1], 9)),
        };
        let error = wait_for_caa(
            &settings,
            &["x.lab.test".to_owned()],
            ACCOUNT,
            Duration::from_millis(300),
            Duration::from_millis(100),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, CaaWaitError::Lookup(_)), "{error}");
    }
}
