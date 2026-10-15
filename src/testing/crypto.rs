//! The rustls process-default `CryptoProvider` for the unit-test binaries (the lib and the bin).
//!
//! reqwest 0.13 is built with `rustls-no-provider` and panics in `Client::new()`
//! until a process-default provider is installed. Production installs ring in
//! `commands::run`, which test binaries never reach, so the install runs before
//! `main` instead.

#[ctor::ctor(unsafe)]
fn install_rustls_provider() {
    drop(rustls::crypto::ring::default_provider().install_default());
}
