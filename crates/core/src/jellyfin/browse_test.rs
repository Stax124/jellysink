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
