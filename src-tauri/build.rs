use std::path::{Path, PathBuf};

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

    create_missing_helper_placeholders();

    tauri_build::build()
}

/// tauri-build fails when a helper listed in `bundle.externalBin` is missing. A helper
/// that nothing built becomes an empty file, so the app still compiles. The feature that
/// needs the helper then fails when it starts it. `scripts/run-tauri.mjs` builds the real
/// `marlin-mount`, and `npm run build:smb-sidecar` builds the real `marlin-smb`.
fn create_missing_helper_placeholders() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // tauri-build reports a config it cannot read, so this reads one as having no helpers.
    let conf: serde_json::Value = std::fs::read_to_string(manifest_dir.join("tauri.conf.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();

    let target = std::env::var("TARGET").unwrap();
    let extension = if std::env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        ".exe"
    } else {
        ""
    };
    for helper in conf["bundle"]["externalBin"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(helper) = helper.as_str() else {
            continue;
        };
        let path = manifest_dir.join(format!("{helper}-{target}{extension}"));
        if !path.exists() {
            std::fs::write(&path, "").unwrap_or_else(|error| {
                panic!(
                    "could not write the helper placeholder {}: {error}",
                    path.display()
                )
            });
        }
    }
}

/// Read the Linux deb install directory for libzpl from tauri.conf.json.
///
/// The `bundle.linux.deb.files` map uses destination-in-package as key and
/// source-on-disk as value. We look for a key whose filename is `libzpl.so`
/// and return its parent directory (e.g. `/usr/lib/marlin`).
fn libzpl_deb_install_dir() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let conf_path = manifest_dir.join("tauri.conf.json");
    println!("cargo:rerun-if-changed={}", conf_path.display());

    let conf_text = std::fs::read_to_string(&conf_path).ok()?;
    let conf: serde_json::Value = serde_json::from_str(&conf_text).ok()?;

    let files = conf
        .get("bundle")?
        .get("linux")?
        .get("deb")?
        .get("files")?
        .as_object()?;

    for pkg_dest in files.keys() {
        if Path::new(pkg_dest).file_name().and_then(|n| n.to_str()) == Some("libzpl.so") {
            return Path::new(pkg_dest).parent().map(|p| p.to_path_buf());
        }
    }

    None
}

fn setup_libzpl() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let lib_name = match target_os.as_str() {
        "macos" => "libzpl.dylib",
        "windows" => "zpl.dll",
        _ => "libzpl.so",
    };

    // zpl-rs declares `links = "zpl"` and prints `cargo:lib_dir`, so Cargo passes this
    // script the directory that holds the library.
    println!("cargo:rerun-if-env-changed=DEP_ZPL_LIB_DIR");
    let lib_dir = std::env::var("DEP_ZPL_LIB_DIR").expect("zpl-rs sets DEP_ZPL_LIB_DIR");
    let lib_path = PathBuf::from(lib_dir).join(lib_name);
    println!("cargo:rerun-if-changed={}", lib_path.display());

    // The final binary lands in target/{profile}/. OUT_DIR sits under
    // target/{profile}/build/, at a depth that differs between Cargo versions.
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let target_dir = out_dir
        .ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "build"))
        .and_then(Path::parent)
        .expect("OUT_DIR is under target/{profile}/build/");
    if let Err(e) = std::fs::copy(&lib_path, target_dir.join(lib_name)) {
        println!("cargo:warning=libzpl: copy failed: {}", e);
    }

    // Also copy to CARGO_MANIFEST_DIR/lib/ so Tauri's bundler can
    // find it: macOS uses `bundle.macOS.frameworks` to bundle into
    // Contents/Frameworks/, Linux uses `bundle.linux.deb.files` to
    // install into the configured directory.
    if target_os == "macos" || target_os == "linux" {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let fw_dir = manifest_dir.join("lib");
        let _ = std::fs::create_dir_all(&fw_dir);
        if let Err(e) = std::fs::copy(&lib_path, fw_dir.join(lib_name)) {
            println!("cargo:warning=libzpl: bundler copy failed: {}", e);
        }
    }

    // Set rpath so the binary can find the dylib next to itself
    if target_os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
    } else if target_os == "linux" {
        // $ORIGIN: finds libzpl.so next to the binary (dev builds + AppImage)
        // deb install dir: finds libzpl.so installed by the .deb package
        // (read from tauri.conf.json so the path isn't duplicated)
        let deb_rpath = libzpl_deb_install_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/usr/lib/marlin".to_string());
        println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN,-rpath,{deb_rpath}");
    }
}
