use jellyfin_naming::{EpisodeResolver, NamingOptions};

#[test]
fn official_episode_number_without_season_matrix() {
    let resolver = EpisodeResolver::new(NamingOptions::default());
    let cases = [
        (8, "The Simpsons/The Simpsons.S25E08.Steal this episode.mp4"),
        (2, "The Simpsons/The Simpsons - 02 - Ep Name.avi"),
        (2, "The Simpsons/02.avi"),
        (2, "The Simpsons/02 - Ep Name.avi"),
        (2, "The Simpsons/02-Ep Name.avi"),
        (2, "The Simpsons/02.EpName.avi"),
        (2, "The Simpsons/The Simpsons - 02.avi"),
        (2, "The Simpsons/The Simpsons - 02 Ep Name.avi"),
        (7, "GJ Club (2013)/GJ Club - 07.mkv"),
        (317, "Case Closed (1996-2007)/Case Closed - 317.mkv"),
    ];
    assert_eq!(cases.len(), 10);

    for (expected, path) in cases {
        assert_eq!(
            resolver
                .resolve(path, false)
                .and_then(|value| value.episode_number),
            Some(expected),
            "{path}"
        );
    }
}

#[test]
fn e_number_requires_a_token_boundary_before_the_letter() {
    let resolver = EpisodeResolver::new(NamingOptions::default());
    assert_eq!(
        resolver
            .resolve(
                "The Simpsons/The Simpsons.S25E08.Steal this episode.mp4",
                false
            )
            .and_then(|value| value.episode_number),
        Some(8)
    );
    assert_eq!(
        resolver
            .resolve("The Simpsons/E01.Pilot.mp4", false)
            .and_then(|value| value.episode_number),
        Some(1)
    );
    assert_eq!(
        resolver
            .resolve("The Simpsons/SimpsonE01Pilot.mp4", false)
            .and_then(|value| value.episode_number),
        None
    );
}
