use std::cmp::Ordering;

use jellyfin_server_implementations::{ParentIndexNumberComparer, ParentIndexNumberOrderKey};

#[test]
fn parent_index_number_order_matches_the_official_matrix() {
    for (x, y, expected) in [
        (None, None, Ordering::Equal),
        (Some(0), None, Ordering::Greater),
        (None, Some(0), Ordering::Less),
        (Some(1), Some(1), Ordering::Equal),
        (Some(0), Some(1), Ordering::Less),
        (Some(1), Some(0), Ordering::Greater),
    ] {
        let x = ParentIndexNumberOrderKey::new(x);
        let y = ParentIndexNumberOrderKey::new(y);

        assert_eq!(ParentIndexNumberComparer::compare(&x, &y), expected);
        assert_eq!(
            ParentIndexNumberComparer::compare(&y, &x),
            expected.reverse()
        );
    }
}
