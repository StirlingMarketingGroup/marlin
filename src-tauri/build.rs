use std::path::PathBuf;

fn main() {
    // Load .env file if it exists (for local development)
    // In CI/CD, these should be set as environment variables directly
    let _ = dotenvy::dotenv();

    // Pass Google OAuth credentials to the compiler
    // These will be available via env!() macro at compile time
    if let Ok(client_id) = std::env::var("GOOGLE_CLIENT_ID") {
        println!("cargo:rustc-env=GOOGLE_CLIENT_ID={}", client_id);
    }
    if let Ok(client_secret) = std::env::var("GOOGLE_CLIENT_SECRET") {
        println!("cargo:rustc-env=GOOGLE_CLIENT_SECRET={}", client_secret);
    }

    // Re-run build script if .env changes
    println!("cargo:rerun-if-changed=.env");
    println!("cargo:rerun-if-env-changed=GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=GOOGLE_CLIENT_SECRET");

    // Copy libzpl shared library next to the final binary so it can be found at runtime.
    // The zpl-rs crate downloads it during its build, but its rpath link args don't
    // propagate to dependent crates.
    setup_libzpl();

    tauri_build::build()
}

fn setup_libzpl() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

    // OUT_DIR is target/{profile}/build/{crate}-{hash}/out
    // Navigate up to target/{profile}/build/
    let build_dir = out_dir
        .parent() // {crate}-{hash}
        .and_then(|p| p.parent()); // build

    let build_dir = match build_dir {
        Some(d) => d,
        None => return,
    };

    // target/{profile}/ is the directory where the final binary ends up
    let target_dir = match build_dir.parent() {
        Some(d) => d,
        None => return,
    };

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let lib_name = match target_os.as_str() {
        "macos" => "libzpl.dylib",
        "windows" => "zpl.dll",
        _ => "libzpl.so",
    };

    // Find libzpl in any zpl-rs build output directory
    let mut found = false;
    if let Ok(entries) = std::fs::read_dir(build_dir) {
        for entry_result in entries {
            let entry = match entry_result {
                Ok(e) => e,
                Err(e) => {
                    println!("cargo:warning=libzpl: error reading build dir entry: {e}");
                    continue;
                }
            };
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("zpl-rs-") {
                let lib_path = entry.path().join("out").join("lib").join(lib_name);
                if lib_path.exists() {
                    // Re-run if the source library changes
                    println!("cargo:rerun-if-changed={}", lib_path.display());

                    let dest = target_dir.join(lib_name);
                    if let Err(e) = std::fs::copy(&lib_path, &dest) {
                        println!("cargo:warning=libzpl: copy failed: {}", e);
                    }

                    // Also copy to CARGO_MANIFEST_DIR/lib/ so Tauri's bundler can
                    // find it: macOS uses `bundle.macOS.frameworks` to bundle into
                    // Contents/Frameworks/, Linux uses `bundle.linux.deb.files` to
                    // install into /usr/lib/marlin/.
                    if target_os == "macos" || target_os == "linux" {
                        let manifest_dir =
                            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
                        let fw_dir = manifest_dir.join("lib");
                        let _ = std::fs::create_dir_all(&fw_dir);
                        if let Err(e) = std::fs::copy(&lib_path, fw_dir.join(lib_name)) {
                            println!("cargo:warning=libzpl: bundler copy failed: {}", e);
                        }
                    }

                    found = true;
                    break;
                }
            }
        }
    }

    if !found {
        println!("cargo:warning=libzpl not found in zpl-rs build output");
    }

    // Set rpath so the binary can find the dylib next to itself
    if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    } else if target_os == "linux" {
        // $ORIGIN: finds libzpl.so next to the binary (dev builds + AppImage)
        // /usr/lib/marlin: finds libzpl.so installed by the .deb package
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN,-rpath,/usr/lib/marlin");
    }
}
