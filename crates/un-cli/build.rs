use std::process::Command;

fn main() {
    // Rerun if the git HEAD changes
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let workspace_root = std::path::Path::new(&manifest_dir)
        .parent()
        .and_then(|p| p.parent())
        .unwrap();
    let git_dir = workspace_root.join(".git");

    if git_dir.exists() {
        let head = git_dir.join("HEAD");
        println!("cargo:rerun-if-changed={}", head.display());

        if let Ok(contents) = std::fs::read_to_string(&head) {
            // If HEAD points to a ref, also track that ref file
            if let Some(git_ref) = contents.strip_prefix("ref: ") {
                let ref_path = git_dir.join(git_ref.trim());
                println!("cargo:rerun-if-changed={}", ref_path.display());
            }
        }
    }

    // Capture commit hash
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
    {
        if output.status.success() {
            let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("cargo:rustc-env=UN_COMMIT_HASH={hash}");
        }
    }

    // Capture short commit hash
    if let Ok(output) = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .output()
    {
        if output.status.success() {
            let hash = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("cargo:rustc-env=UN_COMMIT_SHORT_HASH={hash}");
        }
    }

    // Capture commit date
    if let Ok(output) = Command::new("git")
        .args(["log", "-1", "--date=short", "--format=%cd"])
        .output()
    {
        if output.status.success() {
            let date = String::from_utf8_lossy(&output.stdout).trim().to_string();
            println!("cargo:rustc-env=UN_COMMIT_DATE={date}");
        }
    }
}
