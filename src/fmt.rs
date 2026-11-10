//! Display helpers.

/// Format a Unix timestamp as a UTC date (YYYY-MM-DD), dependency-free.
/// Uses Howard Hinnant's civil_from_days algorithm.
pub fn format_date_utc(epoch_seconds: u64) -> String {
    let days = (epoch_seconds / 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Convert days since 1970-01-01 to a (year, month, day) proleptic
/// Gregorian date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era as i64 + era * 400;
    let day_of_year =
        day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let mp = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (year + (month <= 2) as i64, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_dates() {
        assert_eq!(format_date_utc(0), "1970-01-01");
        assert_eq!(format_date_utc(1_700_000_000), "2023-11-14");
        assert_eq!(format_date_utc(951_782_400), "2000-02-29"); // leap day
        assert_eq!(format_date_utc(1_794_143_179), "2026-11-08");
    }
}
