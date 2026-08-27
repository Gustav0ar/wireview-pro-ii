use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use wireview_core::history::{HistoryEntry, visit_history};

static TEMPORARY_FILE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryFormat {
    Csv,
    Json,
    Raw,
}

impl HistoryFormat {
    pub(crate) fn parse(value: &str) -> Result<Self, ExportError> {
        match value {
            "csv" => Ok(Self::Csv),
            "json" => Ok(Self::Json),
            "raw" => Ok(Self::Raw),
            _ => Err(ExportError::UnsupportedFormat(value.to_owned())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExportError {
    #[error("unsupported history format {0:?}")]
    UnsupportedFormat(String),
    #[error("the destination path has no file name")]
    MissingFileName,
    #[error("failed to write {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub(crate) fn export_history(
    bytes: &[u8],
    format: HistoryFormat,
    destination: &Path,
) -> Result<usize, ExportError> {
    let mut output = AtomicOutput::create(destination)?;
    let entries = match format {
        HistoryFormat::Raw => {
            output
                .writer()
                .write_all(bytes)
                .map_err(|source| ExportError::Io {
                    path: destination.to_owned(),
                    source,
                })?;
            0
        }
        HistoryFormat::Csv => write_csv(output.writer(), bytes, destination)?,
        HistoryFormat::Json => write_json(output.writer(), bytes, destination)?,
    };
    output.commit()?;
    Ok(entries)
}

pub(crate) fn export_bytes(bytes: &[u8], destination: &Path) -> Result<(), ExportError> {
    let mut output = AtomicOutput::create(destination)?;
    output
        .writer()
        .write_all(bytes)
        .map_err(|source| ExportError::Io {
            path: destination.to_owned(),
            source,
        })?;
    output.commit()
}

fn write_csv(
    output: &mut BufWriter<File>,
    bytes: &[u8],
    destination: &Path,
) -> Result<usize, ExportError> {
    writeln!(
        output,
        "device_time_ms,total_power_w,total_current_a,avg_voltage_v,input_temp_c,output_temp_c,cable_capability_w"
    )
    .map_err(|source| ExportError::Io {
        path: destination.to_owned(),
        source,
    })?;
    let summary = visit_history(bytes, |entry| {
        writeln!(
            output,
            "{},{:.3},{:.3},{:.3},{:.1},{:.1},{}",
            entry.device_time_ms,
            entry.metrics.total_power_w,
            entry.metrics.total_current_a,
            entry.metrics.avg_voltage_v,
            entry.metrics.temperatures.input_c,
            entry.metrics.temperatures.output_c,
            entry.metrics.cable_capability_w,
        )
        .map_err(|source| ExportError::Io {
            path: destination.to_owned(),
            source,
        })
    })?;
    Ok(summary.entries)
}

fn write_json(
    output: &mut BufWriter<File>,
    bytes: &[u8],
    destination: &Path,
) -> Result<usize, ExportError> {
    output.write_all(b"[\n").map_err(|source| ExportError::Io {
        path: destination.to_owned(),
        source,
    })?;
    let mut first = true;
    let summary = visit_history(bytes, |entry: HistoryEntry| {
        if !first {
            output.write_all(b",\n").map_err(|source| ExportError::Io {
                path: destination.to_owned(),
                source,
            })?;
        }
        first = false;
        serde_json::to_writer(&mut *output, &entry).map_err(|source| ExportError::Io {
            path: destination.to_owned(),
            source: std::io::Error::other(source),
        })
    })?;
    output
        .write_all(b"\n]\n")
        .map_err(|source| ExportError::Io {
            path: destination.to_owned(),
            source,
        })?;
    Ok(summary.entries)
}

struct AtomicOutput {
    destination: PathBuf,
    temporary: PathBuf,
    writer: Option<BufWriter<File>>,
    committed: bool,
}

impl AtomicOutput {
    fn create(destination: &Path) -> Result<Self, ExportError> {
        let file_name = destination
            .file_name()
            .ok_or(ExportError::MissingFileName)?;
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        for _ in 0..64 {
            let id = TEMPORARY_FILE_ID.fetch_add(1, Ordering::Relaxed);
            let temporary = parent.join(format!(
                ".{}.wireview-{}-{id}.tmp",
                file_name.to_string_lossy(),
                std::process::id()
            ));
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
            {
                Ok(file) => {
                    return Ok(Self {
                        destination: destination.to_owned(),
                        temporary,
                        writer: Some(BufWriter::new(file)),
                        committed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(ExportError::Io {
                        path: temporary,
                        source,
                    });
                }
            }
        }
        Err(ExportError::Io {
            path: destination.to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate a temporary output file",
            ),
        })
    }

    fn writer(&mut self) -> &mut BufWriter<File> {
        self.writer.as_mut().expect("writer exists before commit")
    }

    fn commit(mut self) -> Result<(), ExportError> {
        let mut writer = self.writer.take().expect("writer exists before commit");
        writer.flush().map_err(|source| ExportError::Io {
            path: self.temporary.clone(),
            source,
        })?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|source| ExportError::Io {
                path: self.temporary.clone(),
                source,
            })?;
        drop(writer);
        fs::rename(&self.temporary, &self.destination).map_err(|source| ExportError::Io {
            path: self.destination.clone(),
            source,
        })?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for AtomicOutput {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "wireview-gui-export-{}-{}",
            std::process::id(),
            TEMPORARY_FILE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn raw_export_replaces_the_destination_only_after_success() {
        let directory = temporary_directory();
        let destination = directory.join("history.raw");
        fs::write(&destination, b"old").unwrap();

        export_history(b"new", HistoryFormat::Raw, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
