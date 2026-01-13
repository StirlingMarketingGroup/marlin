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

    tauri_build::build()
}
