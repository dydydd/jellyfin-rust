use jellyfin_model::CollectionType;
use jellyfin_naming::NamingOptions;
use jellyfin_server_implementations::{
    AudioFileSystemEntry, AudioParentContext, AudioResolveArgs, AudioResolver,
};

#[test]
fn resolves_official_single_audiobook_directory_matrix() {
    let cases: &[&[&str]] = &[
        &["words.mp3"],
        &["chapter 01.mp3"],
        &["part 1.mp3"],
        &["chapter 01.mp3", "non-media.txt"],
        &["title.mp3", "title.epub"],
        &["01.mp3", "subdirectory/"],
    ];

    for children in cases {
        assert!(
            resolve_directory("/parent/title", children, Some(CollectionType::Books)).is_some(),
            "expected audiobook for {children:?}"
        );
    }
}

#[test]
fn rejects_official_non_navigable_audiobook_directory_matrix() {
    let cases: &[&[&str]] = &[
        &[],
        &["subdirectory/"],
        &["non-media.txt"],
        &["Name.mp3", "Another Name.mp3"],
        &["01.mp3", "02.mp3"],
        &["chapter 01.mp3", "chapter 02.mp3"],
        &["part 1.mp3", "part 2.mp3"],
        &["chapter 01 part 01.mp3", "chapter 01 part 02.mp3"],
        &["chapter 01.mp3", "part 2.mp3"],
        &["book title.mp3", "chapter name.mp3"],
        &["01 Content.mp3", "01 Credits.mp3"],
        &["Chapter Name.mp3", "Part 1.mp3"],
    ];

    for children in cases {
        assert!(
            resolve_directory("/parent/book title", children, Some(CollectionType::Books))
                .is_none(),
            "expected no audiobook for {children:?}"
        );
    }
}

#[test]
fn only_books_directories_use_audiobook_directory_resolution() {
    for collection_type in [
        None,
        Some(CollectionType::Unknown),
        Some(CollectionType::Music),
        Some(CollectionType::Movies),
    ] {
        assert!(
            resolve_directory("/parent/title", &["title.mp3"], collection_type).is_none(),
            "unexpected audiobook for {collection_type:?}"
        );
    }

    let resolver = resolver();
    let args = AudioResolveArgs {
        collection_type: Some(CollectionType::Books),
        file_info: entry("/parent/title.mp3", false),
        file_system_children: vec![entry("/parent/title.mp3", false)],
        parent: AudioParentContext::None,
    };
    assert!(resolver.resolve(args).is_none());
}

#[test]
fn directory_resolution_preserves_item_metadata() {
    let resolved = resolve_directory(
        "/parent/The Book (2024)",
        &["chapter 01.mp3", "notes.epub", "subdirectory/"],
        Some(CollectionType::Books),
    )
    .expect("single audiobook should resolve");

    assert_eq!(resolved.path, "/parent/The Book (2024)/chapter 01.mp3");
    assert_eq!(resolved.name, "The Book (2024)");
    assert_eq!(resolved.production_year, Some(2024));
    assert!(!resolved.is_in_mixed_folder);
}

#[test]
fn multiple_resolution_retains_leftovers_and_top_parent_context() {
    let resolver = resolver();
    let entries = vec![
        entry("/parent/title/title.mp3", false),
        entry("/parent/title/title.epub", false),
        entry("/parent/title/subdirectory", true),
    ];
    let result = resolver
        .resolve_multiple(
            AudioParentContext::folder(true),
            entries,
            Some(CollectionType::Books),
        )
        .expect("books collection should produce a multi-item result");

    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].path, "/parent/title/title.mp3");
    assert_eq!(result.items[0].name, "title");
    assert!(result.items[0].is_in_mixed_folder);
    assert_eq!(
        result.extra_files,
        vec![
            entry("/parent/title/subdirectory", true),
            entry("/parent/title/title.epub", false),
        ]
    );
}

#[test]
fn multiple_resolution_uses_parsed_book_name_and_year() {
    let resolver = resolver();
    let entries = vec![entry("/parent/The Book (2024)/chapter 01.mp3", false)];
    let result = resolver
        .resolve_multiple(
            AudioParentContext::folder(false),
            entries,
            Some(CollectionType::Books),
        )
        .expect("books collection should produce a multi-item result");

    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].name, "The Book");
    assert_eq!(result.items[0].production_year, Some(2024));
}

#[test]
fn multiple_resolution_marks_multiple_named_books_as_mixed() {
    let resolver = resolver();
    let entries = vec![
        entry("/first/First.mp3", false),
        entry("/second/Second.mp3", false),
    ];
    let result = resolver
        .resolve_multiple(
            AudioParentContext::folder(false),
            entries,
            Some(CollectionType::Books),
        )
        .expect("books collection should produce a multi-item result");

    assert_eq!(result.items.len(), 2);
    assert!(result.items.iter().all(|item| item.is_in_mixed_folder));
}

fn resolve_directory(
    parent: &str,
    children: &[&str],
    collection_type: Option<CollectionType>,
) -> Option<jellyfin_server_implementations::ResolvedAudioBook> {
    let children = children
        .iter()
        .map(|name| {
            entry(
                &format!("{parent}/{}", name.trim_end_matches('/')),
                name.ends_with('/'),
            )
        })
        .collect();
    resolver().resolve(AudioResolveArgs {
        collection_type,
        file_info: entry(parent, true),
        file_system_children: children,
        parent: AudioParentContext::None,
    })
}

fn resolver() -> AudioResolver {
    AudioResolver::new(NamingOptions::default())
}

fn entry(path: &str, is_directory: bool) -> AudioFileSystemEntry {
    AudioFileSystemEntry::new(path, is_directory)
}
