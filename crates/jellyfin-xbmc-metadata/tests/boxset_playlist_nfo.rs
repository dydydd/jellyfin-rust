use std::path::Path;
use jellyfin_xbmc_metadata::{
    boxset::parse_box_set_xml,
    location::{
        album_nfo_save_paths, artist_nfo_save_paths, box_set_nfo_save_paths,
        episode_nfo_save_paths, playlist_nfo_save_paths, season_nfo_save_paths,
        series_nfo_save_paths,
    },
    movie::parse_movie_nfo,
    playlist::parse_playlist_xml,
};

#[test]
fn parses_boxset_xml_with_collection_items_and_metadata() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<Item>
    <Added>2023-01-01 12:00:00</Added>
    <LockData>true</LockData>
    <LocalTitle>Marvel Cinematic Universe</LocalTitle>
    <SortName>Marvel 01</SortName>
    <SortTitle>Marvel 01 Forced</SortTitle>
    <DisplayOrder>Chronological</DisplayOrder>
    <Plot>Complete MCU Collection</Plot>
    <Genre>Action/Adventure/Sci-Fi</Genre>
    <Studio>Marvel Studios</Studio>
    <Tag>Superhero</Tag>
    <Style>Comic Book</Style>
    <TmdbId>86311</TmdbId>
    <CollectionItems>
        <CollectionItem>
            <Path>/media/movies/Iron Man (2008)/Iron Man.mkv</Path>
            <ItemId>b1a2c3d4-0000-0000-0000-000000000001</ItemId>
        </CollectionItem>
        <CollectionItem>
            <Path>/media/movies/The Avengers (2012)/The Avengers.mkv</Path>
        </CollectionItem>
    </CollectionItems>
</Item>"#;

    let boxset = parse_box_set_xml(xml).expect("parse boxset xml");
    assert_eq!(boxset.name.as_deref(), Some("Marvel Cinematic Universe"));
    assert_eq!(boxset.sort_name.as_deref(), Some("Marvel 01"));
    assert_eq!(boxset.forced_sort_name.as_deref(), Some("Marvel 01 Forced"));
    assert_eq!(boxset.display_order.as_deref(), Some("Chronological"));
    assert_eq!(boxset.overview.as_deref(), Some("Complete MCU Collection"));
    assert!(boxset.is_locked);
    assert_eq!(boxset.genres, vec!["Action", "Adventure", "Sci-Fi"]);
    assert_eq!(boxset.studios, vec!["Marvel Studios"]);
    assert!(boxset.tags.contains(&"Superhero".to_owned()));
    assert!(boxset.tags.contains(&"Comic Book".to_owned()));
    assert_eq!(boxset.provider_ids.get("Tmdb").map(String::as_str), Some("86311"));
    assert_eq!(boxset.collection_items.len(), 2);
    assert_eq!(
        boxset.collection_items[0].path.as_deref(),
        Some("/media/movies/Iron Man (2008)/Iron Man.mkv")
    );
    assert_eq!(
        boxset.collection_items[0].library_item_id.as_deref(),
        Some("b1a2c3d4-0000-0000-0000-000000000001")
    );
    assert_eq!(
        boxset.collection_items[1].path.as_deref(),
        Some("/media/movies/The Avengers (2012)/The Avengers.mkv")
    );
}

#[test]
fn parses_playlist_xml_with_items_and_shares() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<Playlist>
    <Name>Road Trip Hits</Name>
    <Overview>Best tracks for driving</Overview>
    <PlaylistMediaType>Audio</PlaylistMediaType>
    <LockData>false</LockData>
    <PlaylistItems>
        <PlaylistItem>
            <Path>/music/Artist/Album/Song1.flac</Path>
            <ItemId>a0000000-0000-0000-0000-000000000001</ItemId>
        </PlaylistItem>
    </PlaylistItems>
    <Shares>
        <Share>
            <UserId>11111111-2222-3333-4444-555555555555</UserId>
            <CanEdit>true</CanEdit>
        </Share>
    </Shares>
</Playlist>"#;

    let playlist = parse_playlist_xml(xml).expect("parse playlist xml");
    assert_eq!(playlist.name.as_deref(), Some("Road Trip Hits"));
    assert_eq!(playlist.overview.as_deref(), Some("Best tracks for driving"));
    assert_eq!(playlist.playlist_media_type.as_deref(), Some("Audio"));
    assert_eq!(playlist.playlist_items.len(), 1);
    assert_eq!(
        playlist.playlist_items[0].path.as_deref(),
        Some("/music/Artist/Album/Song1.flac")
    );
    assert_eq!(playlist.shares.len(), 1);
    assert_eq!(
        playlist.shares[0].user_id.as_deref(),
        Some("11111111-2222-3333-4444-555555555555")
    );
    assert!(playlist.shares[0].can_edit);
}

#[test]
fn parses_movie_nfo_sortname_tags_displayorder_and_dynamic_provider_ids() {
    let xml = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<movie>
    <title>Inception</title>
    <sortname>Inception 2010</sortname>
    <sorttitle>Forced Inception</sorttitle>
    <displayorder>Original</displayorder>
    <tag>Mind-Bending</tag>
    <style>Neo-Noir</style>
    <anidbid>12345</anidbid>
    <tvmaze_id>67890</tvmaze_id>
    <zap2itid>SH010101</zap2itid>
    <uniqueid type="customprovider">CUSTOM999</uniqueid>
</movie>"#;

    let movie = parse_movie_nfo(xml).expect("parse movie nfo");
    assert_eq!(movie.name.as_deref(), Some("Inception"));
    assert_eq!(movie.sort_name.as_deref(), Some("Inception 2010"));
    assert_eq!(movie.forced_sort_name.as_deref(), Some("Forced Inception"));
    assert_eq!(movie.display_order.as_deref(), Some("Original"));
    assert!(movie.tags.contains(&"Mind-Bending".to_owned()));
    assert!(movie.tags.contains(&"Neo-Noir".to_owned()));
    assert_eq!(movie.provider_ids.get("AniDB").map(String::as_str), Some("12345"));
    assert_eq!(movie.provider_ids.get("Tvmaze").map(String::as_str), Some("67890"));
    assert_eq!(movie.provider_ids.get("Zap2It").map(String::as_str), Some("SH010101"));
    assert_eq!(movie.provider_ids.get("Customprovider").map(String::as_str), Some("CUSTOM999"));
}

#[test]
fn verifies_nfo_save_paths_for_all_item_types() {
    let ep = Path::new("/tv/Show/Season 1/Show - S01E01.mkv");
    assert_eq!(episode_nfo_save_paths(ep), vec![Path::new("/tv/Show/Season 1/Show - S01E01.nfo")]);

    let series = Path::new("/tv/Show");
    assert_eq!(series_nfo_save_paths(series), vec![Path::new("/tv/Show/tvshow.nfo")]);

    let season = Path::new("/tv/Show/Season 01");
    let season_paths = season_nfo_save_paths(season, Some(1));
    assert!(season_paths.contains(&Path::new("/tv/Show/Season 01/season.nfo").to_path_buf()));
    assert!(season_paths.contains(&Path::new("/tv/Show/Season 01/season01.nfo").to_path_buf()));

    let artist = Path::new("/music/Artist");
    assert_eq!(artist_nfo_save_paths(artist), vec![Path::new("/music/Artist/artist.nfo")]);

    let album = Path::new("/music/Artist/Album");
    assert_eq!(album_nfo_save_paths(album), vec![Path::new("/music/Artist/Album/album.nfo")]);

    let boxset = Path::new("/collections/MCU");
    assert_eq!(
        box_set_nfo_save_paths(boxset),
        vec![
            Path::new("/collections/MCU/collection.xml"),
            Path::new("/collections/MCU/boxset.xml"),
        ]
    );

    let playlist = Path::new("/playlists/Party");
    assert_eq!(playlist_nfo_save_paths(playlist), vec![Path::new("/playlists/Party/playlist.xml")]);
}
