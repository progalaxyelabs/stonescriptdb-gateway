use crate::error::{GatewayError, Result};
use flate2::read::GzDecoder;
use std::fs::{self};
use std::path::{Path, PathBuf};
use tar::Archive;
use tempfile::TempDir;
use tracing::{debug, info};

pub struct SchemaExtractor {
    temp_dir: TempDir,
    extracted_path: PathBuf,
}

impl SchemaExtractor {
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let temp_dir = TempDir::new().map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to create temp directory: {}", e),
        })?;

        let extracted_path = temp_dir.path().to_path_buf();

        // Create a GzDecoder to decompress
        let decoder = GzDecoder::new(data);

        // Create a tar archive reader
        let mut archive = Archive::new(decoder);

        // Extract all files
        archive
            .unpack(&extracted_path)
            .map_err(|e| GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to extract tar.gz: {}", e),
            })?;

        info!("Extracted schema to {:?}", extracted_path);

        Ok(Self {
            temp_dir,
            extracted_path,
        })
    }

    pub fn functions_dir(&self) -> PathBuf {
        self.find_postgresql_subdir("functions")
    }

    pub fn migrations_dir(&self) -> PathBuf {
        self.find_postgresql_subdir("migrations")
    }

    pub fn tables_dir(&self) -> PathBuf {
        self.find_postgresql_subdir("tables")
    }

    pub fn seeders_dir(&self) -> PathBuf {
        self.find_postgresql_subdir("seeders")
    }

    fn find_postgresql_subdir(&self, subdir: &str) -> PathBuf {
        // First try: direct postgresql/<subdir>
        let direct = self.extracted_path.join("postgresql").join(subdir);
        if direct.exists() {
            return direct;
        }

        // Second try: look for a directory named postgresql at any level
        if let Ok(entries) = fs::read_dir(&self.extracted_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let postgresql_in_subdir = path.join("postgresql").join(subdir);
                    if postgresql_in_subdir.exists() {
                        return postgresql_in_subdir;
                    }
                    // Also check if the entry itself is postgresql
                    if entry.file_name() == "postgresql" {
                        let sub = path.join(subdir);
                        if sub.exists() {
                            return sub;
                        }
                    }
                }
            }
        }

        // Return the expected path even if it doesn't exist
        direct
    }

    pub fn list_pssql_files(&self, dir: &Path) -> Result<Vec<PathBuf>> {
        if !dir.exists() {
            debug!("Directory {:?} does not exist, returning empty list", dir);
            return Ok(Vec::new());
        }

        let mut files: Vec<PathBuf> = Vec::new();

        for entry in fs::read_dir(dir).map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read directory {:?}: {}", dir, e),
        })? {
            let entry = entry.map_err(|e| GatewayError::SchemaExtractionFailed {
                cause: format!("Failed to read directory entry: {}", e),
            })?;

            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "pssql" {
                        files.push(path);
                    }
                }
            }
        }

        // Sort by filename for consistent ordering
        files.sort_by(|a, b| {
            a.file_name()
                .unwrap_or_default()
                .cmp(b.file_name().unwrap_or_default())
        });

        Ok(files)
    }

    pub fn read_file(&self, path: &Path) -> Result<String> {
        fs::read_to_string(path).map_err(|e| GatewayError::SchemaExtractionFailed {
            cause: format!("Failed to read file {:?}: {}", path, e),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use tar::Builder;

    fn create_test_archive() -> Vec<u8> {
        let mut archive_data = Vec::new();
        let encoder = GzEncoder::new(&mut archive_data, Compression::default());
        let mut builder = Builder::new(encoder);

        // Add a function file
        let function_content = b"CREATE OR REPLACE FUNCTION test_fn() RETURNS void AS $$ $$ LANGUAGE plpgsql;";
        let mut header = tar::Header::new_gnu();
        header.set_path("postgresql/functions/test_fn.pssql").unwrap();
        header.set_size(function_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &function_content[..]).unwrap();

        // Add a migration file
        let migration_content = b"CREATE TABLE test (id SERIAL PRIMARY KEY);";
        let mut header = tar::Header::new_gnu();
        header.set_path("postgresql/migrations/001_initial.pssql").unwrap();
        header.set_size(migration_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &migration_content[..]).unwrap();

        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();

        archive_data
    }

    #[test]
    fn test_extract_archive() {
        let archive_data = create_test_archive();
        let extractor = SchemaExtractor::from_bytes(&archive_data).unwrap();

        let functions_dir = extractor.functions_dir();
        assert!(functions_dir.exists());

        let migrations_dir = extractor.migrations_dir();
        assert!(migrations_dir.exists());

        let function_files = extractor.list_pssql_files(&functions_dir).unwrap();
        assert_eq!(function_files.len(), 1);

        let migration_files = extractor.list_pssql_files(&migrations_dir).unwrap();
        assert_eq!(migration_files.len(), 1);
    }
}
