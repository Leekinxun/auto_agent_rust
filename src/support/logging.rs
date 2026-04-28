use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

use crate::config::model::LoggingConfig;

type DateProvider = dyn Fn() -> io::Result<LocalDate> + Send + Sync;

pub fn init_tracing(repo_root: &Path, logging: &LoggingConfig) -> Result<()> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("auto_claude_code_rs=info,tower_http=info"));
    let log_dir = resolve_log_dir(repo_root, &logging.dir);
    let writer = AppLogMakeWriter::new(log_dir.clone())
        .with_context(|| format!("failed to initialize log writer at {}", log_dir.display()))?;

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .init();
    Ok(())
}

fn resolve_log_dir(repo_root: &Path, configured_dir: &str) -> PathBuf {
    let dir = PathBuf::from(configured_dir);
    if dir.is_absolute() {
        dir
    } else {
        repo_root.join(dir)
    }
}

#[derive(Clone)]
struct AppLogMakeWriter {
    file_writer: Arc<Mutex<DailyFileWriter>>,
}

impl AppLogMakeWriter {
    fn new(log_dir: PathBuf) -> io::Result<Self> {
        Ok(Self {
            file_writer: Arc::new(Mutex::new(DailyFileWriter::new(log_dir)?)),
        })
    }
}

impl<'a> MakeWriter<'a> for AppLogMakeWriter {
    type Writer = AppLogWriterGuard;

    fn make_writer(&'a self) -> Self::Writer {
        AppLogWriterGuard {
            stdout: io::stdout(),
            file_writer: self.file_writer.clone(),
        }
    }
}

struct AppLogWriterGuard {
    stdout: io::Stdout,
    file_writer: Arc<Mutex<DailyFileWriter>>,
}

impl Write for AppLogWriterGuard {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdout.write_all(buf)?;
        let mut file_writer = self
            .file_writer
            .lock()
            .map_err(|_| io::Error::other("log writer lock poisoned"))?;
        file_writer.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()?;
        let mut file_writer = self
            .file_writer
            .lock()
            .map_err(|_| io::Error::other("log writer lock poisoned"))?;
        file_writer.flush()
    }
}

struct DailyFileWriter {
    log_dir: PathBuf,
    current_date: LocalDate,
    file: File,
    date_provider: Arc<DateProvider>,
}

impl DailyFileWriter {
    fn new(log_dir: PathBuf) -> io::Result<Self> {
        Self::with_date_provider(log_dir, Arc::new(current_local_date))
    }

    fn with_date_provider(log_dir: PathBuf, date_provider: Arc<DateProvider>) -> io::Result<Self> {
        fs::create_dir_all(&log_dir)?;
        let current_date = date_provider()?;
        let file = open_log_file(&log_dir, current_date)?;
        Ok(Self {
            log_dir,
            current_date,
            file,
            date_provider,
        })
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.rotate_if_needed()?;
        self.file.write_all(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let next_date = (self.date_provider)()?;
        if next_date == self.current_date {
            return Ok(());
        }

        self.file.flush()?;
        self.file = open_log_file(&self.log_dir, next_date)?;
        self.current_date = next_date;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LocalDate {
    year: i32,
    month: u8,
    day: u8,
}

fn open_log_file(log_dir: &Path, date: LocalDate) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file_path(log_dir, date))
}

fn log_file_path(log_dir: &Path, date: LocalDate) -> PathBuf {
    log_dir.join(format!(
        "backend-{:04}-{:02}-{:02}.log",
        date.year, date.month, date.day
    ))
}

#[cfg(unix)]
fn current_local_date() -> io::Result<LocalDate> {
    use std::mem::MaybeUninit;
    use std::os::raw::{c_char, c_int, c_long};

    type TimeT = c_long;

    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *const c_char,
    }

    unsafe extern "C" {
        fn time(time_ptr: *mut TimeT) -> TimeT;
        fn localtime_r(time_ptr: *const TimeT, result: *mut Tm) -> *mut Tm;
    }

    let mut now: TimeT = 0;
    let read = unsafe { time(&mut now as *mut TimeT) };
    if read < 0 {
        return Err(io::Error::other("failed to read system time"));
    }

    let mut tm = MaybeUninit::<Tm>::uninit();
    let converted = unsafe { localtime_r(&now as *const TimeT, tm.as_mut_ptr()) };
    if converted.is_null() {
        return Err(io::Error::other("failed to convert local time"));
    }

    let tm = unsafe { tm.assume_init() };
    Ok(LocalDate {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u8,
        day: tm.tm_mday as u8,
    })
}

#[cfg(not(unix))]
fn current_local_date() -> io::Result<LocalDate> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| io::Error::other(format!("system time error: {error}")))?
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    Ok(LocalDate { year, month, day })
}

#[cfg(not(unix))]
fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u8, u8) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u8, day as u8)
}

#[cfg(test)]
mod tests {
    use super::{DailyFileWriter, LocalDate, log_file_path, resolve_log_dir};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn resolves_relative_log_dir_from_repo_root() {
        let repo_root = PathBuf::from("/tmp/project");
        assert_eq!(resolve_log_dir(&repo_root, "logs"), repo_root.join("logs"));
    }

    #[test]
    fn rotates_log_file_when_date_changes() {
        let temp_dir = unique_temp_dir("log-rotate");
        let today = Arc::new(Mutex::new(LocalDate {
            year: 2026,
            month: 4,
            day: 28,
        }));
        let provider_state = today.clone();
        let provider =
            Arc::new(move || Ok(*provider_state.lock().expect("date provider lock poisoned")));

        let mut writer = DailyFileWriter::with_date_provider(temp_dir.clone(), provider).unwrap();
        writer.write_all(b"first line\n").unwrap();
        writer.flush().unwrap();

        *today.lock().unwrap() = LocalDate {
            year: 2026,
            month: 4,
            day: 29,
        };
        writer.write_all(b"second line\n").unwrap();
        writer.flush().unwrap();

        let first_log = fs::read_to_string(log_file_path(
            &temp_dir,
            LocalDate {
                year: 2026,
                month: 4,
                day: 28,
            },
        ))
        .unwrap();
        let second_log = fs::read_to_string(log_file_path(
            &temp_dir,
            LocalDate {
                year: 2026,
                month: 4,
                day: 29,
            },
        ))
        .unwrap();

        assert!(first_log.contains("first line"));
        assert!(!first_log.contains("second line"));
        assert!(second_log.contains("second line"));

        let _ = fs::remove_dir_all(temp_dir);
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("auto-claude-code-rs-{prefix}-{nanos}"))
    }
}
