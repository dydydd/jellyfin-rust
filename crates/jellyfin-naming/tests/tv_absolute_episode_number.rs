use jellyfin_naming::{EpisodeResolver, NamingOptions};

#[test]
fn official_absolute_episode_number_matrix() {
    let resolver = EpisodeResolver::new(NamingOptions::default());
    let cases = [
        ("The Simpsons/12.avi", 12),
        ("The Simpsons/The Simpsons 12.avi", 12),
        ("The Simpsons/The Simpsons 82.avi", 82),
        ("The Simpsons/The Simpsons 112.avi", 112),
        ("The Simpsons/Foo_ep_02.avi", 2),
        ("The Simpsons/The Simpsons 889.avi", 889),
        ("The Simpsons/The Simpsons 101.avi", 101),
    ];
    assert_eq!(cases.len(), 7);

    for (path, expected) in cases {
        let result = resolver.resolve_with_options(path, false, None, None, Some(true), true);
        assert_eq!(
            result.and_then(|value| value.episode_number),
            Some(expected),
            "{path}"
        );
    }
}
