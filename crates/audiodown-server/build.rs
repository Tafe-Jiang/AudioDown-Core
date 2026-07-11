use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"));
    let web_dist = manifest_dir.join("../../web/dist");
    println!("cargo:rerun-if-changed={}", web_dist.display());

    if !web_dist.join("index.html").is_file() {
        panic!("web/dist is missing; run npm ci && npm run build in web/");
    }
}
