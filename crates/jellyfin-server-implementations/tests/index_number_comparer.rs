use std::cmp::Ordering;

use jellyfin_server_implementations::{IndexNumberComparer, IndexNumberOrderKey};

#[test]
fn index_number_order_matches_the_official_matrix() {
    for (x, y, expected) in [
        (None, None, Ordering::Equal),
        (Some(0), None, Ordering::Greater),
        (None, Some(0), Ordering::Less),
        (Some(1), Some(1), Ordering::Equal),
        (Some(0), Some(1), Ordering::Less),
        (Some(1), Some(0), Ordering::Greater),
    ] {
        let x = IndexNumberOrderKey::new(x);
        let y = IndexNumberOrderKey::new(y);

        assert_eq!(IndexNumberComparer::compare(&x, &y), expected);
        assert_eq!(IndexNumberComparer::compare(&y, &x), expected.reverse());
    }
}
