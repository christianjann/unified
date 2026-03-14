use std::process::Command;

#[test]
fn test_sync_file_repo() {
    // Clean up
    let _ = std::fs::remove_dir_all("tests/test_data");

    // Create tests/test_data/source
    let source_dir = std::path::PathBuf::from("tests/test_data/source");
    std::fs::create_dir_all(&source_dir).unwrap();

    // Init git
    Command::new("git").args(&["init"]).current_dir(&source_dir).status().unwrap();

    // Add file
    std::fs::write(source_dir.join("hello.txt"), "Hello, world!").unwrap();
    Command::new("git").args(&["add", "hello.txt"]).current_dir(&source_dir).status().unwrap();
    Command::new("git").args(&["-c", "user.email=test@example.com", "-c", "user.name=Test", "commit", "-m", "Initial commit"]).current_dir(&source_dir).status().unwrap();

    // Get the file:// url
    let url = format!("file://{}", source_dir.canonicalize().unwrap().to_string_lossy());

    // Simulate sync
    let cache = un_cache::Cache::with_custom_root(std::env::current_dir().unwrap().join("tests/test_data/cache"));
    let remote = un_git::GitRemote::new(&url);
    let database = un_git::GitDatabase::new(&cache, "test", &url).unwrap();
    let oid = database.fetch(&remote, &un_core::GitReference::DefaultBranch, false, false).unwrap();

    // Create workspace
    let workspace_dir = std::env::current_dir().unwrap().join("tests/test_data/workspace");
    std::fs::create_dir_all(&workspace_dir).unwrap();

    // Checkout
    let _checkout = un_git::GitCheckout::new(&database, &oid, &workspace_dir.join("repo"), un_git::CheckoutMode::Worktree).unwrap();

    // Check if file exists
    assert!(workspace_dir.join("repo/hello.txt").exists());
}