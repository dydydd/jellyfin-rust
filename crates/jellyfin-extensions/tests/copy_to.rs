use jellyfin_extensions::{CopyToError, CopyToExtensions, copy_to};

#[test]
fn copy_to_valid_official_rows_are_correct() {
    for (source, mut destination, index, expected) in [
        (
            vec![0, 1, 2, 3, 4, 5],
            vec![0, 0, 0, 0, 0, 0],
            0,
            vec![0, 1, 2, 3, 4, 5],
        ),
        (
            vec![0, 1, 2],
            vec![5, 4, 3, 2, 1, 0],
            2,
            vec![5, 4, 0, 1, 2, 0],
        ),
    ] {
        source.as_slice().copy_to(&mut destination, index).unwrap();
        assert_eq!(destination, expected);
    }
}

#[test]
fn copy_to_invalid_official_rows_return_typed_bounds_errors() {
    let rows = [
        (vec![0, 1, 2, 3, 4, 5], vec![0, 0, 0, 0, 0, 0], -1),
        (vec![0, 1, 2], vec![5, 4, 3, 2, 1, 0], 6),
        (vec![0, 1, 2], vec![], 0),
        (vec![0, 1, 2, 3, 4, 5], vec![0], 0),
        (vec![0, 1, 2, 3, 4, 5], vec![0, 0, 0, 0, 0, 0], 1),
    ];

    for (source, mut destination, index) in rows {
        let original = destination.clone();
        let error = copy_to(&source, &mut destination, index).unwrap_err();
        if index < 0 {
            assert_eq!(error, CopyToError::NegativeIndex { index });
        } else {
            assert_eq!(
                error,
                CopyToError::InsufficientDestinationSpace {
                    index: index as usize,
                    source_len: source.len(),
                    destination_len: destination.len(),
                }
            );
        }
        assert_eq!(destination, original, "failed copies must be atomic");
    }
}

#[test]
fn copy_to_clones_non_copy_elements_and_only_overwrites_the_target_range() {
    let source = vec!["middle".to_owned(), "end".to_owned()];
    let mut destination = vec!["before".to_owned(), "old".to_owned(), "old".to_owned()];

    copy_to(&source, &mut destination, 1).unwrap();

    assert_eq!(destination, ["before", "middle", "end"]);
}
