use std::cmp::Ordering;

use chrono::{TimeZone, Utc};
use jellyfin_server_implementations::{PremiereDateComparer, PremiereDateOrderKey};

#[test]
fn premiere_date_order_matches_the_official_matrix() {
    for (x, y, expected) in [
        (with_date(2018, 1, 1), with_date(2018, 1, 3), Ordering::Less),
        (with_date(2019, 1, 1), with_date(3, 1, 1), Ordering::Greater),
        (with_date(2020, 1, 1), with_year(2021), Ordering::Less),
        (with_date(2022, 1, 2), with_year(2022), Ordering::Greater),
        (with_date(2024, 3, 1), with_year(2023), Ordering::Greater),
        (with_date(2025, 1, 1), with_year(0), Ordering::Greater),
        (with_date(2026, 1, 1), empty(), Ordering::Greater),
    ] {
        assert_eq!(PremiereDateComparer::compare(&x, &y), expected);
        assert_eq!(PremiereDateComparer::compare(&y, &x), expected.reverse());
    }
}

fn with_date(year: i32, month: u32, day: u32) -> PremiereDateOrderKey {
    PremiereDateOrderKey::new(
        Some(
            Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
                .single()
                .expect("test date must be valid"),
        ),
        None,
    )
}

const fn with_year(year: i32) -> PremiereDateOrderKey {
    PremiereDateOrderKey::new(None, Some(year))
}

const fn empty() -> PremiereDateOrderKey {
    PremiereDateOrderKey::new(None, None)
}
