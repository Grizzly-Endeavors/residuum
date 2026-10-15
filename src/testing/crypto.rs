//! The rustls process-default CryptoProvider for the unit-test binary.
//!
//! reqwest 0.13 is built with `rustls-no-provider` and panics in `Client::new()`
//! until a process-default provider is installed. Production installs ring in
//! `commands::run`, which test binaries never reach, so the install runs before
//! `main` instead.

#[ctor::ctor(unsafe)]
#[expect(
    unsafe_code,
    reason = "a test binary has no main to install the rustls provider in, and reqwest panics on the first client without it"
)]
fn install_rustls_provider() {
    drop(rustls::crypto::ring::default_provider().install_default());
}
