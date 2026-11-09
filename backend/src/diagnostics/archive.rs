use std::io::{Seek, SeekFrom, Write};
use crate::error::{AppError, AppResult};

pub const SIZE_CAP: usize = 25 * 1024 * 1024;
/// Stored entries make the cap apply equally to the archive and expanded bytes. Reserve directory
/// overhead and a final explanation. Omit whole files, preserving valid JSON when a bundle is capped.
pub fn build(files: Vec<(String, String)>, cap: usize) -> AppResult<(tempfile::NamedTempFile, bool)> {
    let mut file = tempfile::NamedTempFile::new().map_err(io_error)?;
    let mut zip = zip::ZipWriter::new(file.as_file_mut());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).unix_permissions(0o600);
    let mut used = 22 + 2048;
    let mut omitted = Vec::new();
    for (name, text) in files {
        let cost = text.len() + 76 + name.len() * 2;
        if used + cost > cap { omitted.push(name); continue; }
        used += cost;
        zip.start_file(&name, options).map_err(zip_error)?;
        zip.write_all(text.as_bytes()).map_err(io_error)?;
    }
    let capped = !omitted.is_empty();
    if capped {
        zip.start_file("LIMITS.txt", options).map_err(zip_error)?;
        zip.write_all(format!("Bundle capped at {cap} bytes. Files omitted:\n{}\n", omitted.join("\n")).as_bytes()).map_err(io_error)?;
    }
    zip.finish().map_err(zip_error)?;
    file.seek(SeekFrom::Start(0)).map_err(io_error)?;
    Ok((file, capped))
}
fn io_error(_: std::io::Error) -> AppError { AppError::internal("Could not write diagnostics archive. Check temporary disk space.") }
fn zip_error(_: zip::result::ZipError) -> AppError { AppError::internal("Could not finish diagnostics archive.") }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cap_preserves_valid_zip_and_explains_omissions() {
        let (file, capped) = build(vec![("small.json".into(), "{}".into()), ("large.log".into(), "x".repeat(9000))], 4096).unwrap();
        assert!(capped);
        assert!(file.as_file().metadata().unwrap().len() <= 4096);
        let mut zip = zip::ZipArchive::new(file).unwrap();
        assert!(zip.by_name("small.json").is_ok());
        assert!(zip.by_name("LIMITS.txt").is_ok());
        assert!(zip.by_name("large.log").is_err());
    }
}
