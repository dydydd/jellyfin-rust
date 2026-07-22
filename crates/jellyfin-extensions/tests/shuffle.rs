use jellyfin_extensions::{shuffle, shuffle_with};
use rand::{SeedableRng, rngs::StdRng};

#[test]
fn shuffle_64_bytes_matches_fixed_seed_fisher_yates_order() {
    let mut values = std::array::from_fn::<_, 64, _>(|index| index as u8);
    let mut rng = StdRng::seed_from_u64(0x4a65_6c6c_7966_696e);

    shuffle_with(&mut values, &mut rng);

    assert_eq!(
        values,
        [
            22, 4, 53, 36, 24, 28, 43, 5, 11, 17, 55, 52, 41, 34, 57, 49, 6, 20, 63, 46, 3, 38, 25,
            56, 14, 9, 26, 21, 23, 54, 39, 59, 40, 44, 60, 47, 7, 13, 0, 37, 51, 15, 12, 8, 58, 30,
            29, 27, 18, 19, 16, 48, 2, 33, 31, 1, 62, 10, 42, 32, 45, 61, 35, 50,
        ]
    );
}

#[test]
fn shuffle_with_preserves_every_element() {
    let original = [9, 1, 9, 3, 5, 3, 7, 1, 5, 7];
    let mut shuffled = original;
    let mut rng = StdRng::seed_from_u64(0x0073_6875_6666_6c65);

    shuffle_with(&mut shuffled, &mut rng);

    shuffled.sort_unstable();
    let mut expected = original;
    expected.sort_unstable();
    assert_eq!(shuffled, expected);
}

#[test]
fn shuffle_is_safe_for_empty_and_single_element_slices() {
    let mut empty: [u8; 0] = [];
    let mut single = [42_u8];

    shuffle(&mut empty);
    shuffle(&mut single);

    assert!(empty.is_empty());
    assert_eq!(single, [42]);
}
