use anyhow::{Context, Result};
use std::fs;
use std::io::Cursor;
use std::path::Path;

/// Extract an archive to a destination directory.
/// Supports .zip, .tar.gz, .tgz, and plain files.
/// Returns the number of files extracted.
pub fn extract_archive(data: &[u8], filename: &str, dest: &Path) -> Result<usize> {
    fs::create_dir_all(dest).context("creating extraction directory")?;

    let lower = filename.to_lowercase();
    if lower.ends_with(".zip") {
        extract_zip(data, dest)
    } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        extract_tar_gz(data, dest)
    } else {
        // Single file — write directly with the filename
        let file_name = Path::new(filename)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "download".to_string());
        let dest_path = dest.join(&file_name);
        fs::write(&dest_path, data).context("writing downloaded file")?;
        #[cfg(unix)]
        set_executable(&dest_path)?;
        Ok(1)
    }
}

fn extract_zip(data: &[u8], dest: &Path) -> Result<usize> {
    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor).context("failed to open zip archive")?;

    let mut count = 0;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).context("reading zip entry")?;
        let name = file.name().to_string();

        // Security: reject absolute paths and path traversal
        if name.starts_with('/') || name.contains("..") {
            continue;
        }

        let out_path = dest.join(&name);

        if file.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out_file = fs::File::create(&out_path)?;
            std::io::copy(&mut file, &mut out_file)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(mode) = file.unix_mode() {
                    fs::set_permissions(&out_path, fs::Permissions::from_mode(mode))?;
                }
            }

            count += 1;
        }
    }
    Ok(count)
}

fn extract_tar_gz(data: &[u8], dest: &Path) -> Result<usize> {
    let cursor = Cursor::new(data);
    let gz = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(gz);

    let mut count = 0;
    for entry in archive.entries().context("reading tar entries")? {
        let mut entry = entry.context("reading tar entry")?;
        let path = entry.path().context("reading entry path")?.into_owned();

        // Security: reject absolute paths and path traversal
        let path_str = path.to_string_lossy();
        if path_str.starts_with('/') || path_str.contains("..") {
            continue;
        }

        let out_path = dest.join(&path);

        if entry.header().entry_type().is_dir() {
            fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)?;
            }
            entry.unpack(&out_path)?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::metadata(path)?;
    let mut perms = metadata.permissions();
    perms.set_mode(perms.mode() | 0o111);
    fs::set_permissions(path, perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn extract_plain_file() {
        let dir = tempfile::tempdir().unwrap();
        let data = b"hello world";
        let count = extract_archive(data, "hello.txt", dir.path()).unwrap();
        assert_eq!(count, 1);
        assert!(dir.path().join("hello.txt").exists());
        assert_eq!(
            fs::read_to_string(dir.path().join("hello.txt")).unwrap(),
            "hello world"
        );
    }

    #[test]
    fn extract_zip_archive() {
        let dir = tempfile::tempdir().unwrap();

        // Create a small zip in memory
        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("test.txt", options).unwrap();
        zip.write_all(b"zip content").unwrap();
        let cursor = zip.finish().unwrap();
        let data = cursor.into_inner();

        let count = extract_archive(&data, "test.zip", dir.path()).unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            fs::read_to_string(dir.path().join("test.txt")).unwrap(),
            "zip content"
        );
    }

    #[test]
    fn extract_tar_gz_archive() {
        let dir = tempfile::tempdir().unwrap();

        // Create a tar.gz in memory
        let mut builder = tar::Builder::new(Vec::new());
        let content = b"tar content";
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "test.txt", &content[..])
            .unwrap();
        let tar_data = builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&tar_data).unwrap();
        let gz_data = encoder.finish().unwrap();

        let count = extract_archive(&gz_data, "test.tar.gz", dir.path()).unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            fs::read_to_string(dir.path().join("test.txt")).unwrap(),
            "tar content"
        );
    }

    #[test]
    fn rejects_path_traversal_zip() {
        let dir = tempfile::tempdir().unwrap();

        let buf = Vec::new();
        let cursor = Cursor::new(buf);
        let mut zip = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("../escape.txt", options).unwrap();
        zip.write_all(b"evil").unwrap();
        let cursor = zip.finish().unwrap();
        let data = cursor.into_inner();

        let count = extract_archive(&data, "bad.zip", dir.path()).unwrap();
        assert_eq!(count, 0); // Should skip the file
    }
}
