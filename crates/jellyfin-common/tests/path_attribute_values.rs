use jellyfin_common::{
    AttributeValueError, AttributeValueInput, ProviderIdParsers, get_attribute_value,
};

#[test]
fn get_attribute_value_valid_args_official_matrix() {
    let cases = [
        (
            "Superman: Red Son [imdbid=tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [imdb=tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [imdbid-tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [imdb-tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son - tt10985510",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son {imdbid=tt10985510}",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son {imdb=tt10985510}",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son {imdbid-tt10985510}",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son {imdb-tt10985510}",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son (imdbid=tt10985510)",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son (imdb=tt10985510)",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son (imdbid-tt10985510)",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son (imdb-tt10985510)",
            "imdbid",
            Some("tt10985510"),
        ),
        ("Superman: Red Son", "imdbid", None),
        (
            "Superman: Red Son [imdbid1=tt11111111][imdbid=tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [imdbid1=tt11111111][imdb=tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son {imdbid1=tt11111111}(imdbid=tt10985510)",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son {imdbid1=tt11111111}(imdb=tt10985510)",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son (imdbid1-tt11111111)[imdbid=tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son (imdbid1-tt11111111)[imdb=tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [tmdbid=618355][imdbid=tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [tmdbid=618355][imdb=tt10985510]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [tmdbid-618355]{imdbid-tt10985510}",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [tmdbid-618355]{imdb-tt10985510}",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son (tmdbid-618355)[imdbid-tt10985510]",
            "tmdbid",
            Some("618355"),
        ),
        (
            "Superman: Red Son (tmdbid-618355)[imdb-tt10985510]",
            "tmdbid",
            Some("618355"),
        ),
        (
            "Superman: Red Son [providera-id=1]",
            "providera-id",
            Some("1"),
        ),
        (
            "Superman: Red Son [providerb-id=2]",
            "providerb-id",
            Some("2"),
        ),
        (
            "Superman: Red Son [providera id=4]",
            "providera id",
            Some("4"),
        ),
        (
            "Superman: Red Son [providerb id=5]",
            "providerb id",
            Some("5"),
        ),
        (
            "Superman: Red Son [provider=99][providerid=5]",
            "providerid",
            Some("5"),
        ),
        ("Superman: Red Son [tmdbid=3]", "tmdbid", Some("3")),
        ("Superman: Red Son [tmdb=3]", "tmdbid", Some("3")),
        ("Superman: Red Son [tmdbid-3]", "tmdbid", Some("3")),
        ("Superman: Red Son [tmdb-3]", "tmdbid", Some("3")),
        ("Superman: Red Son {tmdbid=3}", "tmdbid", Some("3")),
        ("Superman: Red Son {tmdb=3}", "tmdbid", Some("3")),
        ("Superman: Red Son {tmdbid-3}", "tmdbid", Some("3")),
        ("Superman: Red Son {tmdb-3}", "tmdbid", Some("3")),
        ("Superman: Red Son (tmdbid=6)", "tmdbid", Some("6")),
        ("Superman: Red Son (tmdb=6)", "tmdbid", Some("6")),
        ("Superman: Red Son (tmdbid-6)", "tmdbid", Some("6")),
        ("Superman: Red Son (tmdb-6)", "tmdbid", Some("6")),
        ("Superman: Red Son [tvdbid=6]", "tvdbid", Some("6")),
        ("Superman: Red Son [tvdb=6]", "tvdbid", Some("6")),
        ("Superman: Red Son [tvdbid-6]", "tvdbid", Some("6")),
        ("Superman: Red Son [tvdb-6]", "tvdbid", Some("6")),
        ("Superman: Red Son {tvdbid=3}", "tvdbid", Some("3")),
        ("Superman: Red Son {tvdb=3}", "tvdbid", Some("3")),
        ("Superman: Red Son {tvdbid-3}", "tvdbid", Some("3")),
        ("Superman: Red Son {tvdb-3}", "tvdbid", Some("3")),
        ("Superman: Red Son (tvdbid=6)", "tvdbid", Some("6")),
        ("Superman: Red Son (tvdb=6)", "tvdbid", Some("6")),
        ("Superman: Red Son (tvdbid-6)", "tvdbid", Some("6")),
        ("Superman: Red Son (tvdb-6)", "tvdbid", Some("6")),
        ("[tmdbid=618355]", "tmdbid", Some("618355")),
        ("[tmdb=618355]", "tmdbid", Some("618355")),
        ("{tmdbid=618355}", "tmdbid", Some("618355")),
        ("{tmdb=618355}", "tmdbid", Some("618355")),
        ("(tmdbid=618355)", "tmdbid", Some("618355")),
        ("(tmdb=618355)", "tmdbid", Some("618355")),
        ("[tmdbid-618355]", "tmdbid", Some("618355")),
        ("[tmdb-618355]", "tmdbid", Some("618355")),
        ("{tmdbid-618355)", "tmdbid", None),
        ("{tmdb-618355)", "tmdbid", None),
        ("[tmdbid-618355}", "tmdbid", None),
        ("[tmdb-618355}", "tmdbid", None),
        ("tmdbid=111111][tmdbid=618355]", "tmdbid", Some("618355")),
        ("tmdbid=111111][tmdb=618355]", "tmdbid", Some("618355")),
        ("[tmdbid=618355]tmdbid=111111]", "tmdbid", Some("618355")),
        ("[tmdb=618355]tmdbid=111111]", "tmdbid", Some("618355")),
        ("tmdbid=618355]", "tmdbid", None),
        ("tmdb=618355]", "tmdbid", None),
        ("[tmdbid=618355", "tmdbid", None),
        ("[tmdb=618355", "tmdbid", None),
        ("tmdbid=618355", "tmdbid", None),
        ("tmdb=618355", "tmdbid", None),
        ("tmdbid=", "tmdbid", None),
        ("tmdb=", "tmdbid", None),
        ("tmdbid", "tmdbid", None),
        ("tmdb", "tmdbid", None),
        ("[tmdbid= ][tmdbid=223344]", "tmdbid", Some("223344")),
        ("[tmdb= ][tmdb=223344]", "tmdbid", Some("223344")),
        ("[tmdbid=  ][tmdb=223344]", "tmdbid", Some("223344")),
        ("[tmdb=   ][tmdbid=223344]", "tmdbid", Some("223344")),
        ("[tmdbid=][imdbid=tt10985510]", "tmdbid", None),
        ("[tmdb=][imdbid=tt10985510]", "tmdbid", None),
        ("[tmdbid-][imdbid-tt10985510]", "tmdbid", None),
        ("[tmdb-][imdbid-tt10985510]", "tmdbid", None),
        (
            "Superman: Red Son [tmdbid-618355][tmdbid=1234567]",
            "tmdbid",
            Some("618355"),
        ),
        (
            "Superman: Red Son [tmdb-618355][tmdbid=1234567]",
            "tmdbid",
            Some("618355"),
        ),
        ("{tmdbid=}{imdbid=tt10985510}", "tmdbid", None),
        ("{tmdb=}{imdbid=tt10985510}", "tmdbid", None),
        ("(tmdbid-)(imdbid-tt10985510)", "tmdbid", None),
        ("(tmdb-)(imdbid-tt10985510)", "tmdbid", None),
        (
            "Superman: Red Son {tmdbid-618355}{tmdbid=1234567}",
            "tmdbid",
            Some("618355"),
        ),
        (
            "Superman: Red Son {tmdb-618355}{tmdbid=1234567}",
            "tmdbid",
            Some("618355"),
        ),
        (
            "Superman: Red Son - tt10985510 [imdbid1=tt11]",
            "imdbid",
            Some("tt10985510"),
        ),
        (
            "Superman: Red Son [tmdb=618355][tmdbid1=1]",
            "tmdbid",
            Some("618355"),
        ),
        (
            "Superman: Red Son [tmdb=618355][tmdbid=12345]",
            "tmdbid",
            Some("618355"),
        ),
    ];

    assert_eq!(cases.len(), 100);
    for (text, attribute, expected) in cases {
        assert_eq!(
            get_attribute_value(text, attribute),
            Ok(expected),
            "text={text:?}, attribute={attribute:?}"
        );
    }
}

#[test]
fn get_attribute_value_empty_string_official_matrix() {
    let cases = [
        (
            "",
            "",
            AttributeValueError::InvalidInput(AttributeValueInput::Text),
        ),
        (
            "Superman: Red Son [imdbid=tt10985510]",
            "",
            AttributeValueError::InvalidInput(AttributeValueInput::Attribute),
        ),
        (
            "",
            "imdbid",
            AttributeValueError::InvalidInput(AttributeValueInput::Text),
        ),
    ];

    assert_eq!(cases.len(), 3);
    for (text, attribute, expected) in cases {
        assert_eq!(
            ProviderIdParsers::get_attribute_value(text, attribute),
            Err(expected)
        );
    }
}

#[test]
fn attribute_and_alias_matching_is_ascii_case_insensitive() {
    assert_eq!(
        get_attribute_value("Movie [TMDB=42]", "TmDbId"),
        Ok(Some("42"))
    );
}
