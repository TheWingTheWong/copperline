// Embed the git commit the binary is built from, so benchmark rows can name the exact build.
fn main() {
    let v = std::process::Command::new("git")
        .args(["describe", "--always", "--dirty", "--tags"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=COPPERLINE_BUILD={v}");
    for p in ["src", ".git/HEAD", ".git/index"] {
        println!("cargo:rerun-if-changed={p}");
    }
}
