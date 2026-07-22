use chrono::{TimeZone, Utc};
use jellyfin_server_implementations::{OrderMapper, OrderMappingError, PremiereDateOrderKey};

#[test]
fn premiere_date_order_value_matches_the_official_matrix() {
    let expected_date = date(1, 2, 3);
    let expected_production_year_date = date(4, 1, 1);

    let only_production_year = PremiereDateOrderKey::new(None, Some(4));
    let only_premiere_date = PremiereDateOrderKey::new(Some(expected_date), None);
    let both = PremiereDateOrderKey::new(Some(expected_date), Some(4));
    let neither = PremiereDateOrderKey::new(None, None);

    assert_eq!(
        OrderMapper::premiere_date_order_value(&only_production_year),
        Ok(Some(expected_production_year_date))
    );
    assert_eq!(
        OrderMapper::premiere_date_order_value(&only_premiere_date),
        Ok(Some(expected_date))
    );
    assert_eq!(
        OrderMapper::premiere_date_order_value(&both),
        Ok(Some(expected_date))
    );
    assert_eq!(OrderMapper::premiere_date_order_value(&neither), Ok(None));
}

#[test]
fn invalid_production_year_returns_a_typed_error_unless_premiere_date_wins() {
    for production_year in [i32::MIN, 0, 10_000, i32::MAX] {
        let invalid = PremiereDateOrderKey::new(None, Some(production_year));
        assert_eq!(
            OrderMapper::premiere_date_order_value(&invalid),
            Err(OrderMappingError::InvalidProductionYear(production_year))
        );

        let expected_date = date(2024, 6, 1);
        let with_premiere_date =
            PremiereDateOrderKey::new(Some(expected_date), Some(production_year));
        assert_eq!(
            OrderMapper::premiere_date_order_value(&with_premiere_date),
            Ok(Some(expected_date))
        );
    }
}

fn date(year: i32, month: u32, day: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(year, month, day, 0, 0, 0)
        .single()
        .expect("test date must be valid")
}
