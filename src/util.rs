use chrono::{DateTime, Utc};

/// Formats a UTC timestamp as ISO-8601 with millisecond precision and a `Z`
/// suffix, matching JavaScript's `Date.toISOString()` — the format the original
/// tool used for run ids and artifact timestamps.
pub fn iso8601(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// The current UTC time formatted with [`iso8601`].
pub fn now_iso() -> String {
    iso8601(Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn formats_iso8601_with_millis_and_z_suffix() {
        let dt = Utc.with_ymd_and_hms(2026, 6, 4, 14, 3, 12).unwrap();
        assert_eq!(iso8601(dt), "2026-06-04T14:03:12.000Z");
    }
}
