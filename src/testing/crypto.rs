//! The rustls process-default `CryptoProvider` for the test binaries: the lib, the bin and `tests/it`.
//!
//! It runs before `main`, which is safe because it only installs a process-default provider and no other initialization depends on its order.
//!
//! reqwest 0.13 is built with `rustls-no-provider` and panics in `Client::new()`
//! until a process-default provider is installed. Production installs ring in
//! `commands::run`, which test binaries never reach, so the install runs before
//! `main` instead.

#[ctor::ctor(unsafe)]
fn install_rustls_provider() {
    drop(rustls::crypto::ring::default_provider().install_default());
}
