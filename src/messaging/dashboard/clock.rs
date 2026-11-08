#[derive(Clone)]
pub(super) struct HourKey {
    pub(super) start_ms: i64,
    pub(super) label: String,
}

pub(super) fn hour_with_offset(ms: u64, offset_seconds: i32) -> Option<HourKey> {
    let offset = time::UtcOffset::from_whole_seconds(offset_seconds).ok()?;
    let local = time::OffsetDateTime::from_unix_timestamp(i64::try_from(ms / 1000).ok()?)
        .ok()?
        .to_offset(offset);
    let start = local.replace_time(time::Time::from_hms(local.hour(), 0, 0).ok()?);
    Some(HourKey {
        start_ms: start.unix_timestamp() * 1000,
        label: format!(
            "{} {:02}:00 UTC{}{:02}:{:02}",
            local.date(),
            local.hour(),
            if offset_seconds < 0 { "-" } else { "+" },
            offset_seconds.abs() / 3600,
            offset_seconds.abs() % 3600 / 60
        ),
    })
}

pub(super) fn local_hour(ms: u64) -> HourKey {
    let key = i64::try_from(ms / 1000).ok().and_then(|seconds| {
        let local = crate::platform::local_datetime_at(seconds)?;
        let offset = i32::try_from(local.assume_utc().unix_timestamp() - seconds).ok()?;
        hour_with_offset(ms, offset)
    });
    key.unwrap_or_else(|| HourKey {
        start_ms: i64::MAX,
        label: "Unknown timestamp".into(),
    })
}
