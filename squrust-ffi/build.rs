//! Copy the public C header into `OUT_DIR` so downstream build systems can find
//! it next to the compiled library.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=squrust.h");
    if let Ok(out_dir) = std::env::var("OUT_DIR") {
        let dst = PathBuf::from(out_dir).join("squrust.h");
        let _ = std::fs::copy("squrust.h", &dst);
        println!("cargo:include={}", dst.display());
    }
}
