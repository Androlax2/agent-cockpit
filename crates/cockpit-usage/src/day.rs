use std::fmt;

use crate::Error;
use crate::error::invalid;

pub(crate) const SECONDS_PER_DAY: i64 = 86_400;
/// 2000-01-01 00:00 UTC: the first moment a record may name, as [`Day::parse`] starts there.
pub(crate) const FIRST_RECORDABLE_SECOND: i64 = 946_684_800;
/// 10000-01-01 00:00 UTC: the first moment past the last year a day is written with four digits.
pub(crate) const END_RECORDABLE_SECOND: i64 = 253_402_300_800;
/// The most days one digest range holds: a year and a leap day.
pub const MAX_RANGE_DAYS: i64 = 366;

/// A calendar day in UTC, `2026-11-02`: the day providers bill by, so a day's cost here is the
/// day's cost on the provider's own report, whatever time zone the owner lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Day {
    year: i64,
    month: u8,
    day: u8,
}

impl Day {
    /// The UTC day containing `unix_seconds`.
    pub fn containing(unix_seconds: i64) -> Day {
        let (year, month, day) = civil_from_days(unix_seconds.div_euclid(SECONDS_PER_DAY));
        Day { year, month, day }
    }

    /// `YYYY-MM-DD`, a real date from the year 2000 on.
    pub fn parse(text: &str) -> Result<Day, Error> {
        let reject = || {
            invalid(
                "day",
                format!("{text:?} is not a UTC date written YYYY-MM-DD"),
            )
        };
        let bytes = text.as_bytes();
        let is_shaped = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(position, byte)| matches!(position, 4 | 7) || byte.is_ascii_digit());
        if !is_shaped {
            return Err(reject());
        }
        let year: i64 = text[..4].parse().map_err(|_| reject())?;
        let month: u8 = text[5..7].parse().map_err(|_| reject())?;
        let day: u8 = text[8..].parse().map_err(|_| reject())?;
        let parsed = Day { year, month, day };
        let is_real = year >= 2000 && Day::containing(parsed.first_second()) == parsed;
        if !is_real {
            return Err(reject());
        }
        Ok(parsed)
    }

    /// Unix seconds at this day's midnight.
    pub fn first_second(self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * SECONDS_PER_DAY
    }

    /// Every day from `first` to `last`, both included, oldest first; refuses a range that ends
    /// before it starts or spans more than [`MAX_RANGE_DAYS`].
    pub(crate) fn range(first: Day, last: Day) -> Result<Vec<Day>, Error> {
        let count = (last.first_second() - first.first_second()).div_euclid(SECONDS_PER_DAY) + 1;
        if !(1..=MAX_RANGE_DAYS).contains(&count) {
            return Err(invalid(
                "days",
                format!("{first} to {last} is not 1 to {MAX_RANGE_DAYS} days"),
            ));
        }
        Ok((0..count)
            .map(|offset| Day::containing(first.first_second() + offset * SECONDS_PER_DAY))
            .collect())
    }

    /// The day `days` before this one.
    pub fn minus_days(self, days: u32) -> Day {
        Day::containing(self.first_second() - i64::from(days) * SECONDS_PER_DAY)
    }
}

impl fmt::Display for Day {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let (month, day) = (i64::from(month), i64::from(day));
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        u8::try_from(month).expect("a month is 1 to 12"),
        u8::try_from(day).expect("a day is 1 to 31"),
    )
}
