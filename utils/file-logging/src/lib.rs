use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use flexi_logger::{
    style, AdaptiveFormat, Age, Cleanup, Criterion, DeferredNow, Duplicate, LogSpecBuilder,
    LogSpecification, Logger, Naming, Record,
};
use std::path::Path;

fn log_format(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    write!(
        w,
        "[{} {:5} {}] {}",
        DateTime::<Utc>::from(*now.now()).to_rfc3339_opts(SecondsFormat::Secs, true),
        record.level(),
        record.module_path().unwrap_or("<unnamed>"),
        record.args()
    )
}

fn log_format_color(
    w: &mut dyn std::io::Write,
    now: &mut DeferredNow,
    record: &Record,
) -> Result<(), std::io::Error> {
    let level = record.level();
    write!(
        w,
        "[{} {:5} {}] {}",
        DateTime::<Utc>::from(*now.now()).to_rfc3339_opts(SecondsFormat::Secs, true),
        style(level, level),
        record.module_path().unwrap_or("<unnamed>"),
        record.args()
    )
}

fn set_logging_to_files(logger: Logger, log_dir: &Path) -> Logger {
    logger
        .log_to_file()
        .directory(log_dir)
        .rotate(
            Criterion::AgeOrSize(Age::Day, /*size in bytes*/ 1024 * 1024 * 1024),
            Naming::Timestamps,
            Cleanup::KeepLogAndCompressedFiles(1, 10),
        )
        .print_message()
        .duplicate_to_stderr(Duplicate::All)
}

pub fn set_logging(
    default_log_level: &str,
    log_dir: &Path,
    filters: &[(&str, log::LevelFilter)],
) -> Result<()> {
    let log_spec = LogSpecification::env_or_parse(default_log_level)?;
    let mut log_spec_builder = LogSpecBuilder::from_module_filters(log_spec.module_filters());
    for filter in filters {
        log_spec_builder.module(filter.0, filter.1);
    }
    let log_spec = log_spec_builder.finalize();

    let mut logger = Logger::with(log_spec).format(log_format);
    if log_dir.components().count() != 0 {
        logger = set_logging_to_files(logger, log_dir);
    }
    logger = logger
        .adaptive_format_for_stderr(AdaptiveFormat::Custom(log_format, log_format_color))
        .set_palette("9;11;2;7;8".to_string());

    logger.start()?;
    Ok(())
}
