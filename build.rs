fn main() {
    let dist = std::path::Path::new("web/dist/index.html");
    assert!(
        dist.exists(),
        "web/dist/ not found — run `npm run build` in the web/ directory first"
    );
    println!("cargo:rerun-if-changed=web/dist/");
}
