fn main() {
    // `option_env!` in `src/sign_in.rs` reads these two when this crate compiles, so the app and the
    // mount helper get the same OAuth client. A developer keeps them in `src-tauri/.env`: this script
    // runs in the crate's folder, and `dotenv` looks in each folder above it.
    let _ = dotenvy::dotenv();

    if let Ok(client_id) = std::env::var("GOOGLE_CLIENT_ID") {
        println!("cargo:rustc-env=GOOGLE_CLIENT_ID={}", client_id);
    }
    if let Ok(client_secret) = std::env::var("GOOGLE_CLIENT_SECRET") {
        println!("cargo:rustc-env=GOOGLE_CLIENT_SECRET={}", client_secret);
    }

    println!("cargo:rerun-if-changed=../../.env");
    println!("cargo:rerun-if-env-changed=GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=GOOGLE_CLIENT_SECRET");
}
