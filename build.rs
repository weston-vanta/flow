//! Fails the build with a clear message when the web bundle that `flowd` embeds is missing.

use std::path::Path;

const BUNDLE: &[&str] = &["web/dist/index.html", "web/dist/app.js", "web/dist/app.css"];

fn main() {
    println!("cargo::rerun-if-changed=web/dist");
    let missing: Vec<_> = BUNDLE.iter().filter(|f| !Path::new(f).is_file()).collect();
    if !missing.is_empty() {
        panic!(
            "the web bundle is missing ({missing:?}); build it with \
             `npm --prefix web ci && npm --prefix web run build`"
        );
    }
}
