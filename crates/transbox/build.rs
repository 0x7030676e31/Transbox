use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=TRANSBOX_BUNDLE");
    let source = match env::var("TRANSBOX_BUNDLE") {
        Ok(path) => {
            let path =
                fs::canonicalize(path).expect("TRANSBOX_BUNDLE must name an existing ZIP file");
            println!("cargo:rerun-if-changed={}", path.display());
            format!(
                "pub static BUNDLE: &[u8] = include_bytes!({:?});",
                path.to_str().expect("UTF-8 bundle path")
            )
        }
        Err(_) => "pub static BUNDLE: &[u8] = &[];".to_owned(),
    };
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("bundle.rs"),
        source,
    )
    .unwrap();
}
