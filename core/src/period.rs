use std::fmt;

use anyhow::{Result, bail};
use chrono::{Datelike, NaiveDate};

/// One calendar month. Every question paybar answers is scoped to one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Period {
    pub year: i32,
    pub month: u32,
}

impl Period {
    pub fn new(year: i32, month: u32) -> Result<Self> {
        if !(1..=12).contains(&month) {
            bail!("month out of range: {month}");
        }
        Ok(Period { year, month })
    }

    pub fn of(date: NaiveDate) -> Self {
        Period { year: date.year(), month: date.month() }
    }

    pub fn parse(s: &str) -> Result<Self> {
        let (y, m) = s
            .split_once('-')
            .ok_or_else(|| anyhow::anyhow!("period must look like YYYY-MM, got {s:?}"))?;
        let year: i32 = y
            .parse()
            .map_err(|_| anyhow::anyhow!("period must look like YYYY-MM, got {s:?}"))?;
        let month: u32 = m
            .parse()
            .map_err(|_| anyhow::anyhow!("period must look like YYYY-MM, got {s:?}"))?;
        Period::new(year, month)
    }

    pub fn days(&self) -> u32 {
        let (ny, nm) = if self.month == 12 {
            (self.year + 1, 1)
        } else {
            (self.year, self.month + 1)
        };
        let first_next = NaiveDate::from_ymd_opt(ny, nm, 1).expect("valid month start");
        let first = NaiveDate::from_ymd_opt(self.year, self.month, 1).expect("valid month start");
        (first_next - first).num_days() as u32
    }

    /// Resolve a day-of-month against this period. A due day past the end of
    /// the month is CLAMPED, never rolled into the next one: day 31 in
    /// February is the 28th (or 29th), so the expense still falls due in the
    /// month it belongs to instead of quietly skipping it.
    pub fn day(&self, due_day: u32) -> NaiveDate {
        let day = due_day.clamp(1, self.days());
        NaiveDate::from_ymd_opt(self.year, self.month, day).expect("clamped day is valid")
    }

    pub fn prev(&self) -> Period {
        if self.month == 1 {
            Period { year: self.year - 1, month: 12 }
        } else {
            Period { year: self.year, month: self.month - 1 }
        }
    }

    pub fn next(&self) -> Period {
        if self.month == 12 {
            Period { year: self.year + 1, month: 1 }
        } else {
            Period { year: self.year, month: self.month + 1 }
        }
    }

    pub fn label(&self) -> String {
        const MONTHS: [&str; 12] = [
            "January", "February", "March", "April", "May", "June",
            "July", "August", "September", "October", "November", "December",
        ];
        format!("{} {}", MONTHS[(self.month - 1) as usize], self.year)
    }
}

impl fmt::Display for Period {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year, self.month)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders_round_trip() {
        let p = Period::parse("2026-08").unwrap();
        assert_eq!((p.year, p.month), (2026, 8));
        assert_eq!(p.to_string(), "2026-08");
        assert_eq!(Period::parse("2026-8").unwrap().to_string(), "2026-08");
    }

    #[test]
    fn rejects_nonsense() {
        assert!(Period::parse("2026").is_err());
        assert!(Period::parse("2026-13").is_err());
        assert!(Period::parse("2026-00").is_err());
        assert!(Period::parse("august").is_err());
    }

    #[test]
    fn month_lengths_including_leap_years() {
        assert_eq!(Period::new(2026, 2).unwrap().days(), 28);
        assert_eq!(Period::new(2024, 2).unwrap().days(), 29);
        assert_eq!(Period::new(2000, 2).unwrap().days(), 29);
        assert_eq!(Period::new(1900, 2).unwrap().days(), 28);
        assert_eq!(Period::new(2026, 4).unwrap().days(), 30);
        assert_eq!(Period::new(2026, 12).unwrap().days(), 31);
    }

    /// The rule that keeps an expense in the month it belongs to.
    #[test]
    fn a_due_day_past_the_month_end_is_clamped_not_rolled() {
        let feb = Period::new(2026, 2).unwrap();
        assert_eq!(feb.day(31), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        let leap = Period::new(2024, 2).unwrap();
        assert_eq!(leap.day(31), NaiveDate::from_ymd_opt(2024, 2, 29).unwrap());
        let aug = Period::new(2026, 8).unwrap();
        assert_eq!(aug.day(5), NaiveDate::from_ymd_opt(2026, 8, 5).unwrap());
        assert_eq!(aug.day(0), NaiveDate::from_ymd_opt(2026, 8, 1).unwrap());
    }

    #[test]
    fn stepping_crosses_the_year_boundary() {
        let jan = Period::new(2026, 1).unwrap();
        assert_eq!(jan.prev().to_string(), "2025-12");
        let dec = Period::new(2026, 12).unwrap();
        assert_eq!(dec.next().to_string(), "2027-01");
        assert_eq!(jan.next().to_string(), "2026-02");
    }

    #[test]
    fn ordering_is_chronological() {
        assert!(Period::parse("2025-12").unwrap() < Period::parse("2026-01").unwrap());
        assert!(Period::parse("2026-02").unwrap() > Period::parse("2026-01").unwrap());
    }
}
