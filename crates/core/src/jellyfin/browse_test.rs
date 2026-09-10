use super::*;

const USER: &str = "11b6f5ee53ae423da590cc581a763d35";

fn params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn value_of(query: &str, key: &str) -> Option<String> {
    params(query)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

fn image_query(max_width: u32, max_height: u32) -> String {
    let path = primary_image_path("f137a2dd", "9e4d2c", max_width, max_height);
    path.split_once('?').expect("query string").1.to_string()
}

#[test]
fn search_term_with_space_and_ampersand_survives_the_query_string() {
    let query = ItemQuery::search("fullmetal & co").to_query(USER);
    // Not `+`: a literal space or `&` here would split the parameter and the
    // server would search for something else entirely.
    assert_eq!(
        value_of(&query, "searchTerm").as_deref(),
        Some("fullmetal%20%26%20co")
    );
    assert_eq!(
        params(&query)
            .iter()
            .filter(|(k, _)| k == "searchTerm")
            .count(),
        1
    );
}

#[test]
fn a_search_recurses_but_a_folder_listing_does_not() {
    assert_eq!(
        value_of(&ItemQuery::search("x").to_query(USER), "Recursive").as_deref(),
        Some("true")
    );
    assert_eq!(
        value_of(&ItemQuery::in_folder("lib1").to_query(USER), "Recursive").as_deref(),
        Some("false")
    );
}

#[test]
fn paging_and_type_filters_reach_the_query() {
    let query = ItemQuery::search("bebop")
        .with_types("Movie,Series")
        .page(60, 30)
        .to_query(USER);
    assert_eq!(value_of(&query, "StartIndex").as_deref(), Some("60"));
    assert_eq!(value_of(&query, "Limit").as_deref(), Some("30"));
    assert_eq!(
        value_of(&query, "IncludeItemTypes").as_deref(),
        Some("Movie%2CSeries")
    );
}

#[test]
fn a_folder_listing_omits_the_search_parameters_entirely() {
    let query = ItemQuery::in_folder("f137a2dd").to_query(USER);
    assert_eq!(value_of(&query, "ParentId").as_deref(), Some("f137a2dd"));
    assert!(value_of(&query, "searchTerm").is_none());
    assert!(value_of(&query, "Limit").is_none());
}

/// An already-aligned size staying put is the one that bites: `(n / 64 + 1) *
/// 64` would push every one of them into the next bucket and never reuse a
/// cached image. A zero box would ask the server for `maxWidth=0`.
#[test]
fn an_image_is_asked_for_at_the_next_bucket_up() {
    let query = image_query(144, 224);
    assert_eq!(value_of(&query, "maxWidth").as_deref(), Some("192"));
    assert_eq!(value_of(&query, "maxHeight").as_deref(), Some("256"));

    let query = image_query(128, 192);
    assert_eq!(value_of(&query, "maxWidth").as_deref(), Some("128"));
    assert_eq!(value_of(&query, "maxHeight").as_deref(), Some("192"));

    let query = image_query(0, 1);
    assert_eq!(value_of(&query, "maxWidth").as_deref(), Some("64"));
    assert_eq!(value_of(&query, "maxHeight").as_deref(), Some("64"));
}

/// Both are part of the server's cache key, so dropping either quietly costs
/// every hit the bucket buys.
#[test]
fn an_image_names_its_format_and_quality() {
    let query = image_query(144, 224);
    assert_eq!(value_of(&query, "format").as_deref(), Some("Webp"));
    assert!(value_of(&query, "quality").is_some(), "{query}");
}

#[test]
fn an_image_tag_cannot_inject_parameters() {
    let path = primary_image_path("f137a2dd", "a&Foo=1", 144, 224);
    assert!(path.contains("tag=a%26Foo%3D1"), "{path}");
    assert!(!path.contains("&Foo=1"), "{path}");
}
