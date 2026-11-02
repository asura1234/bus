use time::{Date, Month, OffsetDateTime, Time};

pub(crate) const MONTHLY_USAGE_LIMIT_ERROR: &str = "You've reached your monthly usage limit. Request higher limits to continue using Cursor (rate_limit)";

pub(crate) fn is_monthly_usage_limit(text: &str) -> bool {
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    lines.windows(2).any(|pair| {
        matches!(
            pair[0],
            "Error: You've reached your monthly usage limit"
                | "You've reached your monthly usage limit"
        ) && pair[1] == "Request higher limits to continue using Cursor"
    })
}

pub(crate) fn is_restored_monthly_usage_limit(screen: &str) -> bool {
    is_monthly_usage_limit(screen)
        && screen.lines().any(|line| {
            let line = line.trim_start().trim_start_matches('│').trim_start();
            line.starts_with(['>', '→']) && line.contains("[Pasted text #")
        })
}

pub(crate) fn monthly_reset_at(now_ms: u64) -> u64 {
    let now = OffsetDateTime::from_unix_timestamp((now_ms / 1000) as i64)
        .expect("current time must fit the provider quota timestamp");
    let (year, month) = if now.month() == Month::December {
        (now.year() + 1, Month::January)
    } else {
        (now.year(), now.month().next())
    };
    Date::from_calendar_date(year, month, 1)
        .expect("the first of the next month is a valid date")
        .with_time(Time::MIDNIGHT)
        .assume_utc()
        .unix_timestamp() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monthly_reset_is_the_next_utc_month_boundary() {
        let now = Date::from_calendar_date(2026, Month::October, 30)
            .unwrap()
            .with_time(Time::MIDNIGHT)
            .assume_utc();
        let reset = monthly_reset_at(now.unix_timestamp() as u64 * 1000);
        assert_eq!(
            OffsetDateTime::from_unix_timestamp(reset as i64).unwrap(),
            Date::from_calendar_date(2026, Month::November, 1)
                .unwrap()
                .with_time(Time::MIDNIGHT)
                .assume_utc()
        );
    }
}
