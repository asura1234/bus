//! Reset text observed in native quota failures. Unknown zones stay unknown.
use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

pub(super) fn deadline(error: &str, now_ms: u64) -> Option<u64> {
    let reset = error.split_once("resets ")?.1;
    let (when, zone) = reset.split_once('(')?;
    let zone = zone.split_once(')')?.0;
    let offset = match zone {
        "Asia/Singapore" => UtcOffset::from_hms(8, 0, 0).ok()?,
        "UTC" | "Etc/UTC" | "GMT" => UtcOffset::UTC,
        _ => {
            let value = zone
                .strip_prefix("UTC")
                .or_else(|| zone.strip_prefix("GMT"))?;
            let sign = match value.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (hour, minute) = value.get(1..)?.split_once(':')?;
            UtcOffset::from_hms(
                hour.parse::<i8>().ok()?.checked_mul(sign)?,
                minute.parse::<i8>().ok()?.checked_mul(sign)?,
                0,
            )
            .ok()?
        }
    };
    let now = OffsetDateTime::from_unix_timestamp(i64::try_from(now_ms / 1000).ok()?)
        .ok()?
        .to_offset(offset);
    let when = when.trim();
    let (date, clock, explicit) = if let Some((date, clock)) = when.split_once(" at ") {
        let mut parts = date.split_whitespace();
        let month = match parts.next()?.to_ascii_lowercase().as_str() {
            "jan" => Month::January,
            "feb" => Month::February,
            "mar" => Month::March,
            "apr" => Month::April,
            "may" => Month::May,
            "jun" => Month::June,
            "jul" => Month::July,
            "aug" => Month::August,
            "sep" => Month::September,
            "oct" => Month::October,
            "nov" => Month::November,
            "dec" => Month::December,
            _ => return None,
        };
        let day = parts.next()?.parse::<u8>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        let year =
            now.year() + i32::from(now.month() == Month::December && month == Month::January);
        (
            Date::from_calendar_date(year, month, day).ok()?,
            clock.trim(),
            true,
        )
    } else {
        (now.date(), when, false)
    };
    let clock = clock.to_ascii_lowercase();
    let (digits, pm) = clock
        .strip_suffix("am")
        .map(|s| (s, false))
        .or_else(|| clock.strip_suffix("pm").map(|s| (s, true)))?;
    let (hour, minute) = digits.split_once(':').unwrap_or((digits, "0"));
    let hour = hour.parse::<u8>().ok()?;
    if !(1..=12).contains(&hour) {
        return None;
    }
    let time = Time::from_hms(hour % 12 + if pm { 12 } else { 0 }, minute.parse().ok()?, 0).ok()?;
    let mut target = PrimitiveDateTime::new(date, time).assume_offset(offset);
    if !explicit && target < now {
        target = target.checked_add(Duration::days(1))?;
    }
    u64::try_from(target.unix_timestamp())
        .ok()?
        .checked_mul(1000)
}
